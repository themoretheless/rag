//! Application service for document ingestion and embedding refresh.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::config::Config;
use crate::db::store::DocumentDerivedWrite;
use crate::db::{SourceManifestWrite, Store};
use crate::document_indexer::DocumentIndexer;
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::models::{Document, DocumentMetaUpdate, IngestResult, OpsLogEntry};
use crate::util::content_hash;

#[derive(Debug, Clone)]
pub struct IngestCommand {
    pub text: String,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub metadata_json: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
    pub source_file: Option<String>,
    pub layer: String,
    pub kind: String,
    pub immutable: bool,
}

/// Filesystem stamp published with a source document after indexing succeeds.
#[derive(Debug, Clone)]
pub(crate) struct SourceManifestStamp {
    pub canonical_path: String,
    pub canonical_root: String,
    pub size_bytes: u64,
    pub mtime_ns: i64,
}

#[derive(Debug, Serialize)]
pub struct ReembedDocumentResult {
    pub document_id: String,
    pub chunk_count: usize,
    pub dims: i32,
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct UpdateDocumentCommand {
    pub document_id: String,
    pub update: DocumentMetaUpdate,
}

#[derive(Debug, Serialize)]
pub struct UpdateDocumentResult {
    pub document_id: String,
    pub uri: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
    pub status: String,
    pub pinned: bool,
    pub boost: f64,
    pub metadata_json: String,
    pub layer: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_file: Option<String>,
    pub content_changed: bool,
    pub reembedded: bool,
    pub chunk_count: usize,
    pub updated_at: String,
}

pub struct IngestService<'a> {
    store: &'a Store,
    indexer: DocumentIndexer<'a>,
    config: &'a Config,
}

impl<'a> IngestService<'a> {
    pub fn new(
        store: &'a Store,
        embedder: &'a Arc<dyn EmbeddingProvider>,
        config: &'a Config,
    ) -> Self {
        Self {
            store,
            indexer: DocumentIndexer::new(embedder.as_ref(), config),
            config,
        }
    }

    pub async fn ingest(&self, command: IngestCommand) -> Result<IngestResult, AppError> {
        self.ingest_inner(command, None).await
    }

    pub(crate) async fn ingest_source(
        &self,
        command: IngestCommand,
        manifest: SourceManifestStamp,
    ) -> Result<IngestResult, AppError> {
        self.ingest_inner(command, Some(manifest)).await
    }

    async fn ingest_inner(
        &self,
        command: IngestCommand,
        manifest: Option<SourceManifestStamp>,
    ) -> Result<IngestResult, AppError> {
        self.ensure_vector_compatibility()?;
        let command = NormalizedIngest::try_from(command)?;
        let new_hash = content_hash(&command.text);
        let now = Utc::now();

        let (document_id, created_at, operation, expected_revision) = if let Some(existing) =
            self.store.find_by_uri(&command.uri)?
        {
            if command.immutable {
                let existing_hash = existing
                    .content_hash
                    .clone()
                    .unwrap_or_else(|| content_hash(&existing.content));
                if existing_hash == new_hash {
                    let etag = existing.etag();
                    let chunk_count = self.store.list_chunks_for_document(&existing.id)?.len();
                    let node_id = self
                        .store
                        .find_node_by_document_id(&existing.id)?
                        .map(|node| node.id)
                        .unwrap_or_default();
                    return Ok(IngestResult {
                        document_id: existing.id,
                        chunk_count,
                        node_id,
                        edge_count: 0,
                        content_hash: existing_hash,
                        op: "unchanged".into(),
                        revision: existing.revision,
                        etag,
                    });
                }
                return Err(AppError::conflict(format!(
                        "raw source uri '{}' is immutable; content differs from registered source (document_id={})",
                        command.uri, existing.id
                    )));
            }
            (
                existing.id,
                existing.created_at,
                "updated",
                Some(existing.revision),
            )
        } else {
            (Uuid::new_v4().to_string(), now, "inserted", None)
        };

        let document = Document {
            id: document_id.clone(),
            uri: command.uri,
            title: command.title,
            content: command.text,
            metadata_json: command.metadata_json,
            created_at,
            updated_at: now,
            wing: command.wing,
            room: command.room,
            source_file: command.source_file,
            layer: command.layer,
            kind: command.kind,
            content_hash: Some(new_hash.clone()),
            ..Default::default()
        };
        let chunks = self.indexer.build_chunks(&document).await?;
        let chunk_count = chunks.len();
        let write = if let Some(manifest) = manifest.as_ref() {
            self.store.write_source_document_atomic(
                &document,
                expected_revision,
                &chunks,
                SourceManifestWrite {
                    canonical_path: &manifest.canonical_path,
                    canonical_root: &manifest.canonical_root,
                    size_bytes: manifest.size_bytes,
                    mtime_ns: manifest.mtime_ns,
                    content_hash: &new_hash,
                    document_id: &document_id,
                },
            )?
        } else {
            self.store.write_document_atomic(
                &document,
                expected_revision,
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )?
        };

        Ok(IngestResult {
            document_id,
            chunk_count,
            node_id: write.node_id.unwrap_or_default(),
            edge_count: write.edge_count,
            content_hash: new_hash,
            op: operation.into(),
            revision: write.revision,
            etag: crate::models::format_document_etag(write.revision),
        })
    }

