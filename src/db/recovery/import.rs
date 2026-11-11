//! Store-side export/import/backup operations and embedding-manifest checks.

use super::*;

pub(super) fn embedding_manifests_match(
    left: &EmbeddingManifest,
    right: &EmbeddingManifest,
) -> bool {
    if left.id != "default"
        || right.id != "default"
        || left.provider != right.provider
        || left.model != right.model
        || left.dims != right.dims
        || left.base_url != right.base_url
    {
        return false;
    }
    match (
        left.content_fingerprint.as_deref(),
        right.content_fingerprint.as_deref(),
    ) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

/// Rebuild the canonical embedding identity fingerprint from serialized fields.
///
/// This deliberately mirrors `store::embedding_config_fingerprint`: recovery
/// manifests are untrusted input, so their persisted fingerprint must describe
/// the provider/model/dimensions/base URL carried alongside it.
pub(super) fn canonical_embedding_manifest_fingerprint(
    manifest: &EmbeddingManifest,
) -> Result<String> {
    if manifest.id != "default" {
        return Err(AppError::embeddings(format!(
            "embedding manifest id must be 'default' (got '{}')",
            manifest.id
        )));
    }
    if manifest.dims <= 0 {
        return Err(AppError::embeddings(format!(
            "embedding manifest has invalid dims={}",
            manifest.dims
        )));
    }
    let base_url = manifest.base_url.as_deref().ok_or_else(|| {
        AppError::embeddings(
            "embedding manifest has no base_url; its canonical fingerprint cannot be verified",
        )
    })?;
    Ok(embedding_identity_fingerprint(
        &manifest.provider,
        &manifest.model,
        manifest.dims,
        base_url,
    ))
}

pub(super) fn validate_embedding_manifest(
    manifest: &EmbeddingManifest,
    context: &str,
) -> Result<()> {
    if manifest
        .content_fingerprint
        .as_deref()
        .is_some_and(|fingerprint| fingerprint.starts_with("migration-incomplete:"))
    {
        return Err(AppError::embeddings(format!(
            "{context} embedding manifest records an incomplete embedding migration"
        )));
    }
    let expected = canonical_embedding_manifest_fingerprint(manifest)?;
    let actual = manifest.content_fingerprint.as_deref().ok_or_else(|| {
        AppError::embeddings(format!(
            "{context} embedding manifest has no canonical content_fingerprint"
        ))
    })?;
    if actual != expected {
        return Err(AppError::embeddings(format!(
            "{context} embedding manifest content_fingerprint does not match its provider/model/dims/base_url identity"
        )));
    }
    Ok(())
}

pub(super) fn validate_bundle_structure(bundle: &RecoveryBundle) -> Result<()> {
    for (document_index, item) in bundle.documents.iter().enumerate() {
        let document_id = item.document.id.trim();
        if document_id.is_empty() {
            return Err(AppError::config(format!(
                "recovery bundle documents[{document_index}] has an empty document id"
            )));
        }
        if item.document.uri.trim().is_empty() {
            return Err(AppError::config(format!(
                "recovery bundle document '{document_id}' has an empty uri"
            )));
        }
        for (chunk_index, chunk) in item.chunks.iter().enumerate() {
            if chunk.id.trim().is_empty() {
                return Err(AppError::config(format!(
                    "recovery bundle document '{document_id}' chunk[{chunk_index}] has an empty id"
                )));
            }
            if chunk.document_id != item.document.id {
                return Err(AppError::config(format!(
                    "recovery bundle chunk '{}' declares document_id '{}', expected parent document id '{}'",
                    chunk.id, chunk.document_id, item.document.id
                )));
            }
        }
    }
    Ok(())
}

/// Reason recorded in `chunk_embedding_quarantine` for a chunk that arrived
/// through a recovery bundle without a vector.
const IMPORTED_QUARANTINE_REASON: &str = "recovery_bundle_without_embedding";

/// Check every chunk vector against the bundle manifest. An empty vector is
/// the bundle's mark for a quarantined chunk (NULL embedding in the source,
/// see `chunk_embedding_quarantine`): it is exported as `[]`, accepted here
/// and recorded as quarantined on import with its document flagged for
/// re-embedding.
pub(super) fn validate_bundle_embeddings(
    bundle: &RecoveryBundle,
) -> Result<Option<&EmbeddingManifest>> {
    let bundle_chunk_count = bundle
        .documents
        .iter()
        .map(|item| item.chunks.len())
        .sum::<usize>();
    if let Some(manifest) = bundle.embedding_manifest.as_ref() {
        validate_embedding_manifest(manifest, "recovery bundle")?;
    }
    if bundle_chunk_count == 0 {
        return Ok(None);
    }
    let manifest = bundle.embedding_manifest.as_ref().ok_or_else(|| {
        AppError::embeddings(
            "recovery bundle contains chunk embeddings but has no embedding_manifest",
        )
    })?;
    let expected_dims = usize::try_from(manifest.dims)
        .ok()
        .filter(|dims| *dims > 0)
        .ok_or_else(|| {
            AppError::embeddings(format!(
                "recovery bundle embedding manifest has invalid dims={}",
                manifest.dims
            ))
        })?;
    if let Some(chunk) = bundle
        .documents
        .iter()
        .flat_map(|item| &item.chunks)
        .find(|chunk| !chunk.embedding.is_empty() && chunk.embedding.len() != expected_dims)
    {
        return Err(AppError::embeddings(format!(
            "recovery bundle chunk {} has dims={}, expected {} from embedding_manifest",
            chunk.id,
            chunk.embedding.len(),
            expected_dims
        )));
    }
    Ok(Some(manifest))
}

/// Backup, portable export/import and vault export.
///
/// A repository over the shared Store connection; get one from [`Store::recovery`].
#[derive(Clone, Copy)]
pub struct Recovery<'a> {
    pub(in crate::db) store: &'a Store,
}

