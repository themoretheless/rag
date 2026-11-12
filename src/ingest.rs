//! Application service for document ingestion and embedding refresh.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::config::Config;
use crate::db::store::{DocumentDerivedWrite, embedding_manifest_from_config};
use crate::db::{SourceManifestWrite, Store};
use crate::document_indexer::{DocumentIndexer, PreparedDocumentChunks};
use crate::embeddings::EmbeddingProvider;
use crate::error::AppError;
use crate::file_ingest::{extract_file, merge_metadata};
use crate::models::{Chunk, Document, DocumentMetaUpdate, IngestResult, OpsLogEntry};
use crate::util::{check_path_allowlist, content_hash, non_blank};

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

/// An ingest whose document identity and chunks are fixed, but whose vectors
/// have not been requested yet.
#[derive(Clone)]
pub(crate) struct PreparedIngest {
    write: Box<PendingIngestWrite>,
    chunks: PreparedDocumentChunks,
}

impl PreparedIngest {
    pub(crate) fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
}

/// A prepared ingest with vectors mapped back to its document chunks.
pub(crate) struct EmbeddedIngest {
    write: Box<PendingIngestWrite>,
    chunks: Vec<Chunk>,
}

#[derive(Clone)]
struct PendingIngestWrite {
    document: Document,
    expected_revision: Option<i64>,
    operation: &'static str,
    content_hash: String,
    manifest: Option<SourceManifestStamp>,
}

/// Chunks per embedding call in [`IngestService::ingest_many`].
const INGEST_MANY_BATCH_CHUNKS: usize = 64;

enum IngestPreparation {
    Complete(IngestResult),
    Pending(PreparedIngest),
}

/// Ingest one file from disk (`ingest_file` MCP tool / `POST /v1/ingest/file`).
///
/// The path must sit under `RAG_INGEST_ROOTS`; `uri` defaults to `file://` + the
/// canonical path and `title` to the file name.
#[derive(Debug, Clone, Default)]
pub struct IngestFileCommand {
    pub path: String,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub metadata_json: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ReembedDocumentResult {
    pub document_id: String,
    pub chunk_count: usize,
    pub dims: i32,
    pub provider: String,
    pub model: String,
}

/// Fetch one allowlisted URL over HTTP(S) and ingest it (`ingest_url`).
///
/// `uri` defaults to the final post-redirect URL and `title` to the URL; the
/// final URL is also stored as `source_file` so a later refetch upserts the
/// same document (content-hash dedup makes unchanged pages a cheap re-write).
#[derive(Debug, Clone, Default)]
pub struct IngestUrlCommand {
    pub url: String,
    pub title: Option<String>,
    pub uri: Option<String>,
    pub metadata_json: Option<String>,
    pub wing: Option<String>,
    pub room: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IngestUrlResult {
    #[serde(flatten)]
    pub ingest: IngestResult,
    pub requested_url: String,
    pub final_url: String,
    pub http_status: u16,
    pub content_type: Option<String>,
    pub bytes: usize,
    pub format: String,
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
    embedder: &'a Arc<dyn EmbeddingProvider>,
    indexer: DocumentIndexer<'a>,
    config: &'a Config,
}

impl<'a> IngestService<'a> {
    /// Bind the service to a store, embedder and config.
    pub fn new(
        store: &'a Store,
        embedder: &'a Arc<dyn EmbeddingProvider>,
        config: &'a Config,
    ) -> Self {
        Self {
            store,
            embedder,
            indexer: DocumentIndexer::new(embedder.as_ref(), config),
            config,
        }
    }

