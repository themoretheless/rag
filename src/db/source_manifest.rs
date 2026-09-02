//! Filesystem source manifest and root-scoped preflight queries.

use std::collections::HashMap;
use std::path::{Path, MAIN_SEPARATOR};

use duckdb::{params, params_from_iter};
use serde::{Deserialize, Serialize};

use super::store::Store;
use crate::error::{AppError, Result};

/// Manifest state plus the joined document/chunk health needed by source sync.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceManifestEntry {
    pub canonical_path: String,
    pub canonical_root: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub content_hash: String,
    pub document_id: String,
    pub last_seen: Option<String>,
    pub last_synced: Option<String>,
    pub manifest_present: bool,
    pub document_exists: bool,
    pub chunks_exist: bool,
    pub document_indexable: bool,
    pub document_content_hash: Option<String>,
    pub document_metadata_json: Option<String>,
    pub document_wing: Option<String>,
    pub document_room: Option<String>,
}

/// Values published after a file has been synchronized successfully.
#[derive(Debug, Clone, Copy)]
pub struct SourceManifestWrite<'a> {
    pub canonical_path: &'a str,
    pub canonical_root: &'a str,
    pub size_bytes: u64,
    pub mtime_ns: i64,
    pub content_hash: &'a str,
    pub document_id: &'a str,
}

/// Lean source-root aggregate for project registry / status surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRootSummary {
    pub canonical_root: String,
    pub file_count: u64,
    pub last_seen: Option<String>,
    pub last_synced: Option<String>,
}

impl Store {
    /// List synchronized roots, optionally restricted to one project (`wing`).
    pub fn list_source_roots(&self, project: Option<&str>) -> Result<Vec<SourceRootSummary>> {
        let project = project.map(str::trim).filter(|value| !value.is_empty());
        let where_sql = if project.is_some() {
            "WHERE d.wing = ?"
        } else {
            ""
        };
        let sql = format!(
            r#"
            SELECT
              sm.canonical_root,
              COUNT(*)::BIGINT AS file_count,
              CAST(MAX(sm.last_seen) AS VARCHAR) AS last_seen,
              CAST(MAX(sm.last_synced) AS VARCHAR) AS last_synced
            FROM source_manifest sm
            LEFT JOIN documents d ON d.id = sm.document_id
            {where_sql}
            GROUP BY sm.canonical_root
            ORDER BY sm.canonical_root ASC
            "#
        );
        let conn = self.lock()?;
        let mut stmt = conn.prepare(&sql)?;
        let mut rows = match project {
            Some(project) => stmt.query(params![project])?,
            None => stmt.query([])?,
        };
        let mut roots = Vec::new();
        while let Some(row) = rows.next()? {
            roots.push(SourceRootSummary {
                canonical_root: row.get(0)?,
                file_count: row.get::<_, i64>(1)?.max(0) as u64,
                last_seen: row.get(2)?,
                last_synced: row.get(3)?,
            });
        }
        Ok(roots)
    }