impl Recovery<'_> {
    /// Refuse a portable export before loading all document and chunk rows.
    pub fn portable_recovery_preflight(&self) -> Result<PortableBundlePreflight> {
        let conn = self.store.lock()?;
        portable_store_preflight_locked(&conn)
    }

    /// Back up the database to `path`, optionally as a dry run or overwriting.
    pub fn backup_database(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
    ) -> Result<BackupReport> {
        self.backup_database_with_publish_hook(path, dry_run, overwrite, |_| Ok(()))
    }

    pub(super) fn backup_database_with_publish_hook<F>(
        &self,
        path: &Path,
        dry_run: bool,
        overwrite: bool,
        mut publish_hook: F,
    ) -> Result<BackupReport>
    where
        F: FnMut(BackupPublishStage) -> Result<()>,
    {
        // A backup is a three-file publication. Keep every generation for the
        // same destination serialized from preflight through final verification
        // so concurrent overwrite requests cannot interleave their sidecars.
        let destination_lock = backup_publication_lock(path);
        let _destination_guard = destination_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let artifacts = backup_artifact_paths(path);
        let mut artifact_exists = [false; 3];
        for (index, artifact) in artifacts.iter().enumerate() {
            artifact_exists[index] = preflight_backup_target(
                self.store.path(),
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
        let source = self.store.path().display().to_string();
        if dry_run {
            return Ok(BackupReport {
                success: true,
                dry_run: true,
                source,
                path: path.display().to_string(),
                overwritten: exists,
                bytes: self.store.maintenance().db_file_size_bytes(),
                errors: Vec::new(),
                sha256: None,
                verification: None,
            });
        }

        let snapshot_conn = {
            let conn = self.store.lock()?;
            conn.execute_batch("CHECKPOINT")?;
            conn.try_clone()?
        };
        let mut publication = BackupGroupPublication::new(artifacts, overwrite);
        let (database_stage, bytes) =
            stage_database_snapshot(snapshot_conn, self.store.path(), path, || {
                publish_hook(BackupPublishStage::SnapshotConnectionReady)
            })?;
        publication.set_staged(DATABASE_ARTIFACT, database_stage);

        let mut verification = verify_backup(publication.staged_path(DATABASE_ARTIFACT)?)?;
        if !verification.ok {
            return Err(AppError::db(format!(
                "staged backup '{}' failed relational or embedding-contract verification",
                path.display()
            )));
        }
        sync_file(publication.staged_path(DATABASE_ARTIFACT)?)?;
        // Sidecars describe the durable destination, not the private staging name.
        verification.path = path.display().to_string();
        let sidecar_bytes = backup_sidecar_bytes(path, self.store.path(), &verification)?;
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
        let final_verification = verify_published_backup_group(path, self.store.path())?;
        if !backup_verifications_match(&final_verification, &verification)? {
            return Err(AppError::db(format!(
                "published backup group '{}' does not match its verified staging generation",
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

    /// Export all documents and related rows as a portable recovery bundle.
    pub fn recovery_bundle(&self) -> Result<RecoveryBundle> {
        let conn = self.store.lock()?;
        portable_store_preflight_locked(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, uri, title, content, metadata_json, content_hash, wing, room, \
            source_file, layer, kind, CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR), \
            COALESCE(status, 'active'), COALESCE(pinned, false), COALESCE(boost, 1.0), \
            COALESCE(revision, 1) FROM documents ORDER BY id",
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

        // All chunks in one ordered scan, grouped by document in Rust (was
        // one prepare + query per document).
        let mut chunks_by_document: std::collections::HashMap<String, Vec<Chunk>> =
            std::collections::HashMap::with_capacity(documents.len());
        {
            let mut chunks_stmt = conn.prepare(
                "SELECT id, document_id, chunk_index, content, embedding, char_start, \
                char_end, COALESCE(metadata_json, '{}') FROM chunks ORDER BY document_id, \
                chunk_index",
            )?;
            let mut rows = chunks_stmt.query([])?;
            crate::db::rows::visit_rows(&mut rows, |row| {
                let embedding = crate::db::rows::embedding(row, 4).map_err(|error| {
                    AppError::db(format!(
                        "invalid embedding for chunk {} in recovery export: {error}",
                        row.get::<_, String>(0).unwrap_or_else(|_| "unknown".into())
                    ))
                })?;
                let chunk = Chunk {
                    id: row.get(0)?,
                    document_id: row.get(1)?,
                    chunk_index: row.get(2)?,
                    content: row.get(3)?,
                    embedding,
                    char_start: row.get(5)?,
                    char_end: row.get(6)?,
                    metadata_json: row.get(7)?,
                };
                chunks_by_document
                    .entry(chunk.document_id.clone())
                    .or_default()
                    .push(chunk);
                Ok(())
            })?;
        }
        let bundled: Vec<BundleDocument> = documents
            .into_iter()
            .map(|document| {
                let chunks = chunks_by_document.remove(&document.id).unwrap_or_default();
                BundleDocument { document, chunks }
            })
            .collect();
        let embedding_manifest = {
            let mut manifest_stmt = conn.prepare(
                r#"
                SELECT id, provider, model, dims, base_url, content_fingerprint,
                       CAST(updated_at AS VARCHAR)
                FROM embedding_manifest
                WHERE id = 'default'
                "#,
            )?;
            let mut rows = manifest_stmt.query([])?;
            match rows.next()? {
                Some(row) => Some(crate::db::rows::embedding_manifest(row)?),
                None => None,
            }
        };
        let bundle = RecoveryBundle {
            format: "rag-recovery-bundle".into(),
            version: BUNDLE_VERSION,
            exported_at: Utc::now(),
            embedding_manifest,
            documents: bundled,
        };
        validate_bundle_structure(&bundle)?;
        validate_bundle_embeddings(&bundle)?;
        preflight_recovery_bundle(&bundle)?;
        Ok(bundle)
    }

    /// Import a recovery bundle, resolving conflicts with `policy`.
    pub fn import_recovery_bundle(
        &self,
        bundle: &RecoveryBundle,
        policy: ConflictPolicy,
        dry_run: bool,
        path: &Path,
        format: &str,
    ) -> Result<BundleImportReport> {
        preflight_recovery_bundle(bundle)?;
        if bundle.format != "rag-recovery-bundle" || bundle.version != BUNDLE_VERSION {
            return Err(AppError::config(format!(
                "unsupported recovery bundle format/version: {}/{}",
                bundle.format, bundle.version
            )));
        }
        validate_bundle_structure(bundle)?;
        let mut report = BundleImportReport {
            success: true,
            dry_run,
            durable_mutation_committed: false,
            legacy_bundle_version: None,
            legacy_reembed_requested: false,
            embeddings_reembed_planned: 0,
            embeddings_reembedded: 0,
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
            chunks_quarantined: 0,
            conflicts: 0,
            errors: Vec::new(),
        };
        let bundle_manifest = validate_bundle_embeddings(bundle)?;
        let mut conn = self.store.lock()?;
        let tx = conn.transaction()?;
        if let Some(bundle_manifest) = bundle_manifest {
            let target_chunk_count: i64 =
                tx.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
            let target_manifest = {
                let mut stmt = tx.prepare(
                    r#"
                    SELECT id, provider, model, dims, base_url, content_fingerprint,
                           CAST(updated_at AS VARCHAR)
                    FROM embedding_manifest
                    WHERE id = 'default'
                    "#,
                )?;
                let mut rows = stmt.query([])?;
                match rows.next()? {
                    Some(row) => Some(crate::db::rows::embedding_manifest(row)?),
                    None => None,
                }
            };
            match target_manifest.as_ref() {
                Some(target) => {
                    validate_embedding_manifest(target, "target corpus")?;
                    if !embedding_manifests_match(target, bundle_manifest) {
                        return Err(AppError::embeddings(format!(
                            "recovery bundle embedding identity does not match target corpus \
                            (bundle provider='{}', model='{}', dims={}; target provider='{}', \
                            model='{}', dims={})",
                            bundle_manifest.provider,
                            bundle_manifest.model,
                            bundle_manifest.dims,
                            target.provider,
                            target.model,
                            target.dims,
                        )));
                    }
                }
                None if target_chunk_count > 0 => {
                    return Err(AppError::embeddings(format!(
                        "target corpus has {target_chunk_count} chunks but no embedding_manifest; run a complete uncapped reembed_all before importing vectors"
                    )));
                }
                None => set_embedding_manifest_locked(&tx, bundle_manifest)?,
            }
        }
        let mut fts_marked_dirty = false;
        let mut fts_full_rebuild_marked = false;
        // Two equality lookups prepared once (an `id = ? OR uri = ?`
        // predicate can use neither index and was re-prepared per item).
        let mut by_id = tx.prepare("SELECT id FROM documents WHERE id = ?")?;
        let mut by_uri = tx.prepare("SELECT id FROM documents WHERE uri = ?")?;
        for item in &bundle.documents {
            let matching_document_ids = {
                let mut ids: Vec<String> = by_id
                    .query_map(params![item.document.id], |row| row.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                ids.extend(
                    by_uri
                        .query_map(params![item.document.uri], |row| row.get::<_, String>(0))?
                        .collect::<std::result::Result<Vec<_>, _>>()?,
                );
                ids.sort();
                ids.dedup();
                ids
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
            let quarantined = item
                .chunks
                .iter()
                .filter(|chunk| chunk.embedding.is_empty())
                .count() as u64;
            report.chunks_inserted += item.chunks.len() as u64 - quarantined;
            report.chunks_quarantined += quarantined;
            if let Some(existing_document_id) = existing_document_id {
                crate::db::store::delete_document_locked(&tx, existing_document_id)?;
                fts_marked_dirty = true;
            } else if !fts_marked_dirty && !item.chunks.is_empty() {
                crate::db::fts::mark_fts_dirty(&tx)?;
                fts_marked_dirty = true;
            }
            if !item.chunks.is_empty() && !fts_full_rebuild_marked {
                // Bundle chunk ids may equal ids already indexed with other
                // text; an id-diff patch would miss that.
                crate::db::fts::mark_fts_needs_rebuild(&tx)?;
                fts_full_rebuild_marked = true;
            }
            let d = &item.document;
            crate::db::store::insert_document_row_locked(&tx, d, d.revision)?;
            // Restore keeps the bundle's revision and timestamps (no CAS bump),
            // but chunk rows go through the same writer as every other path.
            let chunks: Vec<Chunk> = item
                .chunks
                .iter()
                .filter(|chunk| !chunk.embedding.is_empty())
                .map(|chunk| Chunk {
                    document_id: d.id.clone(),
                    ..chunk.clone()
                })
                .collect();
            let embedding_json = crate::db::store::embedding_json(&chunks)?;
            crate::db::store::insert_chunks_locked(&tx, &chunks, &embedding_json)?;
            restore_quarantined_chunks_locked(&tx, &d.id, &item.chunks)?;
        }
        if report.success && !dry_run {
            tx.commit()?;
            report.durable_mutation_committed =
                report.documents_inserted > 0 || report.documents_overwritten > 0;
        }
        Ok(report)
    }
}

/// Restore the quarantine of chunks the bundle marked with an empty vector.
/// A fresh store's `chunks.embedding` is `NOT NULL`, so such a chunk is not
/// inserted; it gets a `chunk_embedding_quarantine` row and its document's
/// `content_hash` is cleared, so the next sync re-chunks and re-embeds it.
fn restore_quarantined_chunks_locked(
    conn: &duckdb::Connection,
    document_id: &str,
    chunks: &[Chunk],
) -> Result<()> {
    let mut any = false;
    for chunk in chunks.iter().filter(|chunk| chunk.embedding.is_empty()) {
        any = true;
        conn.execute(
            "INSERT OR REPLACE INTO chunk_embedding_quarantine \
               (chunk_id, document_id, raw_embedding_json, reason, quarantined_at) \
             VALUES (?, ?, NULL, ?, CURRENT_TIMESTAMP)",
            duckdb::params![chunk.id, document_id, IMPORTED_QUARANTINE_REASON],
        )?;
    }
    if any {
        conn.execute(
            "UPDATE documents SET content_hash = NULL WHERE id = ?",
            duckdb::params![document_id],
        )?;
    }
    Ok(())
}