    /// Run synchronous work (file extraction, chunking, DuckDB reads and
    /// commits) on the blocking pool with an owned service, so ingest never
    /// pins an async worker. Only the embedding call stays on the executor.
    async fn off_thread<T, F>(&self, operation: &'static str, work: F) -> Result<T, AppError>
    where
        T: Send + 'static,
        F: FnOnce(&IngestService<'_>) -> Result<T, AppError> + Send + 'static,
    {
        let store = self.store.clone();
        let embedder = self.embedder.clone();
        let config = self.config.clone();
        crate::util::run_blocking(operation, move || {
            work(&IngestService::new(&store, &embedder, &config))
        })
        .await
    }

    /// Read, extract, and ingest one allowlisted file as `layer=raw`, `kind=document`.
    pub async fn ingest_file(&self, command: IngestFileCommand) -> Result<IngestResult, AppError> {
        let ingest = self
            .off_thread("ingest_file extract", move |service| {
                service.file_ingest_command(command)
            })
            .await?;
        self.ingest(ingest).await
    }

    /// Extract an allowlisted file and build its ingest command (blocking I/O).
    fn file_ingest_command(&self, command: IngestFileCommand) -> Result<IngestCommand, AppError> {
        let path = Path::new(&command.path);
        check_path_allowlist(path, &self.config.ingest_roots)?;
        let extracted = extract_file(path).map_err(|error| {
            if path.try_exists().ok() == Some(false) {
                AppError::not_found(format!("file not found: {}", command.path))
            } else {
                AppError::config(error.to_string())
            }
        })?;
        let canonical = path.canonicalize().ok();
        let default_uri = canonical
            .as_ref()
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_else(|| format!("file://{}", command.path));
        let uri = command.uri.unwrap_or(default_uri);
        let base_metadata = match command.metadata_json {
            Some(metadata) => Some(metadata),
            None => self
                .store
                .documents()
                .find_by_uri(&uri)?
                .map(|doc| doc.metadata_json),
        };
        let metadata_json = merge_metadata(base_metadata, extracted.metadata).map_err(|error| {
            AppError::invalid(format!("metadata_json is not valid JSON: {error}"))
        })?;
        let default_title = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
        let source_file = canonical
            .map(|p| p.display().to_string())
            .or_else(|| Some(command.path.clone()));
        Ok(IngestCommand {
            text: extracted.text,
            title: command.title.or(default_title),
            uri: Some(uri),
            metadata_json: Some(metadata_json),
            wing: command.wing,
            room: command.room,
            source_file,
            layer: crate::models::Layer::Raw.as_str().into(),
            kind: crate::models::DocumentKind::Document.as_str().into(),
            immutable: false,
        })
    }

    /// Fetch one allowlisted URL and ingest the extracted main content.
    ///
    /// Policy: disabled unless `RAG_INGEST_URL_ALLOW` is set; http/https only;
    /// the allowlist is enforced on both the requested host and the final
    /// post-redirect host; the body is capped at `ingest_url_max_bytes`.
    pub async fn ingest_url(
        &self,
        fetcher: &dyn crate::web_ingest::UrlFetcher,
        command: IngestUrlCommand,
    ) -> Result<IngestUrlResult, AppError> {
        use crate::web_ingest::{PageKind, check_final_url, check_url, classify_content_type};

        let checked = check_url(&command.url, &self.config.ingest_url_allow)?;
        let page = fetcher
            .fetch(checked.url.as_str(), self.config.ingest_url_max_bytes)
            .await?;
        // Redirects must not escape the allowlist.
        let final_checked = check_final_url(&page.final_url, &self.config.ingest_url_allow)?;
        let final_url = final_checked.url.as_str().to_string();

        let kind = classify_content_type(page.content_type.as_deref(), &final_url)?;
        let raw_text = String::from_utf8(page.body).map_err(|error| {
            AppError::config(format!(
                "ingest_url body for {final_url} is not valid UTF-8: {error}"
            ))
        })?;
        let (text, format) = match kind {
            PageKind::Html => (crate::file_ingest::html_to_text(&raw_text), "html"),
            PageKind::Text => (raw_text, "text"),
        };
        if text.trim().is_empty() {
            return Err(AppError::config(format!(
                "ingest_url extracted no text from {final_url}"
            )));
        }

        let uri = command.uri.unwrap_or_else(|| final_url.clone());
        let base_metadata = match command.metadata_json {
            Some(metadata) => Some(metadata),
            None => self
                .store
                .documents()
                .find_by_uri(&uri)?
                .map(|doc| doc.metadata_json),
        };
        let fetched_at = Utc::now();
        let mut additions = serde_json::Map::new();
        additions.insert("format".into(), serde_json::Value::String(format.into()));
        additions.insert(
            "source_url".into(),
            serde_json::Value::String(final_url.clone()),
        );
        additions.insert(
            "requested_url".into(),
            serde_json::Value::String(command.url.clone()),
        );
        additions.insert(
            "fetched_at".into(),
            serde_json::Value::String(fetched_at.to_rfc3339()),
        );
        if let Some(content_type) = page.content_type.as_ref() {
            additions.insert(
                "http_content_type".into(),
                serde_json::Value::String(content_type.clone()),
            );
        }
        let metadata_json = merge_metadata(base_metadata, additions).map_err(|error| {
            AppError::invalid(format!("metadata_json is not valid JSON: {error}"))
        })?;
        let title = command.title.or_else(|| Some(final_checked.host.clone()));
        let bytes = text.len();

        let ingest = self
            .ingest(IngestCommand {
                text,
                title,
                uri: Some(uri.clone()),
                metadata_json: Some(metadata_json),
                wing: command.wing,
                room: command.room,
                source_file: Some(final_url.clone()),
                layer: "raw".into(),
                kind: "document".into(),
                immutable: false,
            })
            .await?;

        self.store.ops_log().record(
            &OpsLogEntry::new("ingest_url", "INGEST", format!("ingested {final_url}"))
                .entity(Some(ingest.document_id.clone()), "document")
                .payload(
                    serde_json::json!({
                        "requested_url": command.url,
                        "final_url": final_url,
                        "http_status": page.status,
                        "content_type": page.content_type,
                        "format": format,
                        "bytes": bytes,
                        "chunk_count": ingest.chunk_count,
                        "op": ingest.op,
                    })
                    .to_string(),
                ),
        );

        Ok(IngestUrlResult {
            ingest,
            requested_url: command.url,
            final_url,
            http_status: page.status,
            content_type: page.content_type,
            bytes,
            format: format.into(),
        })
    }

    /// Ingest one document: prepare (dedupe, chunk), embed, then commit.
    pub async fn ingest(&self, command: IngestCommand) -> Result<IngestResult, AppError> {
        let preparation = self
            .off_thread("ingest prepare", move |service| {
                service.prepare_ingest(command, None)
            })
            .await?;
        match preparation {
            IngestPreparation::Complete(result) => Ok(result),
            IngestPreparation::Pending(prepared) => {
                let embedded = self
                    .embed_prepared_batch(vec![prepared])
                    .await?
                    .pop()
                    .expect("one prepared ingest yields one embedded ingest");
                self.off_thread("ingest commit", move |service| {
                    service.commit_embedded(embedded)
                })
                .await
            }
        }
    }

    /// Ingest several documents, embedding up to [`INGEST_MANY_BATCH_CHUNKS`]
    /// chunks per provider call; results keep `commands` order.
    ///
    /// Each document still commits at its own atomic/CAS boundary. When a
    /// grouped embedding call fails, that group's documents are embedded one
    /// by one, so a single bad document fails alone, as with [`Self::ingest`].
    pub(crate) async fn ingest_many(
        &self,
        commands: Vec<IngestCommand>,
    ) -> Vec<Result<IngestResult, AppError>> {
        let mut results: Vec<Option<Result<IngestResult, AppError>>> =
            (0..commands.len()).map(|_| None).collect();
        let mut group: Vec<(usize, PreparedIngest)> = Vec::new();
        let mut group_chunks = 0usize;
        let preparations = self
            .off_thread("ingest_many prepare", move |service| {
                Ok(commands
                    .into_iter()
                    .map(|command| service.prepare_ingest(command, None))
                    .collect::<Vec<_>>())
            })
            .await;
        let preparations = match preparations {
            Ok(preparations) => preparations,
            Err(error) => {
                let message = error.to_string();
                return results
                    .into_iter()
                    .map(|_| Err(AppError::db(message.clone())))
                    .collect();
            }
        };
        for (index, preparation) in preparations.into_iter().enumerate() {
            match preparation {
                Ok(IngestPreparation::Complete(result)) => results[index] = Some(Ok(result)),
                Ok(IngestPreparation::Pending(prepared)) => {
                    let chunks = prepared.chunk_count();
                    if !group.is_empty() && group_chunks + chunks > INGEST_MANY_BATCH_CHUNKS {
                        self.embed_and_commit(std::mem::take(&mut group), &mut results)
                            .await;
                        group_chunks = 0;
                    }
                    group_chunks += chunks;
                    group.push((index, prepared));
                }
                Err(error) => results[index] = Some(Err(error)),
            }
        }
        self.embed_and_commit(group, &mut results).await;
        results
            .into_iter()
            .map(|result| result.expect("every command yields a result"))
            .collect()
    }

    async fn embed_and_commit(
        &self,
        group: Vec<(usize, PreparedIngest)>,
        results: &mut [Option<Result<IngestResult, AppError>>],
    ) {
        if group.is_empty() {
            return;
        }
        let (indexes, prepared): (Vec<usize>, Vec<PreparedIngest>) = group.into_iter().unzip();
        // Keep a copy of each plan so a failed group can be retried per document.
        let retry: Option<Vec<PreparedIngest>> = (prepared.len() > 1).then(|| prepared.clone());
        let mut embedded: Vec<(usize, Result<EmbeddedIngest, AppError>)> = Vec::new();
        match (self.embed_prepared_batch(prepared).await, retry) {
            (Ok(vectors), _) => {
                embedded.extend(indexes.into_iter().zip(vectors.into_iter().map(Ok)));
            }
            (Err(error), None) => embedded.push((indexes[0], Err(error))),
            (Err(_), Some(retry)) => {
                for (index, prepared) in indexes.into_iter().zip(retry) {
                    let one = self
                        .embed_prepared_batch(vec![prepared])
                        .await
                        .map(|mut one| {
                            one.pop()
                                .expect("one prepared ingest yields one embedded ingest")
                        });
                    embedded.push((index, one));
                }
            }
        }
        // Commit every embedded document (each at its own boundary) in one
        // blocking-pool hop.
        let committed = self
            .off_thread("ingest_many commit", move |service| {
                Ok(embedded
                    .into_iter()
                    .map(|(index, embedded)| {
                        (
                            index,
                            embedded.and_then(|embedded| service.commit_embedded(embedded)),
                        )
                    })
                    .collect::<Vec<_>>())
            })
            .await;
        match committed {
            Ok(committed) => {
                for (index, result) in committed {
                    results[index] = Some(result);
                }
            }
            Err(error) => {
                for slot in results.iter_mut().filter(|slot| slot.is_none()) {
                    *slot = Some(Err(AppError::db(error.to_string())));
                }
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn ingest_source(
        &self,
        command: IngestCommand,
        manifest: SourceManifestStamp,
    ) -> Result<IngestResult, AppError> {
        let prepared = self.prepare_source(command, manifest)?;
        let embedded = self
            .embed_prepared_batch(vec![prepared])
            .await?
            .pop()
            .expect("one prepared source yields one embedded ingest");
        self.commit_embedded(embedded)
    }

    /// Prepare a mutable source ingest without contacting the embedding
    /// provider or writing document state.
    pub(crate) fn prepare_source(
        &self,
        command: IngestCommand,
        manifest: SourceManifestStamp,
    ) -> Result<PreparedIngest, AppError> {
        debug_assert!(!command.immutable);
        match self.prepare_ingest(command, Some(manifest))? {
            IngestPreparation::Pending(prepared) => Ok(prepared),
            IngestPreparation::Complete(_) => Err(AppError::conflict(
                "mutable source ingest unexpectedly completed during preparation",
            )),
        }
    }

    /// Embed all chunk plans in one provider request while retaining document
    /// boundaries and input order.
    pub(crate) async fn embed_prepared_batch(
        &self,
        batch: Vec<PreparedIngest>,
    ) -> Result<Vec<EmbeddedIngest>, AppError> {
        let (writes, chunk_plans): (Vec<_>, Vec<_>) = batch
            .into_iter()
            .map(|prepared| (prepared.write, prepared.chunks))
            .unzip();
        // Re-ingest of an existing document: reuse the stored vector of every
        // chunk whose text is unchanged, so only edited chunks reach the
        // provider (an identical re-ingest makes no provider call at all).
        let existing: Vec<&str> = writes
            .iter()
            .filter(|write| write.expected_revision.is_some())
            .map(|write| write.document.id.as_str())
            .collect();
        let known = if existing.is_empty() {
            std::collections::HashMap::new()
        } else {
            crate::util::blocking_section(|| {
                self.store.chunks().chunk_vectors_by_content(&existing)
            })?
        };
        let chunk_groups = if known.is_empty() {
            self.indexer.embed_prepared_batch(chunk_plans).await?
        } else {
            let reusing =
                crate::embeddings::PrecomputedEmbedder::with_vectors(self.embedder.clone(), known);
            DocumentIndexer::new(&reusing, self.config)
                .embed_prepared_batch(chunk_plans)
                .await?
        };
        Ok(writes
            .into_iter()
            .zip(chunk_groups)
            .map(|(write, chunks)| EmbeddedIngest { write, chunks })
            .collect())
    }

    /// Persist exactly one embedded document at the existing atomic/CAS
    /// boundary. Source-sync callers invoke this in canonical path order.
    pub(crate) fn commit_embedded(
        &self,
        embedded: EmbeddedIngest,
    ) -> Result<IngestResult, AppError> {
        let EmbeddedIngest { write, chunks } = embedded;
        let PendingIngestWrite {
            document,
            expected_revision,
            operation,
            content_hash,
            manifest,
        } = *write;
        let chunk_count = chunks.len();
        let document_id = document.id.clone();
        let persisted = if let Some(manifest) = manifest.as_ref() {
            self.store.documents().write_source_document_atomic(
                &document,
                expected_revision,
                &chunks,
                SourceManifestWrite {
                    canonical_path: &manifest.canonical_path,
                    canonical_root: &manifest.canonical_root,
                    size_bytes: manifest.size_bytes,
                    mtime_ns: manifest.mtime_ns,
                    content_hash: &content_hash,
                    document_id: &document_id,
                },
            )?
        } else {
            self.store.documents().write_document_atomic(
                &document,
                expected_revision,
                DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
            )?
        };

        Ok(IngestResult {
            document_id,
            chunk_count,
            node_id: persisted.node_id.unwrap_or_default(),
            edge_count: persisted.edge_count,
            content_hash,
            op: operation.into(),
            revision: persisted.revision,
            etag: crate::models::format_document_etag(persisted.revision),
        })
    }

    fn prepare_ingest(
        &self,
        command: IngestCommand,
        manifest: Option<SourceManifestStamp>,
    ) -> Result<IngestPreparation, AppError> {
        self.ensure_vector_compatibility()?;
        let preserve_metadata = command.metadata_json.is_none();
        let command = NormalizedIngest::try_from(command)?;
        let new_hash = content_hash(&command.text);
        let now = Utc::now();

        let existing = self.store.documents().find_by_uri(&command.uri)?;
        let (document_id, created_at, operation, expected_revision) = if let Some(existing) =
            existing.as_ref()
        {
            if command.immutable {
                let existing_hash = existing
                    .content_hash
                    .clone()
                    .unwrap_or_else(|| content_hash(&existing.content));
                if existing_hash == new_hash {
                    let etag = existing.etag();
                    let chunk_count = self
                        .store
                        .chunks()
                        .count_chunks_for_document(&existing.id)?
                        as usize;
                    let node_id = self
                        .store
                        .graph()
                        .find_node_by_document_id(&existing.id)?
                        .map(|node| node.id)
                        .unwrap_or_default();
                    return Ok(IngestPreparation::Complete(IngestResult {
                        document_id: existing.id.clone(),
                        chunk_count,
                        node_id,
                        edge_count: 0,
                        content_hash: existing_hash,
                        op: "unchanged".into(),
                        revision: existing.revision,
                        etag,
                    }));
                }
                return Err(AppError::conflict(format!(
                    "raw source uri '{}' is immutable; content differs from registered source (document_id={})",
                    command.uri, existing.id
                )));
            }
            (
                existing.id.clone(),
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
            metadata_json: if preserve_metadata {
                existing
                    .as_ref()
                    .map(|doc| doc.metadata_json.clone())
                    .unwrap_or(command.metadata_json)
            } else {
                command.metadata_json
            },
            created_at,
            updated_at: now,
            wing: command
                .wing
                .or_else(|| existing.as_ref().and_then(|doc| doc.wing.clone())),
            room: command
                .room
                .or_else(|| existing.as_ref().and_then(|doc| doc.room.clone())),
            source_file: command
                .source_file
                .or_else(|| existing.as_ref().and_then(|doc| doc.source_file.clone())),
            layer: command.layer.into(),
            kind: command.kind.into(),
            content_hash: Some(new_hash.clone()),
            ..existing.unwrap_or_default()
        };
        let chunks = self.indexer.prepare_chunks(&document);
        Ok(IngestPreparation::Pending(PreparedIngest {
            write: Box::new(PendingIngestWrite {
                document,
                expected_revision,
                operation,
                content_hash: new_hash,
                manifest,
            }),
            chunks,
        }))
    }

    /// Re-embed an existing document's chunks with the current embedder.
    pub async fn reembed_document(
        &self,
        document_id: &str,
    ) -> Result<ReembedDocumentResult, AppError> {
        let document_id = document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::invalid("document_id must be non-empty"));
        }
        let (document, expected_chunks) = crate::util::blocking_section(|| {
            let document = self
                .store
                .documents()
                .get_document(document_id)?
                .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
            self.store
                .embedding_manifest()
                .require_embedding_manifest_match(self.config)?;
            let chunks = self.store.chunks().list_chunks_for_document(&document.id)?;
            Ok::<_, AppError>((document, chunks))
        })?;
        let mut chunks = expected_chunks.clone();
        let chunk_count = chunks.len();
        if !chunks.is_empty() {
            self.indexer.reembed_chunks(&mut chunks).await?;
        }
        let embeddings = chunks
            .into_iter()
            .map(|chunk| chunk.embedding)
            .collect::<Vec<_>>();
        let manifest = embedding_manifest_from_config(self.config);
        crate::util::blocking_section(|| {
            self.store.chunks().update_chunk_embeddings_atomic(
                &document.id,
                document.revision,
                &expected_chunks,
                &embeddings,
                Some(&manifest),
            )
        })?;
        Ok(ReembedDocumentResult {
            document_id: document.id,
            chunk_count,
            dims: manifest.dims,
            provider: manifest.provider,
            model: manifest.model,
        })
    }

    /// Apply a metadata or content update to an existing document, re-embedding when content changed.
    pub async fn update_document(
        &self,
        command: UpdateDocumentCommand,
    ) -> Result<UpdateDocumentResult, AppError> {
        let document_id = command.document_id.trim();
        if document_id.is_empty() {
            return Err(AppError::invalid("document_id must be non-empty"));
        }
        let mut applied = crate::util::blocking_section(|| {
            self.store
                .documents()
                .prepare_document_meta_update(document_id, &command.update)
        })?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;
        let expected_revision = applied.document.revision;

        let (write, chunk_count, reembedded) = if applied.content_changed {
            self.ensure_vector_compatibility()?;
            let chunk_embedder = crate::util::blocking_section(|| {
                crate::document_indexer::reusing_document_vectors(
                    self.store,
                    self.embedder,
                    &applied.document.id,
                )
            })?;
            let chunks = DocumentIndexer::new(chunk_embedder.as_ref(), self.config)
                .build_plain_chunks(&applied.document)
                .await?;
            let chunk_count = chunks.len();
            let write = crate::util::blocking_section(|| {
                self.store.documents().write_document_atomic(
                    &applied.document,
                    Some(expected_revision),
                    DocumentDerivedWrite::ReplaceChunksAndGraph(&chunks),
                )
            })?;
            (write, chunk_count, true)
        } else {
            let derived = if applied.title_changed {
                DocumentDerivedWrite::RefreshGraphLabel
            } else {
                DocumentDerivedWrite::Preserve
            };
            let (write, chunk_count) = crate::util::blocking_section(|| {
                let chunk_count = self
                    .store
                    .chunks()
                    .count_chunks_for_document(&applied.document.id)?;
                let write = self.store.documents().write_document_atomic(
                    &applied.document,
                    Some(expected_revision),
                    derived,
                )?;
                Ok::<_, AppError>((write, chunk_count as usize))
            })?;
            (write, chunk_count, false)
        };
        applied.document.revision = write.revision;

        let document = applied.document;
        crate::util::blocking_section(|| {
            self.store.ops_log().record(
                &OpsLogEntry::new(
                    "update_document_meta",
                    "META",
                    format!("updated document meta for {}", document.id),
                )
                .entity(Some(document.id.clone()), "document")
                .payload(
                    serde_json::json!({
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
                ),
            )
        });
        Ok(UpdateDocumentResult {
            document_id: document.id,
            uri: document.uri,
            title: document.title,
            wing: document.wing,
            room: document.room,
            status: document.status.to_string(),
            pinned: document.pinned,
            boost: document.boost,
            metadata_json: document.metadata_json,
            layer: document.layer.to_string(),
            kind: document.kind.to_string(),
            source_file: document.source_file,
            content_changed: applied.content_changed,
            reembedded,
            chunk_count,
            updated_at: document.updated_at.to_rfc3339(),
        })
    }

    fn ensure_vector_compatibility(&self) -> Result<(), AppError> {
        crate::util::blocking_section(|| {
            self.store
                .embedding_manifest()
                .ensure_embedding_manifest(self.config)?;
            self.store
                .embedding_manifest()
                .require_embedding_manifest_match(self.config)
        })
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
        let metadata_json = non_blank(command.metadata_json).unwrap_or_else(|| "{}".into());
        serde_json::from_str::<serde_json::Value>(&metadata_json).map_err(|error| {
            AppError::invalid(format!("metadata_json is not valid JSON: {error}"))
        })?;
        let uri = non_blank(command.uri).unwrap_or_else(|| format!("text://{}", Uuid::new_v4()));
        let title = non_blank(command.title).unwrap_or_else(|| {
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
            wing: non_blank(command.wing),
            room: non_blank(command.room),
            source_file: non_blank(command.source_file),
            layer: normalized_or(command.layer, "raw"),
            kind: normalized_or(command.kind, "document"),
            immutable: command.immutable,
        })
    }
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
mod tests;