    /// Preload all manifest and legacy source-document state under one root.
    ///
    /// This is one database query regardless of file count. Joined existence
    /// flags eliminate the previous per-file document/chunk probes on the
    /// unchanged fast path. Legacy source documents are included once so the
    /// first schema-v9 sync can seed their manifest without losing metadata.
    pub fn load_source_manifest_root(
        &self,
        canonical_root: &Path,
    ) -> Result<HashMap<String, SourceManifestEntry>> {
        let root = canonical_root.display().to_string();
        let prefix = root_prefix(&root);
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            WITH root_documents AS (
              SELECT id, source_file, content, content_hash, metadata_json, wing, room
              FROM documents
              WHERE source_file = ? OR starts_with(source_file, ?)
            ), combined AS (
              SELECT
                sm.canonical_path,
                sm.canonical_root,
                sm.size_bytes,
                sm.mtime_ns,
                sm.content_hash,
                sm.document_id,
                CAST(sm.last_seen AS VARCHAR) AS last_seen,
                CAST(sm.last_synced AS VARCHAR) AS last_synced,
                true AS manifest_present,
                d.id IS NOT NULL AS document_exists,
                EXISTS (
                  SELECT 1 FROM chunks c WHERE c.document_id = sm.document_id
                ) AS chunks_exist,
                COALESCE(length(trim(d.content)) > 0, false) AS document_indexable,
                d.content_hash AS document_content_hash,
                d.metadata_json AS document_metadata_json,
                d.wing AS document_wing,
                d.room AS document_room
              FROM source_manifest sm
              LEFT JOIN root_documents d ON d.id = sm.document_id
              WHERE sm.canonical_root = ?
                 OR sm.canonical_path = ?
                 OR starts_with(sm.canonical_path, ?)

              UNION ALL

              SELECT
                d.source_file AS canonical_path,
                ? AS canonical_root,
                CAST(0 AS BIGINT) AS size_bytes,
                CAST(0 AS BIGINT) AS mtime_ns,
                COALESCE(d.content_hash, '') AS content_hash,
                d.id AS document_id,
                CAST(NULL AS VARCHAR) AS last_seen,
                CAST(NULL AS VARCHAR) AS last_synced,
                false AS manifest_present,
                true AS document_exists,
                EXISTS (
                  SELECT 1 FROM chunks c WHERE c.document_id = d.id
                ) AS chunks_exist,
                length(trim(d.content)) > 0 AS document_indexable,
                d.content_hash AS document_content_hash,
                d.metadata_json AS document_metadata_json,
                d.wing AS document_wing,
                d.room AS document_room
              FROM root_documents d
              WHERE NOT EXISTS (
                SELECT 1
                FROM source_manifest sm
                WHERE sm.canonical_path = d.source_file OR sm.document_id = d.id
              )
            )
            SELECT * FROM combined
            ORDER BY manifest_present DESC, canonical_path ASC
            "#,
        )?;
        let mut rows = stmt.query(params![root, prefix, root, root, prefix, root])?;
        let mut entries = HashMap::new();
        while let Some(row) = rows.next()? {
            let canonical_path: String = row.get(0)?;
            let size_bytes = row.get::<_, i64>(2)?.max(0) as u64;
            entries
                .entry(canonical_path.clone())
                .or_insert(SourceManifestEntry {
                    canonical_path,
                    canonical_root: row.get(1)?,
                    size_bytes,
                    mtime_ns: row.get(3)?,
                    content_hash: row.get(4)?,
                    document_id: row.get(5)?,
                    last_seen: row.get(6)?,
                    last_synced: row.get(7)?,
                    manifest_present: row.get(8)?,
                    document_exists: row.get(9)?,
                    chunks_exist: row.get(10)?,
                    document_indexable: row.get(11)?,
                    document_content_hash: row.get(12)?,
                    document_metadata_json: row.get(13)?,
                    document_wing: row.get(14)?,
                    document_room: row.get(15)?,
                });
        }
        Ok(entries)
    }

    /// Publish metadata/hash/document linkage after a successful sync.
    pub fn upsert_source_manifest(&self, entry: SourceManifestWrite<'_>) -> Result<()> {
        let conn = self.lock()?;
        upsert_source_manifest_locked(&conn, entry)
    }

    /// Update `last_seen` for a scanned root in bounded batches, without one
    /// connection acquisition/query per file.
    pub fn mark_source_manifest_seen(
        &self,
        canonical_root: &Path,
        canonical_paths: &[String],
    ) -> Result<()> {
        if canonical_paths.is_empty() {
            return Ok(());
        }
        let root = canonical_root.display().to_string();
        let conn = self.lock()?;
        for paths in canonical_paths.chunks(500) {
            let placeholders = std::iter::repeat_n("?", paths.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE source_manifest SET last_seen = now() \
                 WHERE canonical_root = ? AND canonical_path IN ({placeholders})"
            );
            let mut values = Vec::with_capacity(paths.len() + 1);
            values.push(root.clone());
            values.extend(paths.iter().cloned());
            conn.execute(&sql, params_from_iter(values.iter()))?;
        }
        Ok(())
    }

    /// Delete one manifest record. Returns true when a row was removed.
    pub fn delete_source_manifest(&self, canonical_path: &str) -> Result<bool> {
        let conn = self.lock()?;
        let deleted = conn.execute(
            "DELETE FROM source_manifest WHERE canonical_path = ?",
            params![canonical_path],
        )?;
        Ok(deleted > 0)
    }
}

pub(super) fn upsert_source_manifest_locked(
    conn: &duckdb::Connection,
    entry: SourceManifestWrite<'_>,
) -> Result<()> {
    let size_bytes = i64::try_from(entry.size_bytes)
        .map_err(|_| AppError::config("source file size exceeds BIGINT"))?;
    conn.execute(
        r#"
        INSERT INTO source_manifest
          (canonical_path, canonical_root, size_bytes, mtime_ns, content_hash,
           document_id, last_seen, last_synced)
        VALUES (?, ?, ?, ?, ?, ?, now(), now())
        ON CONFLICT (canonical_path) DO UPDATE SET
          canonical_root = excluded.canonical_root,
          size_bytes = excluded.size_bytes,
          mtime_ns = excluded.mtime_ns,
          content_hash = excluded.content_hash,
          document_id = excluded.document_id,
          last_seen = now(),
          last_synced = now()
        "#,
        params![
            entry.canonical_path,
            entry.canonical_root,
            size_bytes,
            entry.mtime_ns,
            entry.content_hash,
            entry.document_id,
        ],
    )?;
    Ok(())
}

fn root_prefix(root: &str) -> String {
    if root.ends_with(MAIN_SEPARATOR) {
        root.to_string()
    } else {
        format!("{root}{MAIN_SEPARATOR}")
    }
}