    pub async fn reembed_document(
        &self,
        document_id: &str,
    ) -> Result<ReembedDocumentResult, AppError> {
        let document_id = document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        let document = self
            .store
            .get_document(document_id)?
            .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
        let mut chunks = self.store.list_chunks_for_document(&document.id)?;
        let chunk_count = chunks.len();
        if !chunks.is_empty() {
            self.indexer.reembed_chunks(&mut chunks).await?;
            self.store
                .replace_chunks_for_document(&document.id, &chunks)?;
        }
        let manifest = self
            .store
            .write_embedding_manifest_from_config(self.config)?;
        Ok(ReembedDocumentResult {
            document_id: document.id,
            chunk_count,
            dims: manifest.dims,
            provider: manifest.provider,
            model: manifest.model,
        })
    }

    pub async fn update_document(
        &self,
        command: UpdateDocumentCommand,
    ) -> Result<UpdateDocumentResult, AppError> {
        let document_id = command.document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::config("document_id must be non-empty"));
        }
        let mut applied = self
            .store
            .prepare_document_meta_update(document_id, &command.update)?
            .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
        let expected_revision = applied.document.revision;

        let (write, chunk_count, reembedded) = if applied.content_changed {
            self.ensure_vector_compatibility()?;
            let chunks = self.indexer.build_plain_chunks(&applied.document).await?;
            let chunk_count = chunks.len();
            let write = self.store.write_document_atomic(
                &applied.document,
                Some(expected_revision),
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )?;
            (write, chunk_count, true)
        } else {
            let chunk_count = self
                .store
                .list_chunks_for_document(&applied.document.id)?
                .len();
            let derived = if applied.title_changed {
                DocumentDerivedWrite::RefreshGraphLabel
            } else {
                DocumentDerivedWrite::Preserve
            };
            let write = self.store.write_document_atomic(
                &applied.document,
                Some(expected_revision),
                derived,
            )?;
            (write, chunk_count, false)
        };
        applied.document.revision = write.revision;

        let document = applied.document;
        let _ = self.store.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: Utc::now(),
            op: "update_document_meta".into(),
            prefix: Some("META".into()),
            message: format!("updated document meta for {}", document.id),
            entity_id: Some(document.id.clone()),
            entity_kind: Some("document".into()),
            payload_json: serde_json::json!({
                "content_changed": applied.content_changed,
                "title_changed": applied.title_changed,
                "reembedded": reembedded,
                "wing": document.wing,
                "room": document.room,
                "status": document.status,
                "pinned": document.pinned,
                "boost": document.boost,
            })
            .to_string(),
            agent_name: None,
        });
        Ok(UpdateDocumentResult {
            document_id: document.id,
            uri: document.uri,
            title: document.title,
            wing: document.wing,
            room: document.room,
            status: document.status,
            pinned: document.pinned,
            boost: document.boost,
            metadata_json: document.metadata_json,
            layer: document.layer,
            kind: document.kind,
            source_file: document.source_file,
            content_changed: applied.content_changed,
            reembedded,
            chunk_count,
            updated_at: document.updated_at.to_rfc3339(),
        })
    }

    fn ensure_vector_compatibility(&self) -> Result<(), AppError> {
        self.store.ensure_embedding_manifest(self.config)?;
        self.store
            .require_embedding_dims_match(self.config.embedding_dims)
    }
}

struct NormalizedIngest {
    text: String,
    title: String,
    uri: String,
    metadata_json: String,
    wing: Option<String>,
    room: Option<String>,
    source_file: Option<String>,
    layer: String,
    kind: String,
    immutable: bool,
}

