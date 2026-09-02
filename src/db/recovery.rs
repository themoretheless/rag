//! Consistent DuckDB backup and portable document bundle recovery.

use std::fs;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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
        match value
            .unwrap_or("error")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification: Option<BackupVerification>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupVerification {
    pub ok: bool,
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub schema_version: i32,
    pub documents: u64,
    pub chunks: u64,
    pub nodes: u64,
    pub edges: u64,
    pub orphan_chunks: u64,
    pub orphan_document_nodes: u64,
    pub orphan_edges: u64,
    pub embedding_manifest: Option<crate::models::EmbeddingManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupSidecar {
    pub format: String,
    pub created_at: DateTime<Utc>,
    pub source: String,
    pub required_free_bytes: u64,
    pub verification: BackupVerification,
}

#[derive(Debug, Clone, Serialize)]
pub struct BackupInventoryItem {
    pub path: String,
    pub bytes: u64,
    pub modified_at: DateTime<Utc>,
    pub protected: bool,
    pub newest: bool,
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
    pub fn backup_database(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<BackupReport> {
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
                success: true,
                dry_run: true,
                source,
                path: path.display().to_string(),
                overwritten: exists,
                bytes: self.db_file_size_bytes(),
                errors: Vec::new(),
                sha256: None,
                verification: None,
            });
        }

        refuse_same_file(self.path(), path)?;

        let conn = self.lock()?;
        conn.execute_batch("CHECKPOINT")?;
        let bytes = fs::copy(self.path(), path)?;
        drop(conn);
        let verification = verify_backup(path)?;
        write_backup_sidecars(path, self.path(), &verification)?;
        Ok(BackupReport {
            success: true,
            dry_run: false,
            source,
            path: path.display().to_string(),
            overwritten: exists,
            bytes: Some(bytes),
            errors: Vec::new(),
            sha256: Some(verification.sha256.clone()),
            verification: Some(verification),
        })
    }

    pub fn recovery_bundle(&self) -> Result<RecoveryBundle> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR), COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), COALESCE(revision, 1) FROM documents ORDER BY id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Document {
                id: row.get(0)?,
                uri: row.get(1)?,
                title: row.get(2)?,
                content: row.get(3)?,
                metadata_json: row.get(4)?,
                content_hash: row.get(5)?,
                wing: row.get(6)?,
                room: row.get(7)?,
                source_file: row.get(8)?,
                layer: row.get(9)?,
                kind: row.get(10)?,
                created_at: parse_ts(row.get::<_, String>(11)?),
                updated_at: parse_ts(row.get::<_, String>(12)?),
                status: row.get(13)?,
                pinned: row.get(14)?,
                boost: row.get(15)?,
                revision: row.get(16)?,
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
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    chunk_index: row.get(2)?,
                    content: row.get(3)?,
                    embedding: serde_json::from_str(&raw).unwrap_or_default(),
                    char_start: row.get(5)?,
                    char_end: row.get(6)?,
                    metadata_json: row.get(7)?,
                })
            })?;
            let chunks = rows.collect::<std::result::Result<Vec<_>, _>>()?;
            bundled.push(BundleDocument { document, chunks });
        }
        Ok(RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            documents: bundled,
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
            success: true,
            dry_run,
            path: path.display().to_string(),
            format: format.into(),
            conflict_policy: match policy {
                ConflictPolicy::Error => "error",
                ConflictPolicy::Skip => "skip",
                ConflictPolicy::Overwrite => "overwrite",
            }
            .into(),
            documents_read: bundle.documents.len() as u64,
            documents_inserted: 0,
            documents_overwritten: 0,
            documents_skipped: 0,
            chunks_inserted: 0,
            conflicts: 0,
            errors: Vec::new(),
        };
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;
        let mut fts_marked_dirty = false;
        for item in &bundle.documents {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ? OR uri = ?)",
                params![item.document.id, item.document.uri],
                |r| r.get(0),
            )?;
            if exists {
                report.conflicts += 1;
                match policy {
                    ConflictPolicy::Error => {
                        report.success = false;
                        report.errors.push(format!(
                            "document conflict: {} ({})",
                            item.document.id, item.document.uri
                        ));
                        continue;
                    }
                    ConflictPolicy::Skip => {
                        report.documents_skipped += 1;
                        continue;
                    }
                    ConflictPolicy::Overwrite => {
                        report.documents_overwritten += 1;
                    }
                }
            } else {
                report.documents_inserted += 1;
            }
            report.chunks_inserted += item.chunks.len() as u64;
            if dry_run {
                continue;
            }
            if !fts_marked_dirty && (exists || !item.chunks.is_empty()) {
                super::fts::mark_fts_dirty(&tx)?;
                fts_marked_dirty = true;
            }
            if exists {
                tx.execute("DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)) OR target_id IN (SELECT id FROM graph_nodes WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?))", params![item.document.id, item.document.uri, item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM graph_nodes WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)", params![item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM wiki_index WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)", params![item.document.id, item.document.uri])?;
                tx.execute("DELETE FROM chunks WHERE document_id IN (SELECT id FROM documents WHERE id = ? OR uri = ?)", params![item.document.id, item.document.uri])?;
                tx.execute(
                    "DELETE FROM documents WHERE id = ? OR uri = ?",
                    params![item.document.id, item.document.uri],
                )?;
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
        if report.success && !dry_run {
            tx.commit()?;
        }
        Ok(report)
    }
}

