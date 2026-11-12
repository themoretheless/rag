//! Corpus embedding identity (manifest) reads, writes and compatibility checks.

use super::*;

impl Store {
    /// Load the embedding corpus fingerprint (typically id `"default"`).
    pub fn get_embedding_manifest(&self) -> Result<Option<EmbeddingManifest>> {
        self.get_embedding_manifest_by_id("default")
    }

    /// Load embedding manifest by primary key.
    pub fn get_embedding_manifest_by_id(&self, id: &str) -> Result<Option<EmbeddingManifest>> {
        let r = self.read()?;
        get_embedding_manifest_locked(&r, id)
    }

    /// Insert or replace the embedding corpus fingerprint.
    pub fn set_embedding_manifest(&self, manifest: &EmbeddingManifest) -> Result<()> {
        let mut w = self.write()?;
        set_embedding_manifest_locked(&mut w, manifest)
    }

    /// Record the default embedding manifest from config if none is stored yet.
    pub fn ensure_embedding_manifest(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<EmbeddingManifest> {
        if let Some(existing) = self.get_embedding_manifest()? {
            return Ok(existing);
        }

        let mut w = self.write()?;
        if let Some(existing) = get_embedding_manifest_locked(&w, "default")? {
            return Ok(existing);
        }
        let chunk_count = w.count_rows("chunks", None)?;
        if chunk_count > 0 {
            return Err(AppError::embeddings(format!(
                "embedding manifest is missing for a non-empty corpus ({chunk_count} chunks); \
                run a complete uncapped reembed_all to establish a verified corpus identity"
            )));
        }
        let manifest = embedding_manifest_from_config(config);
        set_embedding_manifest_locked(&mut w, &manifest)?;
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
        config: &impl EmbeddingIdentity,
    ) -> Result<EmbeddingManifest> {
        let manifest = embedding_manifest_from_config(config);
        self.set_embedding_manifest(&manifest)?;
        Ok(manifest)
    }

    /// True when the persisted corpus embedding identity matches runtime config.
    pub fn embedding_manifest_matches_config(
        &self,
        config: &impl EmbeddingIdentity,
    ) -> Result<bool> {
        Ok(self
            .get_embedding_manifest()?
            .as_ref()
            .is_some_and(|manifest| embedding_manifest_matches_config(manifest, config)))
    }

    /// Refuse vector-producing or vector-consuming work on identity mismatch.
    pub fn require_embedding_manifest_match(&self, config: &impl EmbeddingIdentity) -> Result<()> {
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
            "embedding manifest mismatch ({comparison}): stored provider='{}', model='{}', \
            dims={}; configured provider='{}', model='{}', dims={}. Run a complete uncapped \
            reembed_all to migrate the corpus; reembed_document cannot migrate embedding \
            identity",
            manifest.provider,
            manifest.model,
            manifest.dims,
            configured.provider,
            configured.model,
            configured.dims,
        )))
    }

    /// Legacy dimensions-only compatibility probe.
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

/// Build a default-id [`EmbeddingManifest`] snapshot from runtime config.
pub fn embedding_manifest_from_config(config: &impl EmbeddingIdentity) -> EmbeddingManifest {
    let provider = config.embedding_provider_name().to_string();
    let fingerprint = embedding_config_fingerprint(config);
    EmbeddingManifest {
        id: "default".into(),
        provider,
        model: config.embedding_model_name().to_string(),
        dims: config.embedding_dimensions() as i32,
        base_url: Some(config.embedding_endpoint().to_string()),
        content_fingerprint: Some(fingerprint),
        updated_at: Utc::now(),
    }
}

/// Build a durable fail-closed marker for an in-progress corpus migration.
pub(crate) fn embedding_migration_manifest(config: &impl EmbeddingIdentity) -> EmbeddingManifest {
    let mut marker = embedding_manifest_from_config(config);
    let target_fingerprint = marker
        .content_fingerprint
        .as_deref()
        .unwrap_or_default()
        .to_string();
    marker.content_fingerprint = Some(format!(
        "{EMBEDDING_MIGRATION_FINGERPRINT_PREFIX}{}",
        target_fingerprint
    ));
    marker.updated_at = Utc::now();
    marker
}

/// Canonical corpus identity predicate.
pub fn embedding_manifest_matches_config(
    manifest: &EmbeddingManifest,
    config: &impl EmbeddingIdentity,
) -> bool {
    match manifest.content_fingerprint.as_deref() {
        Some(fingerprint) => {
            manifest.id == "default"
                && manifest.provider == config.embedding_provider_name()
                && manifest.model == config.embedding_model_name()
                && manifest.dims == config.embedding_dimensions() as i32
                && manifest.base_url.as_deref() == Some(config.embedding_endpoint())
                && fingerprint == embedding_config_fingerprint(config)
        }
        None => false,
    }
}

pub(super) fn embedding_config_fingerprint(config: &impl EmbeddingIdentity) -> String {
    embedding_identity_fingerprint(
        config.embedding_provider_name(),
        config.embedding_model_name(),
        config.embedding_dimensions() as i32,
        config.embedding_endpoint(),
    )
}

pub(crate) fn embedding_identity_fingerprint(
    provider: &str,
    model: &str,
    dims: i32,
    base_url: &str,
) -> String {
    content_hash(&format!("{}|{}|{}|{}", provider, model, dims, base_url))
}

/// Read the manifest inside an already-owned consistent database observation.
pub(crate) fn get_embedding_manifest_locked(
    r: &impl LinReader,
    id: &str,
) -> Result<Option<EmbeddingManifest>> {
    r.get_by_id("embedding_manifest", id)?
        .as_ref()
        .map(rows::embedding_manifest)
        .transpose()
}