impl TryFrom<IngestCommand> for NormalizedIngest {
    type Error = AppError;

    fn try_from(command: IngestCommand) -> Result<Self, Self::Error> {
        let metadata_json = nonempty(command.metadata_json).unwrap_or_else(|| "{}".into());
        serde_json::from_str::<serde_json::Value>(&metadata_json).map_err(|error| {
            AppError::config(format!("metadata_json is not valid JSON: {error}"))
        })?;
        let uri = nonempty(command.uri).unwrap_or_else(|| format!("text://{}", Uuid::new_v4()));
        let title = nonempty(command.title).unwrap_or_else(|| {
            uri.rsplit('/')
                .next()
                .filter(|value| !value.is_empty())
                .unwrap_or("untitled")
                .to_string()
        });
        Ok(Self {
            text: command.text,
            title,
            uri,
            metadata_json,
            wing: nonempty(command.wing),
            room: nonempty(command.room),
            source_file: nonempty(command.source_file),
            layer: normalized_or(command.layer, "raw"),
            kind: normalized_or(command.kind, "document"),
            immutable: command.immutable,
        })
    }
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|item| !item.trim().is_empty())
}

fn normalized_or(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    use crate::embeddings::MockEmbedder;

    struct FailingEmbedder {
        dims: usize,
    }

    struct BarrierEmbedder {
        barrier: Arc<tokio::sync::Barrier>,
        inner: MockEmbedder,
    }

    #[async_trait]
    impl EmbeddingProvider for FailingEmbedder {
        async fn embed(&self, _texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            Err(AppError::embeddings("injected embedding failure"))
        }

        fn dimensions(&self) -> usize {
            self.dims
        }
    }

    #[async_trait]
    impl EmbeddingProvider for BarrierEmbedder {
        async fn embed(&self, texts: &[String]) -> crate::error::Result<Vec<Vec<f32>>> {
            self.barrier.wait().await;
            self.inner.embed(texts).await
        }

        fn dimensions(&self) -> usize {
            self.inner.dimensions()
        }
    }

    fn test_config(db_path: std::path::PathBuf) -> Config {
        Config {
            db_path,
            embedding_dims: 8,
            chunk_size: 64,
            chunk_overlap: 8,
            ..Config::for_tests()
        }
    }

    fn command(text: &str) -> IngestCommand {
        IngestCommand {
            text: text.into(),
            title: Some("Atomic page".into()),
            uri: Some("wiki://atomic-page".into()),
            metadata_json: Some("{}".into()),
            wing: Some("tests".into()),
            room: Some("atomicity".into()),
            source_file: None,
            layer: "wiki".into(),
            kind: "wiki".into(),
            immutable: false,
        }
    }

    #[tokio::test]
    async fn source_manifest_failure_rolls_back_document_and_derived_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path().join("source-manifest-atomic.duckdb"));
        let store = Store::open(&config.db_path).expect("store");
        store
            .lock()
            .unwrap()
            .execute_batch("DROP TABLE source_manifest")
            .unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));

        let error = IngestService::new(&store, &embedder, &config)
            .ingest_source(
                IngestCommand {
                    source_file: Some("/sources/atomic.md".into()),
                    layer: "raw".into(),
                    kind: "document".into(),
                    ..command("Source body links to [[Target]].")
                },
                SourceManifestStamp {
                    canonical_path: "/sources/atomic.md".into(),
                    canonical_root: "/sources".into(),
                    size_bytes: 32,
                    mtime_ns: 1,
                },
            )
            .await
            .expect_err("manifest write must fail");

        assert!(error.to_string().contains("source_manifest"));
        assert!(store.list_documents().unwrap().is_empty());
        assert!(store
            .get_graph_view(crate::models::GraphFilter::default())
            .unwrap()
            .nodes
            .is_empty());
        assert!(store.list_graph_edges().unwrap().is_empty());
    }

    #[tokio::test]
    async fn embedding_failure_leaves_document_chunks_and_graph_unchanged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = test_config(temp.path().join("ingest-atomic.duckdb"));
        let store = Store::open(&config.db_path).expect("store");
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
        let service = IngestService::new(&store, &embedder, &config);
        let inserted = service
            .ingest(command("Original body links to [[Old target]]."))
            .await
            .expect("initial ingest");

        let original_document = store
            .get_document(&inserted.document_id)
            .unwrap()
            .expect("document");
        let original_chunks = store
            .list_chunks_for_document(&inserted.document_id)
            .unwrap();
        let original_node = store
            .find_node_by_document_id(&inserted.document_id)
            .unwrap()
            .expect("node");
        let original_edges: Vec<_> = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.source_id == original_node.id)
            .collect();

        let failing_embedder: Arc<dyn EmbeddingProvider> = Arc::new(FailingEmbedder { dims: 8 });
        let failing_service = IngestService::new(&store, &failing_embedder, &config);
        let error = failing_service
            .update_document(UpdateDocumentCommand {
                document_id: inserted.document_id.clone(),
                update: DocumentMetaUpdate {
                    title: Some("Changed title".into()),
                    content: Some("Changed body links to [[New target]].".into()),
                    ..Default::default()
                },
            })
            .await
            .expect_err("embedding must fail before persistence");
        assert!(error.to_string().contains("injected embedding failure"));

        let persisted_document = store
            .get_document(&inserted.document_id)
            .unwrap()
            .expect("document after failure");
        assert_eq!(persisted_document.revision, original_document.revision);
        assert_eq!(persisted_document.title, original_document.title);
        assert_eq!(persisted_document.content, original_document.content);
        let persisted_chunks = store
            .list_chunks_for_document(&inserted.document_id)
            .unwrap();
        assert_eq!(persisted_chunks.len(), original_chunks.len());
        assert_eq!(
            persisted_chunks
                .iter()
                .map(|chunk| (&chunk.id, &chunk.content))
                .collect::<Vec<_>>(),
            original_chunks
                .iter()
                .map(|chunk| (&chunk.id, &chunk.content))
                .collect::<Vec<_>>()
        );
        let persisted_node = store
            .find_node_by_document_id(&inserted.document_id)
            .unwrap()
            .expect("node after failure");
        assert_eq!(persisted_node.id, original_node.id);
        assert_eq!(persisted_node.label, original_node.label);
        let persisted_edges: Vec<_> = store
            .list_graph_edges()
            .unwrap()
            .into_iter()
            .filter(|edge| edge.source_id == persisted_node.id)
            .collect();
        assert_eq!(persisted_edges.len(), original_edges.len());
        assert_eq!(persisted_edges[0].id, original_edges[0].id);
        assert!(store.find_nodes_by_label("New target").unwrap().is_empty());
    }

    #[tokio::test]
    async fn concurrent_initial_ingest_never_creates_duplicate_uri_owners() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = Arc::new(test_config(temp.path().join("initial-race.duckdb")));
        let store = Arc::new(Store::open(&config.db_path).expect("store"));
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(BarrierEmbedder {
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            inner: MockEmbedder::new(8),
        });

        let mut tasks = Vec::new();
        for text in ["first contender", "second contender"] {
            let store = store.clone();
            let config = config.clone();
            let embedder = embedder.clone();
            tasks.push(tokio::spawn(async move {
                IngestService::new(store.as_ref(), &embedder, config.as_ref())
                    .ingest(command(text))
                    .await
            }));
        }
        let results = [
            tasks.remove(0).await.unwrap(),
            tasks.remove(0).await.unwrap(),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(AppError::Conflict(_))))
                .count(),
            1
        );
        let documents = store.list_documents().unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].uri, "wiki://atomic-page");
    }

    #[tokio::test]
    async fn concurrent_updates_use_the_revision_observed_before_embedding() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config = Arc::new(test_config(temp.path().join("update-race.duckdb")));
        let store = Arc::new(Store::open(&config.db_path).expect("store"));
        let initial_embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(8));
        IngestService::new(store.as_ref(), &initial_embedder, config.as_ref())
            .ingest(command("initial"))
            .await
            .unwrap();
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(BarrierEmbedder {
            barrier: Arc::new(tokio::sync::Barrier::new(2)),
            inner: MockEmbedder::new(8),
        });

        let mut tasks = Vec::new();
        for text in ["update one", "update two"] {
            let store = store.clone();
            let config = config.clone();
            let embedder = embedder.clone();
            tasks.push(tokio::spawn(async move {
                IngestService::new(store.as_ref(), &embedder, config.as_ref())
                    .ingest(command(text))
                    .await
            }));
        }
        let results = [
            tasks.remove(0).await.unwrap(),
            tasks.remove(0).await.unwrap(),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(AppError::Conflict(_))))
                .count(),
            1
        );
        let document = store.list_documents().unwrap().pop().unwrap();
        assert_eq!(document.revision, 2);
        assert!(matches!(
            document.content.as_str(),
            "update one" | "update two"
        ));
    }
}
