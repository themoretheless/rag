//! Corpus embedding identity (manifest) reads, writes and compatibility checks.

use super::*;

impl Store {
    /// Load the embedding corpus fingerprint (typically id `"default"`).
    pub fn get_embedding_manifest(&self) -> Result<Option<EmbeddingManifest>> {
        self.get_embedding_manifest_by_id("default")
    }

    /// Load embedding manifest by primary key.
    pub fn get_embedding_manifest_by_id(&self, id: &str) -> Result<Option<EmbeddingManifest>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, provider, model, dims, base_url, content_fingerprint,
                   CAST(updated_at AS VARCHAR)
            FROM embedding_manifest
            WHERE id = ?
            "#,
        )?;

        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(rows::embedding_manifest(row)?)),
            None => Ok(None),
        }
    }

    /// Insert or replace the embedding corpus fingerprint.
    pub fn set_embedding_manifest(&self, manifest: &EmbeddingManifest) -> Result<()> {
        let conn = self.lock()?;
        set_embedding_manifest_locked(&conn, manifest)
    }

    /// Record the default embedding manifest from config if none is stored yet.
    ///
    /// Does **not** overwrite an existing row (preserves corpus fingerprint so
    /// embedding identity drift can be detected). Call on server start and before
    /// first ingest.
    pub fn ensure_embedding_manifest(&self, config: &Config) -> Result<EmbeddingManifest> {
        if let Some(existing) = self.get_embedding_manifest()? {
            return Ok(existing);
        }

        // Never infer the identity of already-persisted vectors. A missing
        // manifest on a non-empty corpus is an unknown legacy identity and must
        // be migrated explicitly through a complete reembed_all.
        let conn = self.lock()?;
        let manifest_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM embedding_manifest WHERE id = 'default')",
            [],
            |row| row.get(0),
        )?;
        if manifest_exists {
            drop(conn);
            return self.get_embedding_manifest()?.ok_or_else(|| {
                AppError::db("embedding manifest disappeared during initialization")
            });
        }
        let chunk_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        if chunk_count > 0 {
            return Err(AppError::embeddings(format!(
                "embedding manifest is missing for a non-empty corpus ({chunk_count} chunks); run a complete uncapped reembed_all to establish a verified corpus identity"
            )));
        }
        let manifest = embedding_manifest_from_config(config);
        set_embedding_manifest_locked(&conn, &manifest)?;
        tracing::info!(
            provider = %manifest.provider,
            model = %manifest.model,
            dims = manifest.dims,
            "recorded embedding_manifest"
        );
        Ok(manifest)
    }

    /// Overwrite the default embedding manifest to match live config (e.g. after reembed).
    pub fn write_embedding_manifest_from_config(
        &self,
        config: &Config,
    ) -> Result<EmbeddingManifest> {
        let manifest = embedding_manifest_from_config(config);
        self.set_embedding_manifest(&manifest)?;
        Ok(manifest)
    }

    /// True when the persisted corpus embedding identity matches runtime config.
    ///
    /// A missing manifest is not a match. Call [`Self::ensure_embedding_manifest`]
    /// first only when initializing a corpus whose current identity is already
    /// known to be the configured one (the normal server/ingest startup path).
    pub fn embedding_manifest_matches_config(&self, config: &Config) -> Result<bool> {
        Ok(self
            .get_embedding_manifest()?
            .as_ref()
            .is_some_and(|manifest| embedding_manifest_matches_config(manifest, config)))
    }

    /// Refuse vector-producing or vector-consuming work on identity mismatch.
    ///
    /// A complete, uncapped `reembed_all` is the only supported migration path.
    /// Single-document refreshes must never publish a new corpus identity while
    /// other chunks may still contain vectors from the old model or endpoint.
    pub fn require_embedding_manifest_match(&self, config: &Config) -> Result<()> {
        let Some(manifest) = self.get_embedding_manifest()? else {
            return Err(AppError::embeddings(
                "embedding manifest is missing; initialize an empty corpus or run a complete uncapped reembed_all before vector operations",
            ));
        };
        if embedding_manifest_matches_config(&manifest, config) {
            return Ok(());
        }

        let configured = embedding_manifest_from_config(config);
        let comparison = if manifest
            .content_fingerprint
            .as_deref()
            .is_some_and(|fingerprint| {
                fingerprint.starts_with(EMBEDDING_MIGRATION_FINGERPRINT_PREFIX)
            }) {
            format!(
                "incomplete corpus migration marker={}",
                manifest.content_fingerprint.as_deref().unwrap_or_default()
            )
        } else if manifest.content_fingerprint.is_some() {
            format!(
                "stored fingerprint={}, configured fingerprint={}",
                manifest.content_fingerprint.as_deref().unwrap_or_default(),
                configured
                    .content_fingerprint
                    .as_deref()
                    .unwrap_or_default()
            )
        } else {
            "legacy provider/model/dims comparison".to_string()
        };
        Err(AppError::embeddings(format!(
            "embedding manifest mismatch ({comparison}): stored provider='{}', model='{}', dims={}; configured provider='{}', model='{}', dims={}. Run a complete uncapped reembed_all to migrate the corpus; reembed_document cannot migrate embedding identity",
            manifest.provider,
            manifest.model,
            manifest.dims,
            configured.provider,
            configured.model,
            configured.dims,
        )))
    }

    /// Legacy dimensions-only compatibility probe.
    ///
    /// Production vector paths use [`Self::require_embedding_manifest_match`]
    /// so equal-dimensional vectors from another model cannot be mixed.
    pub fn require_embedding_dims_match(&self, config_dims: usize) -> Result<()> {
        let Some(manifest) = self.get_embedding_manifest()? else {
            return Ok(());
        };
        let expected = config_dims as i32;
        if manifest.dims == expected {
            return Ok(());
        }
        Err(AppError::embeddings(format!(
            "embedding dimension mismatch: corpus has dims={} (provider={}, model={}), \
             config has dims={}. Run a complete uncapped reembed_all after changing \
             RAG_EMBEDDING_DIMS (or embedding identity), then verify with get_embedding_manifest.",
            manifest.dims, manifest.provider, manifest.model, expected
        )))
    }
}