pub fn verify_backup(path: &Path) -> Result<BackupVerification> {
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "backup file '{}'",
            path.display()
        )));
    }
    let bytes = fs::metadata(path)?.len();
    let sha256 = sha256_file(path)?;
    let store = Store::open(path)?;
    let schema_version = store.schema_version()?.unwrap_or(0);
    let (documents, chunks, nodes, edges) = store.stats()?;
    let (_, orphan_chunks, orphan_document_nodes, orphan_edges, _) = store.integrity_counts()?;
    let embedding_manifest = store.get_embedding_manifest()?;
    let ok = orphan_chunks == 0 && orphan_document_nodes == 0 && orphan_edges == 0;
    Ok(BackupVerification {
        ok,
        path: path.display().to_string(),
        bytes,
        sha256,
        schema_version,
        documents,
        chunks,
        nodes,
        edges,
        orphan_chunks,
        orphan_document_nodes,
        orphan_edges,
        embedding_manifest,
    })
}

pub fn backup_inventory(dir: &Path) -> Result<Vec<BackupInventoryItem>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("duckdb") {
            continue;
        }
        let metadata = entry.metadata()?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let modified_at: DateTime<Utc> = modified.into();
        let protected = path
            .file_name()
            .and_then(|v| v.to_str())
            .is_some_and(|n| n.contains("final"));
        rows.push(BackupInventoryItem {
            path: path.display().to_string(),
            bytes: metadata.len(),
            modified_at,
            protected,
            newest: false,
        });
    }
    rows.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.path.cmp(&a.path))
    });
    if let Some(first) = rows.first_mut() {
        first.newest = true;
        first.protected = true;
    }
    Ok(rows)
}

pub fn retention_preview(dir: &Path, keep: usize) -> Result<Vec<BackupInventoryItem>> {
    Ok(backup_inventory(dir)?
        .into_iter()
        .enumerate()
        .filter_map(|(index, item)| (index >= keep.max(1) && !item.protected).then_some(item))
        .collect())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut context = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        context.update(&buffer[..read]);
    }
    Ok(context
        .finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn write_backup_sidecars(
    path: &Path,
    source: &Path,
    verification: &BackupVerification,
) -> Result<()> {
    let sha_path = PathBuf::from(format!("{}.sha256", path.display()));
    let metadata_path = PathBuf::from(format!("{}.metadata.json", path.display()));
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("backup.duckdb");
    fs::write(sha_path, format!("{}  {}\n", verification.sha256, name))?;
    let sidecar = BackupSidecar {
        format: "rag-duckdb-backup".into(),
        created_at: Utc::now(),
        source: source.display().to_string(),
        required_free_bytes: verification.bytes.saturating_mul(2),
        verification: verification.clone(),
    };
    fs::write(metadata_path, serde_json::to_vec_pretty(&sidecar)?)?;
    Ok(())
}

fn refuse_same_file(source: &Path, destination: &Path) -> Result<()> {
    if source == destination {
        return Err(AppError::forbidden(
            "backup destination is the live database",
        ));
    }
    #[cfg(unix)]
    if let (Ok(a), Ok(b)) = (fs::metadata(source), fs::metadata(destination)) {
        if a.dev() == b.dev() && a.ino() == b.ino() {
            return Err(AppError::forbidden(
                "backup destination resolves to the live database inode",
            ));
        }
    }
    Ok(())
}

fn parse_ts(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .or_else(|_| {
            NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f").map(|dt| dt.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_writes_verified_sidecars_and_inventory_protects_final() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("live.duckdb");
        let backups = root.path().join("backups");
        fs::create_dir(&backups).unwrap();
        let backup = backups.join("rag-final.duckdb");
        let store = Store::open(&source).unwrap();

        let report = store.backup_database(&backup, false, false).unwrap();
        let verification = report.verification.expect("verification");
        assert!(verification.ok);
        assert_eq!(
            verification.schema_version,
            crate::db::schema::SCHEMA_VERSION
        );
        assert!(PathBuf::from(format!("{}.sha256", backup.display())).is_file());
        let metadata_path = PathBuf::from(format!("{}.metadata.json", backup.display()));
        let sidecar: BackupSidecar =
            serde_json::from_slice(&fs::read(metadata_path).unwrap()).unwrap();
        assert_eq!(sidecar.required_free_bytes, verification.bytes * 2);
        assert_eq!(sidecar.verification.sha256, verification.sha256);

        let inventory = backup_inventory(&backups).unwrap();
        assert_eq!(inventory.len(), 1);
        let final_backup = inventory
            .iter()
            .find(|item| item.path.ends_with("rag-final.duckdb"))
            .unwrap();
        assert!(final_backup.protected);
        assert!(retention_preview(&backups, 1).unwrap().is_empty());
    }

    #[test]
    fn backup_reports_destination_errors() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("live.duckdb")).unwrap();
        let missing_parent = root.path().join("missing/backup.duckdb");
        assert!(store
            .backup_database(&missing_parent, false, false)
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn backup_refuses_live_database_inode_alias() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("live.duckdb");
        let alias = root.path().join("alias.duckdb");
        let store = Store::open(&source).unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let error = store.backup_database(&alias, false, true).unwrap_err();
        assert!(error.to_string().contains("inode"));
    }
}
