//! Consistent DuckDB backup and portable document bundle recovery.

use std::fs;
use std::fs::OpenOptions;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use chrono::{DateTime, NaiveDateTime, Utc};
use duckdb::params;
use serde::{Deserialize, Serialize};

use super::Store;
use crate::error::{AppError, Result};
use crate::models::{Chunk, Document};
use crate::util::backup_artifact_paths;

pub const BUNDLE_VERSION: u32 = 1;

const DATABASE_ARTIFACT: usize = 0;
const SHA256_ARTIFACT: usize = 1;
const METADATA_ARTIFACT: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BackupPublishStage {
    SidecarPublished,
    MainPublishedAndVerified,
}

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
        self.backup_database_with_publish_hook(path, dry_run, overwrite, |_| Ok(()))
    }

    fn backup_database_with_publish_hook<F>(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
        mut publish_hook: F,
    ) -> Result<BackupReport>
    where
        F: FnMut(BackupPublishStage) -> Result<()>,
    {
        let artifacts = backup_artifact_paths(path);
        let mut artifact_exists = [false; 3];
        for (index, artifact) in artifacts.iter().enumerate() {
            artifact_exists[index] = preflight_backup_target(
                self.path(),
                artifact,
                overwrite,
                if index == 0 {
                    "backup"
                } else {
                    "backup sidecar"
                },
            )?;
        }
        let exists = artifact_exists[0];
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

        let conn = self.lock()?;
        conn.execute_batch("CHECKPOINT")?;
        let mut publication = BackupGroupPublication::new(artifacts, overwrite);
        let (database_stage, bytes) = stage_file_copy(self.path(), path)?;
        publication.set_staged(DATABASE_ARTIFACT, database_stage);
        drop(conn);

        let mut verification = verify_backup(publication.staged_path(DATABASE_ARTIFACT)?)?;
        sync_file(publication.staged_path(DATABASE_ARTIFACT)?)?;
        // Sidecars describe the durable destination, not the private staging name.
        verification.path = path.display().to_string();
        let sidecar_bytes = backup_sidecar_bytes(path, self.path(), &verification)?;
        publication.set_staged(
            SHA256_ARTIFACT,
            stage_bytes(
                &publication.artifacts[SHA256_ARTIFACT],
                &sidecar_bytes.sha256,
            )?,
        );
        publication.set_staged(
            METADATA_ARTIFACT,
            stage_bytes(
                &publication.artifacts[METADATA_ARTIFACT],
                &sidecar_bytes.metadata,
            )?,
        );

        // Persist all three staged files and their directory entries before any
        // public name changes. The database destination is deliberately the
        // final publish: for a new (overwrite=false) group, its presence is the
        // commit marker and both sidecars are already durable at that point.
        sync_parent_directory(path)?;
        publication.preserve_overwritten_artifacts()?;
        sync_parent_directory(path)?;
        publication.publish(SHA256_ARTIFACT)?;
        sync_parent_directory(path)?;
        publish_hook(BackupPublishStage::SidecarPublished)?;
        publication.publish(METADATA_ARTIFACT)?;
        sync_parent_directory(path)?;

        publication.publish(DATABASE_ARTIFACT)?;
        sync_parent_directory(path)?;
        let final_verification = verify_backup(path)?;
        if final_verification.sha256 != verification.sha256
            || final_verification.bytes != verification.bytes
        {
            return Err(AppError::db(format!(
                "published backup '{}' does not match its verified staging file",
                path.display()
            )));
        }
        publish_hook(BackupPublishStage::MainPublishedAndVerified)?;
        publication.commit();

        Ok(BackupReport {
            success: true,
            dry_run: false,
            source,
            path: path.display().to_string(),
            overwritten: exists,
            bytes: Some(bytes),
            errors: Vec::new(),
            sha256: Some(final_verification.sha256.clone()),
            verification: Some(final_verification),
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
            let matching_document_ids = {
                let mut stmt =
                    tx.prepare("SELECT id FROM documents WHERE id = ? OR uri = ? ORDER BY id ASC")?;
                let rows = stmt.query_map(params![item.document.id, item.document.uri], |row| {
                    row.get::<_, String>(0)
                })?;
                rows.collect::<std::result::Result<Vec<_>, _>>()?
            };
            if matching_document_ids.len() > 1 {
                return Err(AppError::conflict(format!(
                    "recovery bundle document {} ({}) matches multiple existing documents by id/uri: {}",
                    item.document.id,
                    item.document.uri,
                    matching_document_ids.join(", ")
                )));
            }
            let existing_document_id = matching_document_ids.first();
            let exists = existing_document_id.is_some();
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
            if let Some(existing_document_id) = existing_document_id {
                tx.execute(
                    "DELETE FROM graph_edges WHERE source_id IN (SELECT id FROM graph_nodes WHERE document_id = ?) OR target_id IN (SELECT id FROM graph_nodes WHERE document_id = ?)",
                    params![existing_document_id, existing_document_id],
                )?;
                tx.execute(
                    "DELETE FROM graph_nodes WHERE document_id = ?",
                    params![existing_document_id],
                )?;
                tx.execute(
                    "DELETE FROM wiki_index WHERE document_id = ? OR page_id = ?",
                    params![existing_document_id, existing_document_id],
                )?;
                tx.execute(
                    "DELETE FROM chunks WHERE document_id = ?",
                    params![existing_document_id],
                )?;
                tx.execute(
                    "DELETE FROM documents WHERE id = ?",
                    params![existing_document_id],
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

/// Atomically publish one recovery artifact after fully staging and syncing it.
///
/// With `overwrite=false`, a destination that appears after preflight is left
/// untouched and reported as a conflict.
pub fn publish_recovery_artifact(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
) -> Result<bool> {
    publish_recovery_artifact_with_hook(destination, bytes, overwrite, |_| Ok(()))
}

fn publish_recovery_artifact_with_hook<F>(
    destination: &Path,
    bytes: &[u8],
    overwrite: bool,
    before_publish: F,
) -> Result<bool>
where
    F: FnOnce(&Path) -> Result<()>,
{
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "recovery artifact parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let existed = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if existed && !overwrite {
        return Err(AppError::conflict(format!(
            "recovery artifact destination '{}' already exists; set overwrite=true explicitly",
            destination.display()
        )));
    }

    let temporary = stage_bytes(destination, bytes)?;
    let result = (|| {
        before_publish(destination)?;
        publish_temporary_artifact(&temporary, destination, overwrite)?;
        if !overwrite {
            fs::remove_file(&temporary)?;
        }
        sync_parent_directory(destination)?;
        Ok(existed)
    })();
    if temporary.exists() {
        let _ = fs::remove_file(&temporary);
    }
    result
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

struct BackupSidecarBytes {
    sha256: Vec<u8>,
    metadata: Vec<u8>,
}

fn backup_sidecar_bytes(
    path: &Path,
    source: &Path,
    verification: &BackupVerification,
) -> Result<BackupSidecarBytes> {
    let name = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("backup.duckdb");
    let sidecar = BackupSidecar {
        format: "rag-duckdb-backup".into(),
        created_at: Utc::now(),
        source: source.display().to_string(),
        required_free_bytes: verification.bytes.saturating_mul(2),
        verification: verification.clone(),
    };
    Ok(BackupSidecarBytes {
        sha256: format!("{}  {}\n", verification.sha256, name).into_bytes(),
        metadata: serde_json::to_vec_pretty(&sidecar)?,
    })
}

/// Owns the staged and published members of one three-file backup group.
///
/// A normal Rust error unwinds through `Drop`, which rolls back artifacts
/// published by this attempt and restores overwrite targets from hard-link
/// snapshots (subject, unavoidably, to the filesystem accepting cleanup I/O).
/// Filesystems do not provide a transaction spanning three names, though: a
/// process/power loss between renames can still leave sidecars without a new
/// database, hidden staging/rollback links, or (when overwriting) a mixed
/// generation. Publishing the database last makes a *new* database name the
/// commit marker; callers needing the strongest crash semantics should use a
/// fresh destination (`overwrite=false`) and ignore orphan sidecars.
struct BackupGroupPublication {
    artifacts: [PathBuf; 3],
    staged: [Option<PathBuf>; 3],
    overwritten: [Option<PathBuf>; 3],
    published: [bool; 3],
    overwrite: bool,
    committed: bool,
}

impl BackupGroupPublication {
    fn new(artifacts: [PathBuf; 3], overwrite: bool) -> Self {
        Self {
            artifacts,
            staged: std::array::from_fn(|_| None),
            overwritten: std::array::from_fn(|_| None),
            published: [false; 3],
            overwrite,
            committed: false,
        }
    }

    fn set_staged(&mut self, index: usize, path: PathBuf) {
        self.staged[index] = Some(path);
    }

    fn staged_path(&self, index: usize) -> Result<&Path> {
        self.staged[index]
            .as_deref()
            .ok_or_else(|| AppError::db("backup artifact was not staged"))
    }

    fn preserve_overwritten_artifacts(&mut self) -> Result<()> {
        if !self.overwrite {
            return Ok(());
        }
        for index in 0..self.artifacts.len() {
            match fs::symlink_metadata(&self.artifacts[index]) {
                Ok(_) => {
                    self.overwritten[index] = Some(create_rollback_link(&self.artifacts[index])?);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn publish(&mut self, index: usize) -> Result<()> {
        let staged = self.staged_path(index)?.to_path_buf();
        publish_temporary_artifact(&staged, &self.artifacts[index], self.overwrite)?;
        self.published[index] = true;
        if !self.overwrite {
            // `hard_link` publishes the final name without consuming staging.
            // Mark ownership first so an unlink error rolls back both names.
            fs::remove_file(&staged)?;
        }
        self.staged[index] = None;
        Ok(())
    }

    fn commit(mut self) {
        self.committed = true;
        for path in self.overwritten.iter_mut().filter_map(Option::take) {
            let _ = fs::remove_file(path);
        }
        let _ = sync_parent_directory(&self.artifacts[DATABASE_ARTIFACT]);
    }

    fn rollback(&mut self) {
        for index in (0..self.artifacts.len()).rev() {
            if self.published[index] {
                let _ = fs::remove_file(&self.artifacts[index]);
                self.published[index] = false;
                if let Some(original) = self.overwritten[index].take() {
                    if fs::rename(&original, &self.artifacts[index]).is_err() {
                        // Keep the hard-link snapshot for manual recovery rather
                        // than deleting the last known copy of the old artifact.
                        self.overwritten[index] = Some(original);
                    }
                }
            } else if let Some(original) = self.overwritten[index].take() {
                let _ = fs::remove_file(original);
            }
        }
        for staged in self.staged.iter_mut().filter_map(Option::take) {
            let _ = fs::remove_file(staged);
        }
        let _ = sync_parent_directory(&self.artifacts[DATABASE_ARTIFACT]);
    }
}

impl Drop for BackupGroupPublication {
    fn drop(&mut self) {
        if !self.committed {
            self.rollback();
        }
    }
}

fn preflight_backup_target(
    source: &Path,
    destination: &Path,
    overwrite: bool,
    label: &str,
) -> Result<bool> {
    refuse_same_file(source, destination)?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(AppError::config(format!(
            "{label} parent directory '{}' does not exist",
            parent.display()
        )));
    }
    let metadata = match fs::symlink_metadata(destination) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(AppError::forbidden(format!(
            "{label} destination '{}' must not be a symbolic link",
            destination.display()
        )));
    }
    let exists = metadata.is_some();
    if exists && !overwrite {
        return Err(AppError::conflict(format!(
            "{label} destination '{}' already exists; set overwrite=true explicitly",
            destination.display()
        )));
    }
    Ok(exists)
}

fn stage_file_copy(source: &Path, destination: &Path) -> Result<(PathBuf, u64)> {
    let (temporary_path, mut temporary) = create_temporary_artifact(destination)?;
    let result = (|| -> Result<(PathBuf, u64)> {
        let mut input = fs::File::open(source)?;
        let bytes = std::io::copy(&mut input, &mut temporary)?;
        if let Ok(metadata) = fs::metadata(source) {
            fs::set_permissions(&temporary_path, metadata.permissions())?;
        }
        temporary.sync_all()?;
        drop(temporary);
        Ok((temporary_path.clone(), bytes))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn stage_bytes(destination: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let (temporary_path, mut temporary) = create_temporary_artifact(destination)?;
    let result = (|| -> Result<PathBuf> {
        temporary.write_all(bytes)?;
        temporary.sync_all()?;
        drop(temporary);
        Ok(temporary_path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result
}

fn sync_file(path: &Path) -> Result<()> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?
        .sync_all()?;
    Ok(())
}

fn create_rollback_link(destination: &Path) -> Result<PathBuf> {
    static NEXT_ROLLBACK_ID: AtomicU64 = AtomicU64::new(0);

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let id = NEXT_ROLLBACK_ID.fetch_add(1, Ordering::Relaxed);
        let mut rollback_name = file_name.to_os_string();
        rollback_name.push(format!(".rag-backup-rollback-{}-{id}", std::process::id()));
        let rollback_path = parent.join(rollback_name);
        match fs::hard_link(destination, &rollback_path) {
            Ok(()) => return Ok(rollback_path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::conflict(format!(
        "could not preserve overwritten backup artifact '{}'",
        destination.display()
    )))
}

#[cfg(unix)]
fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_parent_directory(_path: &Path) -> Result<()> {
    Ok(())
}

fn create_temporary_artifact(destination: &Path) -> Result<(PathBuf, fs::File)> {
    static NEXT_TEMPORARY_ID: AtomicU64 = AtomicU64::new(0);

    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    let file_name = destination
        .file_name()
        .ok_or_else(|| AppError::config("backup destination must name a file"))?;
    for _ in 0..100 {
        let id = NEXT_TEMPORARY_ID.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".rag-backup-tmp-{}-{id}", std::process::id()));
        let temporary_path = parent.join(temporary_name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)
        {
            Ok(file) => return Ok((temporary_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(AppError::conflict(format!(
        "could not allocate a temporary backup artifact beside '{}'",
        destination.display()
    )))
}

fn publish_temporary_artifact(temporary: &Path, destination: &Path, overwrite: bool) -> Result<()> {
    if overwrite {
        fs::rename(temporary, destination)?;
        return Ok(());
    }
    match fs::hard_link(temporary, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(AppError::conflict(format!(
                "backup artifact '{}' appeared while the backup was running",
                destination.display()
            )))
        }
        Err(error) => Err(error.into()),
    }
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
    use crate::models::{GraphEdge, GraphFilter, GraphNode};

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

    #[test]
    fn backup_preflights_every_sidecar_before_publishing_the_database() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("live.duckdb");
        let backup = root.path().join("backup.duckdb");
        let sha_path = PathBuf::from(format!("{}.sha256", backup.display()));
        let store = Store::open(&source).unwrap();
        fs::write(&sha_path, "keep-existing-sidecar").unwrap();

        let error = store.backup_database(&backup, false, false).unwrap_err();

        assert!(matches!(error, AppError::Conflict(_)));
        assert!(!backup.exists());
        assert_eq!(
            fs::read_to_string(sha_path).unwrap(),
            "keep-existing-sidecar"
        );
    }

    #[test]
    fn atomic_no_overwrite_publish_rejects_a_racing_destination() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("backup.duckdb.sha256");
        let (temporary_path, mut temporary) = create_temporary_artifact(&destination).unwrap();
        temporary.write_all(b"new").unwrap();
        temporary.sync_all().unwrap();
        drop(temporary);
        fs::write(&destination, "racing-writer").unwrap();

        let error = publish_temporary_artifact(&temporary_path, &destination, false).unwrap_err();

        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(fs::read_to_string(destination).unwrap(), "racing-writer");
        fs::remove_file(temporary_path).unwrap();
    }

    #[test]
    fn recovery_artifact_publish_is_atomic_and_no_clobber_under_race() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("bundle.json");

        let error = publish_recovery_artifact_with_hook(
            &destination,
            b"new bundle",
            false,
            |destination| {
                fs::write(destination, "racing writer")?;
                Ok(())
            },
        )
        .unwrap_err();

        assert!(matches!(error, AppError::Conflict(_)));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "racing writer");
        assert_no_backup_work_files(root.path());

        assert!(publish_recovery_artifact(&destination, b"replacement", true).unwrap());
        assert_eq!(fs::read(&destination).unwrap(), b"replacement");
        assert_no_backup_work_files(root.path());
    }

    #[test]
    fn recovery_artifact_publish_cleans_stage_after_injected_failure() {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("bundle.jsonl");

        let error = publish_recovery_artifact_with_hook(
            &destination,
            b"complete staged bytes",
            false,
            |_| Err(AppError::db("injected failure before publish")),
        )
        .unwrap_err();

        assert!(error.to_string().contains("injected failure"));
        assert!(!destination.exists());
        assert_no_backup_work_files(root.path());
    }

    #[test]
    fn recovery_overwrite_cross_collision_conflicts_and_rolls_back_every_item() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("live.duckdb")).unwrap();
        let original_a = recovery_document("document-a", "recovery://a", "original a");
        let original_b = recovery_document("document-b", "recovery://b", "original b");
        store.upsert_document(&original_a).unwrap();
        store.upsert_document(&original_b).unwrap();
        store
            .insert_chunks(&[
                recovery_chunk("chunk-a", "document-a", "original chunk a"),
                recovery_chunk("chunk-b", "document-b", "original chunk b"),
            ])
            .unwrap();
        for node in [
            recovery_node("node-a", "document-a", "recovery://a"),
            recovery_node("node-b", "document-b", "recovery://b"),
        ] {
            store.upsert_graph_node(&node).unwrap();
        }
        store
            .insert_graph_edges(&[GraphEdge {
                id: "edge-a-b".into(),
                source_id: "node-a".into(),
                target_id: "node-b".into(),
                rel_type: "related".into(),
                weight: 1.0,
                context: Some("preserve me".into()),
            }])
            .unwrap();

        let before_documents = serde_json::to_vec(&store.recovery_bundle().unwrap().documents)
            .expect("serialize documents before import");
        let before_graph = serde_json::to_vec(
            &store
                .get_graph_view(GraphFilter {
                    max_nodes: Some(100),
                    ..GraphFilter::default()
                })
                .unwrap(),
        )
        .expect("serialize graph before import");
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            documents: vec![
                BundleDocument {
                    document: recovery_document("new-document", "recovery://new", "must roll back"),
                    chunks: vec![recovery_chunk(
                        "new-chunk",
                        "new-document",
                        "must roll back",
                    )],
                },
                BundleDocument {
                    document: recovery_document("document-a", "recovery://b", "cross collision"),
                    chunks: vec![recovery_chunk(
                        "collision-chunk",
                        "document-a",
                        "cross collision",
                    )],
                },
            ],
        };

        let error = store
            .import_recovery_bundle(
                &bundle,
                ConflictPolicy::Overwrite,
                false,
                Path::new("fixture.json"),
                "json",
            )
            .unwrap_err();

        assert!(matches!(error, AppError::Conflict(_)));
        assert!(error.to_string().contains("document-a"));
        assert!(error.to_string().contains("document-b"));
        assert!(store.get_document("new-document").unwrap().is_none());
        let after_documents = serde_json::to_vec(&store.recovery_bundle().unwrap().documents)
            .expect("serialize documents after import");
        let after_graph = serde_json::to_vec(
            &store
                .get_graph_view(GraphFilter {
                    max_nodes: Some(100),
                    ..GraphFilter::default()
                })
                .unwrap(),
        )
        .expect("serialize graph after import");
        assert_eq!(after_documents, before_documents);
        assert_eq!(after_graph, before_graph);
    }

    #[test]
    fn recovery_import_preserves_zero_and_single_match_overwrite_semantics() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(&root.path().join("live.duckdb")).unwrap();
        store
            .upsert_document(&recovery_document(
                "old-document",
                "recovery://replace",
                "old body",
            ))
            .unwrap();
        store
            .insert_chunks(&[recovery_chunk("old-chunk", "old-document", "old chunk")])
            .unwrap();
        store
            .upsert_graph_node(&recovery_node(
                "old-node",
                "old-document",
                "recovery://replace",
            ))
            .unwrap();
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            documents: vec![
                BundleDocument {
                    document: recovery_document(
                        "replacement-document",
                        "recovery://replace",
                        "replacement body",
                    ),
                    chunks: vec![recovery_chunk(
                        "replacement-chunk",
                        "replacement-document",
                        "replacement chunk",
                    )],
                },
                BundleDocument {
                    document: recovery_document(
                        "inserted-document",
                        "recovery://inserted",
                        "inserted body",
                    ),
                    chunks: vec![recovery_chunk(
                        "inserted-chunk",
                        "inserted-document",
                        "inserted chunk",
                    )],
                },
            ],
        };

        let report = store
            .import_recovery_bundle(
                &bundle,
                ConflictPolicy::Overwrite,
                false,
                Path::new("fixture.json"),
                "json",
            )
            .unwrap();

        assert!(report.success);
        assert_eq!(report.documents_overwritten, 1);
        assert_eq!(report.documents_inserted, 1);
        assert_eq!(report.conflicts, 1);
        assert_eq!(report.chunks_inserted, 2);
        assert!(store.get_document("old-document").unwrap().is_none());
        assert!(store.find_node_by_id("old-node").unwrap().is_none());
        assert_eq!(
            store
                .get_document("replacement-document")
                .unwrap()
                .unwrap()
                .content,
            "replacement body"
        );
        assert_eq!(
            store
                .list_chunks_for_document("replacement-document")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .get_document("inserted-document")
                .unwrap()
                .unwrap()
                .content,
            "inserted body"
        );
    }

    fn recovery_document(id: &str, uri: &str, content: &str) -> Document {
        Document {
            id: id.into(),
            uri: uri.into(),
            title: id.into(),
            content: content.into(),
            ..Document::default()
        }
    }

    fn recovery_chunk(id: &str, document_id: &str, content: &str) -> Chunk {
        Chunk {
            id: id.into(),
            document_id: document_id.into(),
            chunk_index: 0,
            content: content.into(),
            embedding: vec![1.0, 0.0],
            char_start: 0,
            char_end: content.chars().count() as i32,
            metadata_json: "{}".into(),
        }
    }

    fn recovery_node(id: &str, document_id: &str, uri: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: "document".into(),
            label: id.into(),
            document_id: Some(document_id.into()),
            uri: Some(uri.into()),
            resolved: true,
            metadata_json: "{}".into(),
        }
    }

    #[test]
    fn backup_failure_after_sidecar_publish_removes_group_and_retry_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("live.duckdb");
        let backup = root.path().join("backup.duckdb");
        let artifacts = backup_artifact_paths(&backup);
        let store = Store::open(&source).unwrap();

        let error = store
            .backup_database_with_publish_hook(&backup, false, false, |stage| match stage {
                BackupPublishStage::SidecarPublished => {
                    assert!(!artifacts[DATABASE_ARTIFACT].exists());
                    assert!(artifacts[SHA256_ARTIFACT].is_file());
                    assert!(!artifacts[METADATA_ARTIFACT].exists());
                    Err(AppError::db("injected failure after sidecar publish"))
                }
                BackupPublishStage::MainPublishedAndVerified => Ok(()),
            })
            .unwrap_err();

        assert!(error.to_string().contains("injected failure"));
        assert_backup_group_absent(&backup);
        assert_no_backup_work_files(root.path());

        let report = store.backup_database(&backup, false, false).unwrap();
        assert!(report.success);
        assert_complete_backup_group(&backup);
    }

    #[test]
    fn backup_failure_after_main_verification_removes_group_and_retry_succeeds() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("live.duckdb");
        let backup = root.path().join("backup.duckdb");
        let artifacts = backup_artifact_paths(&backup);
        let store = Store::open(&source).unwrap();

        let error = store
            .backup_database_with_publish_hook(&backup, false, false, |stage| match stage {
                BackupPublishStage::SidecarPublished => Ok(()),
                BackupPublishStage::MainPublishedAndVerified => {
                    assert!(artifacts.iter().all(|artifact| artifact.is_file()));
                    Err(AppError::db("injected failure after main verification"))
                }
            })
            .unwrap_err();

        assert!(error.to_string().contains("injected failure"));
        assert_backup_group_absent(&backup);
        assert_no_backup_work_files(root.path());

        let report = store.backup_database(&backup, false, false).unwrap();
        assert!(report.success);
        assert_complete_backup_group(&backup);
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

    #[cfg(unix)]
    #[test]
    fn backup_refuses_a_sidecar_alias_to_the_live_database() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("live.duckdb");
        let backup = root.path().join("backup.duckdb");
        let metadata_path = PathBuf::from(format!("{}.metadata.json", backup.display()));
        let store = Store::open(&source).unwrap();
        fs::hard_link(&source, &metadata_path).unwrap();
        let original_len = fs::metadata(&source).unwrap().len();

        let error = store.backup_database(&backup, false, true).unwrap_err();

        assert!(matches!(error, AppError::Forbidden(_)));
        assert!(!backup.exists());
        assert_eq!(fs::metadata(source).unwrap().len(), original_len);
    }

    fn assert_backup_group_absent(path: &Path) {
        assert!(
            backup_artifact_paths(path)
                .iter()
                .all(|artifact| !artifact.exists()),
            "failed backup must not leave a visible partial group"
        );
    }

    fn assert_complete_backup_group(path: &Path) {
        assert!(
            backup_artifact_paths(path)
                .iter()
                .all(|artifact| artifact.is_file()),
            "successful retry must publish the complete backup group"
        );
        assert!(verify_backup(path).unwrap().ok);
    }

    fn assert_no_backup_work_files(dir: &Path) {
        let leaked = fs::read_dir(dir)
            .unwrap()
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".rag-backup-"))
            .collect::<Vec<_>>();
        assert!(leaked.is_empty(), "leaked backup work files: {leaked:?}");
    }
}
