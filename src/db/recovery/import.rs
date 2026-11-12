//! Store-side export/import/backup operations and embedding-manifest checks.

use super::*;

impl Store {
    /// Refuse a portable export before loading all document and chunk rows.
    pub fn portable_recovery_preflight(&self) -> Result<PortableBundlePreflight> {
        let r = self.read()?;
        portable_store_preflight_locked(&r)
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

        let disk_v2_view = {
            let r = self.read()?;
            r.disk_v2().map(|eng| eng.snapshot_view())
        };
        let mut publication = BackupGroupPublication::new(artifacts, overwrite);
        let (database_stage, bytes) = if let Some(view) = disk_v2_view {
            stage_disk_v2_backup(view, self.path(), path, || {
                publish_hook(BackupPublishStage::SnapshotConnectionReady)
            })?
        } else {
            let snapshot = {
                let mut w = self.write()?;
                w.capture_portable_snapshot()?
            };
            stage_database_snapshot(snapshot, self.path(), path, || {
                publish_hook(BackupPublishStage::SnapshotConnectionReady)
            })?
        };
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
        let final_verification = verify_published_backup_group(path, self.path())?;
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
        let r = self.read()?;
        portable_store_preflight_locked(&r)?;
        let mut documents: Vec<Document> = r.filter_map_rows("documents", |row| {
            Ok(Some(rows::document(row)?))
        })?;
        documents.sort_by(|a, b| a.id.cmp(&b.id));

        let mut chunks_by_document: std::collections::HashMap<String, Vec<Chunk>> =
            std::collections::HashMap::with_capacity(documents.len());
        r.scan_each("chunks", |row| {
            if let Some(raw_json) = cell_str(row, "embedding_json")
                && !raw_json.is_empty()
                && serde_json::from_str::<Vec<f32>>(raw_json).is_err()
            {
                return Err(AppError::db(format!(
                    "invalid embedding JSON for chunk {} in recovery export",
                    cell_str(row, "id").unwrap_or("unknown")
                )));
            }
            let chunk = rows::chunk(row)?;
            chunks_by_document
                .entry(chunk.document_id.clone())
                .or_default()
                .push(chunk);
            Ok(true)
        })?;
        for chunks in chunks_by_document.values_mut() {
            chunks.sort_by(|a, b| {
                a.chunk_index
                    .cmp(&b.chunk_index)
                    .then_with(|| a.id.cmp(&b.id))
            });
        }

        let bundled: Vec<BundleDocument> = documents
            .into_iter()
            .map(|document| {
                let chunks = chunks_by_document.remove(&document.id).unwrap_or_default();
                BundleDocument { document, chunks }
            })
            .collect();
        let embedding_manifest = r
            .get_by_id("embedding_manifest", "default")?
            .map(|row| rows::embedding_manifest(&row))
            .transpose()?;
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
            conflicts: 0,
            errors: Vec::new(),
        };
        let bundle_manifest = validate_bundle_embeddings(bundle)?;
        let mut w = self.write()?;
        let mut tx = w.begin_tx()?;
        if let Some(bundle_manifest) = bundle_manifest {
            let target_chunk_count = tx.count_rows("chunks", None)? as i64;
            let target_manifest = tx
                .get_by_id("embedding_manifest", "default")?
                .map(|row| rows::embedding_manifest(&row))
                .transpose()?;
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
                None => set_embedding_manifest_locked(&mut tx, bundle_manifest)?,
            }
        }
        let mut fts_marked_dirty = false;
        let mut fts_full_rebuild_marked = false;
        for item in &bundle.documents {
            let matching_document_ids = {
                let mut ids: Vec<String> = Vec::new();
                if let Some(row) = tx.get_by_id("documents", &item.document.id)?
                    && let Some(id) = cell_str(&row, "id")
                {
                    ids.push(id.to_string());
                }
                for row in tx.find_by_eq("documents", "uri", item.document.uri.as_str())? {
                    if let Some(id) = cell_str(&row, "id") {
                        ids.push(id.to_string());
                    }
                }
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
            report.chunks_inserted += item.chunks.len() as u64;
            if let Some(existing_document_id) = existing_document_id {
                crate::db::store::delete_document_locked(&mut tx, existing_document_id)?;
                fts_marked_dirty = true;
            } else if !fts_marked_dirty && !item.chunks.is_empty() {
                crate::db::fts::mark_fts_dirty(&mut tx)?;
                fts_marked_dirty = true;
            }
            if !item.chunks.is_empty() && !fts_full_rebuild_marked {
                // Bundle chunk ids may equal ids already indexed with other
                // text; an id-diff patch would miss that.
                crate::db::fts::mark_fts_needs_rebuild(&mut tx)?;
                fts_full_rebuild_marked = true;
            }
            let d = &item.document;
            tx.insert_one(
                "documents",
                RecBuilder::new()
                    .str("id", &d.id)
                    .str("uri", &d.uri)
                    .str("title", &d.title)
                    .str("content", &d.content)
                    .str("metadata_json", &d.metadata_json)
                    .opt_string("content_hash", &d.content_hash)
                    .opt_string("wing", &d.wing)
                    .opt_string("room", &d.room)
                    .opt_string("source_file", &d.source_file)
                    .str("layer", &d.layer)
                    .str("kind", d.kind.as_str())
                    .str("status", &d.status)
                    .bool("pinned", d.pinned)
                    .f64("boost", d.boost)
                    .i64("revision", d.revision)
                    .str("created_at", crate::util::format_db_timestamp(d.created_at))
                    .str("updated_at", crate::util::format_db_timestamp(d.updated_at))
                    .build(),
            )?;
            // Restore keeps the bundle's revision and timestamps (no CAS bump),
            // but chunk rows go through the same writer as every other path.
            let chunks: Vec<Chunk> = item
                .chunks
                .iter()
                .map(|chunk| Chunk {
                    document_id: d.id.clone(),
                    ..chunk.clone()
                })
                .collect();
            for chunk in &chunks {
                if tx.get_by_id("chunks", &chunk.id)?.is_some() {
                    return Err(AppError::conflict(format!(
                        "duplicate chunk id '{}' during recovery bundle import",
                        chunk.id
                    )));
                }
            }
            let embedding_json = crate::db::store::embedding_json(&chunks)?;
            crate::db::store::insert_chunks_locked(&mut tx, &chunks, &embedding_json)?;
        }
        if report.success && !dry_run {
            tx.commit()?;
            report.durable_mutation_committed =
                report.documents_inserted > 0 || report.documents_overwritten > 0;
        } else {
            let _ = tx.rollback();
        }
        Ok(report)
    }
}

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
        .find(|chunk| chunk.embedding.len() != expected_dims)
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
