//! Consistent DuckDB backup and portable document bundle recovery.

use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDateTime, Utc};
use duckdb::params;
use serde::{Deserialize, Serialize};

use super::Store;
use crate::error::{AppError, Result};
use crate::models::{Chunk, Document};

pub const BUNDLE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryBundle {
    pub format: String,
    pub version: u32,
    pub exported_at: DateTime<Utc>,
    pub documents: Vec<BundleDocument>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleDocument {
    pub document: Document,
    #[serde(default)]
    pub chunks: Vec<Chunk>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Error,
    Skip,
    Overwrite,
}

impl ConflictPolicy {
    pub fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("error").trim().to_ascii_lowercase().as_str() {
            "error" => Ok(Self::Error),
            "skip" => Ok(Self::Skip),
            "overwrite" => Ok(Self::Overwrite),
            other => Err(AppError::config(format!(
                "invalid conflict_policy '{other}': expected error, skip, or overwrite"
            ))),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct BackupReport {
    pub success: bool,
    pub dry_run: bool,
    pub source: String,
    pub path: String,
    pub overwritten: bool,
    pub bytes: Option<u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BundleExportReport {
    pub success: bool,
    pub dry_run: bool,
    pub path: String,
    pub format: String,
    pub overwritten: bool,
    pub documents: u64,
    pub chunks: u64,
    pub bytes: Option<u64>,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BundleImportReport {
    pub success: bool,
    pub dry_run: bool,
    pub path: String,
    pub format: String,
    pub conflict_policy: String,
    pub documents_read: u64,
    pub documents_inserted: u64,
    pub documents_overwritten: u64,
    pub documents_skipped: u64,
    pub chunks_inserted: u64,
    pub conflicts: u64,
    pub errors: Vec<String>,
}

impl Store {
    pub fn backup_database(&self, path: &Path, dry_run: bool, overwrite: bool) -> Result<BackupReport> {
        let exists = path.exists();
        if exists && !overwrite {
            return Err(AppError::conflict(format!(
                "backup destination '{}' already exists; set overwrite=true explicitly",
                path.display()
            )));
        }
        let source = self.path().display().to_string();
        if dry_run {
            return Ok(BackupReport {
                success: true, dry_run: true, source, path: path.display().to_string(),
                overwritten: exists, bytes: self.db_file_size_bytes(), errors: Vec::new(),
            });
        }

        let conn = self.lock()?;
        conn.execute_batch("CHECKPOINT")?;
        let bytes = fs::copy(self.path(), path)?;
        drop(conn);
        Ok(BackupReport {
            success: true, dry_run: false, source, path: path.display().to_string(),
            overwritten: exists, bytes: Some(bytes), errors: Vec::new(),
        })
    }

    pub fn recovery_bundle(&self) -> Result<RecoveryBundle> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), COALESCE(revision, 1) FROM documents ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Document {
                id: row.get(0)?, uri: row.get(1)?, title: row.get(2)?, content: row.get(3)?,
                metadata_json: row.get(4)?, content_hash: row.get(5)?, wing: row.get(6)?,
                room: row.get(7)?, source_file: row.get(8)?, layer: row.get(9)?, kind: row.get(10)?,
                created_at: parse_ts(row.get::<_, String>(11)?), updated_at: parse_ts(row.get::<_, String>(12)?),
                status: row.get(13)?, pinned: row.get(14)?, boost: row.get(15)?, revision: row.get(16)?,
            })
        })?;
        let documents = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut bundled = Vec::with_capacity(documents.len());
        for document in documents {
            let mut chunks_stmt = conn.prepare(
                "SELECT id, document_id, chunk_index, content, embedding_json, char_start, char_end, COALESCE(metadata_json, '{}') FROM chunks WHERE document_id = ? ORDER BY chunk_index",
            )?;
            let rows = chunks_stmt.query_map(params![document.id], |row| {
                let raw: String = row.get(4)?;
                Ok(Chunk {
                    id: row.get(0)?, document_id: row.get(1)?, chunk_index: row.get(2)?,
                    content: row.get(3)?, embedding: serde_json::from_str(&raw).unwrap_or_default(),
                    char_start: row.get(5)?, char_end: row.get(6)?, metadata_json: row.get(7)?,
                })
            })?;
            let chunks = rows.collect::<std::result::Result<Vec<_>, _>>()?;
            bundled.push(BundleDocument { document, chunks });
        }
        Ok(RecoveryBundle {
            format: "rag-recovery-bundle".into(), version: BUNDLE_VERSION,
            exported_at: Utc::now(), documents: bundled,
        })
    }

    pub fn import_recovery_bundle(
        &self,
        bundle: &RecoveryBundle,
        policy: ConflictPolicy,
        dry_run: bool,
        path: &Path,
        format: &str,
    ) -> Result<BundleImportReport> {
        if bundle.format != "rag-recovery-bundle" || bundle.version != BUNDLE_VERSION {
            return Err(AppError::config(format!(
                "unsupported recovery bundle format/version: {}/{}",
                bundle.format, bundle.version
            )));
        }
        let mut report = BundleImportReport {
            success: true, dry_run, path: path.display().to_string(), format: format.into(),
            conflict_policy: match policy { ConflictPolicy::Error => "error", ConflictPolicy::Skip => "skip", ConflictPolicy::Overwrite => "overwrite" }.into(),
            documents_read: bundle.documents.len() as u64, documents_inserted: 0,
            documents_overwritten: 0, documents_skipped: 0, chunks_inserted: 0,
            conflicts: 0, errors: Vec::new(),
        };
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        for item in &bundle.documents {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ? OR uri = ?)",
                params![item.document.id, item.document.uri], |r| r.get(0),
            )?;
            if exists {
                report.conflicts += 1;
                match policy {
                    ConflictPolicy::Error => {
                        report.success = false;
                        report.errors.push(format!("document conflict: {} ({})", item.document.id, item.document.uri));
                        continue;
                    }
                    ConflictPolicy::Skip => { report.documents_skipped += 1; continue; }
                    ConflictPolicy::Overwrite => { report.documents_overwritten += 1; }
                }
            } else {
                report.documents_inserted += 1;
            }
            report.chunks_inserted += item.chunks.len() as u64;
            if dry_run { continue; }
            if exists {
                tx.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)) OR target_id IN (SELECT id FROM graph_nodes WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?))", params![item.document.id, item.document.uri, item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM graph_nodes WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)", params![item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM wiki_index WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)", params![item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM chunks WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)", params![item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM documents WHERE id = ? OR uri = ?", params![item.document.id, item.document.uri])?;
            }
            let d = &item.document;
            tx.execute(
                "INSERT INTO documents (id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, status, pinned, boost, revision, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS TIMESTAMP), CAST(? AS TIMESTAMP))",
                params![d.id, d.uri, d.title, d.content, d.metadata_json, d.content_hash, d.wing, d.room, d.source_file, d.layer, d.kind, d.status, d.pinned, d.boost, d.revision, d.created_at.to_rfc3339(), d.updated_at.to_rfc3339()],
            )?;
            for c in &item.chunks {
                tx.execute(
                    "INSERT INTO chunks (id, document_id, chunk_index, content, embedding_json, char_start, char_end, metadata_json, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP)",
                    params![c.id, d.id, c.chunk_index, c.content, serde_json::to_string(&c.embedding)?, c.char_start, c.char_end, c.metadata_json],
                )?;
            }
        }
        if report.success && !dry_run { tx.commit()?; }
        Ok(report)
    }
}

fn parse_ts(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f").map(|dt| dt.and_utc()))
        .unwrap_or_else(|_| Utc::now())
}
