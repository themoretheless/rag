//! Actualize / refresh: FTS reindex, graph rebuild, wiki catalog, optional reembed.
//!
//! Whitelist-only actions (no SQL/shell invention). Logs every apply to `ops_log`.
//! See `docs/LOCAL_LLM_MAINTENANCE.md` §3.5.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::config::Config;
use crate::db::{self, Store};
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::graph::rebuild_document_graph;
use crate::models::{EmbeddingManifest, OpsLogEntry};

/// Allowed `maintain_refresh` action names (safety whitelist).
pub const REFRESH_ACTION_WHITELIST: &[&str] = &[
    "reindex_fts",
    "rebuild_graph",
    "rebuild_wiki_index",
    "reembed_all",
];

/// Flags controlling which refresh steps run.
///
/// Defaults (when built via [`MaintainRefreshFlags::from_options`]):
/// - `reindex_fts`, `rebuild_graph`, `rebuild_wiki_index` = **true**
/// - `graph_dirty_only` = **true** (missing document nodes only)
/// - `reembed_all` = **false** (opt-in; expensive)
/// - `dry_run` = **false**
#[derive(Debug, Clone)]
pub struct MaintainRefreshFlags {
    pub reindex_fts: bool,
    pub rebuild_graph: bool,
    /// When true with `rebuild_graph`, only docs without a document graph node.
    /// When false, rebuild graph for every document (capped by `max_docs`).
    pub graph_dirty_only: bool,
    pub rebuild_wiki_index: bool,
    /// Opt-in: re-embed all document chunks with the live embedder.
    pub reembed_all: bool,
    /// When true, report planned work without mutating store.
    pub dry_run: bool,
    /// Cap documents touched by graph rebuild / reembed (`RAG_MAINT_MAX_DOCS`).
    pub max_docs: usize,
}

impl Default for MaintainRefreshFlags {
    fn default() -> Self {
        Self {
            reindex_fts: true,
            rebuild_graph: true,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: false,
            max_docs: 50,
        }
    }
}

impl MaintainRefreshFlags {
    /// Build flags from optional MCP/tool parameters.
    ///
    /// Any explicitly provided flag overrides the default. When **no** action
    /// flags are provided at all (`None` for all four), safe defaults apply
    /// (fts + dirty graph + wiki index; no reembed). When at least one action
    /// is explicitly set, unspecified actions default to **false** so callers
    /// can run a single step.
    pub fn from_options(
        reindex_fts: Option<bool>,
        rebuild_graph: Option<bool>,
        graph_dirty_only: Option<bool>,
        rebuild_wiki_index: Option<bool>,
        reembed_all: Option<bool>,
        dry_run: Option<bool>,
        max_docs: usize,
    ) -> Self {
        let any_action_set = reindex_fts.is_some()
            || rebuild_graph.is_some()
            || rebuild_wiki_index.is_some()
            || reembed_all.is_some();

        let max_docs = max_docs.max(1);

        if any_action_set {
            Self {
                reindex_fts: reindex_fts.unwrap_or(false),
                rebuild_graph: rebuild_graph.unwrap_or(false),
                graph_dirty_only: graph_dirty_only.unwrap_or(true),
                rebuild_wiki_index: rebuild_wiki_index.unwrap_or(false),
                reembed_all: reembed_all.unwrap_or(false),
                dry_run: dry_run.unwrap_or(false),
                max_docs,
            }
        } else {
            Self {
                reindex_fts: true,
                rebuild_graph: true,
                graph_dirty_only: graph_dirty_only.unwrap_or(true),
                rebuild_wiki_index: true,
                reembed_all: false,
                dry_run: dry_run.unwrap_or(false),
                max_docs,
            }
        }
    }

    /// True when at least one whitelist action is requested.
    pub fn has_work(&self) -> bool {
        self.reindex_fts || self.rebuild_graph || self.rebuild_wiki_index || self.reembed_all
    }

    /// Ordered whitelist action names that will run.
    pub fn selected_actions(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.reindex_fts {
            out.push("reindex_fts");
        }
        if self.rebuild_graph {
            out.push("rebuild_graph");
        }
        if self.rebuild_wiki_index {
            out.push("rebuild_wiki_index");
        }
        if self.reembed_all {
            out.push("reembed_all");
        }
        out
    }
}

/// FTS reindex step result.
#[derive(Debug, Clone, Serialize)]
pub struct ReindexFtsReport {
    pub backend: String,
    pub stemmer: String,
    pub dry_run: bool,
}

/// Per-document graph rebuild line (best effort).
#[derive(Debug, Clone, Serialize)]
pub struct GraphDocResult {
    pub document_id: String,
    pub node_id: Option<String>,
    pub edge_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Graph rebuild step result.
#[derive(Debug, Clone, Serialize)]
pub struct GraphRebuildReport {
    pub dirty_only: bool,
    pub candidate_count: usize,
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_cap: usize,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<GraphDocResult>,
}

/// Wiki index rebuild step result.
#[derive(Debug, Clone, Serialize)]
pub struct WikiIndexRebuildReport {
    pub entry_count: usize,
    pub dry_run: bool,
}

/// One document reembed line.
#[derive(Debug, Clone, Serialize)]
pub struct ReembedDocResult {
    pub document_id: String,
    pub chunk_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Optional full-corpus reembed step result.
#[derive(Debug, Clone, Serialize)]
pub struct ReembedAllReport {
    pub documents_considered: usize,
    pub documents_processed: usize,
    pub documents_succeeded: usize,
    pub documents_failed: usize,
    pub skipped_cap: usize,
    pub chunks_reembedded: usize,
    pub dry_run: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<EmbeddingManifest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub documents: Vec<ReembedDocResult>,
}

/// Aggregate report for `maintain_refresh`.
#[derive(Debug, Clone, Serialize)]
pub struct MaintainRefreshReport {
    pub dry_run: bool,
    pub actions: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reindex_fts: Option<ReindexFtsReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_graph: Option<GraphRebuildReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rebuild_wiki_index: Option<WikiIndexRebuildReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reembed_all: Option<ReembedAllReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_log_seq: Option<i64>,
}

/// Rebuild the FTS / BM25 index on `chunks` (read-your-writes).
///
/// Uses `stemmer` when provided; otherwise the last meta stemmer / `"porter"`.
pub fn reindex_fts(store: &Store, stemmer: Option<&str>) -> Result<ReindexFtsReport> {
    let conn = store.lock()?;
    let state = match stemmer {
        Some(s) if !s.trim().is_empty() => db::reindex_with_stemmer(&conn, s)?,
        _ => db::reindex(&conn)?,
    };
    Ok(ReindexFtsReport {
        backend: state.backend.as_str().to_string(),
        stemmer: state.stemmer,
        dry_run: false,
    })
}

/// Rebuild object-graph slices for all documents or only dirty ones (best effort).
///
/// **Dirty** = no graph node with `document_id` matching the document.
/// Individual document failures are recorded and do not abort the batch.
/// Processing is capped at `max_docs` (must be ≥ 1).
pub fn rebuild_graph_for_all_or_dirty(
    store: &Store,
    dirty_only: bool,
    max_docs: usize,
) -> Result<GraphRebuildReport> {
    let max_docs = max_docs.max(1);
    let docs = store.list_documents()?;
    let mut candidates = Vec::new();

    for doc in docs {
        if dirty_only {
            match store.find_node_by_document_id(&doc.id) {
                Ok(Some(_)) => continue,
                Ok(None) => candidates.push(doc),
                Err(e) => {
                    // Best effort: treat lookup failure as dirty and try rebuild.
                    tracing::warn!(
                        document_id = %doc.id,
                        error = %e,
                        "find_node_by_document_id failed; treating as dirty"
                    );
                    candidates.push(doc);
                }
            }
        } else {
            candidates.push(doc);
        }
    }

    let candidate_count = candidates.len();
    let skipped_cap = candidate_count.saturating_sub(max_docs);
    let to_process: Vec<_> = candidates.into_iter().take(max_docs).collect();

    let mut documents = Vec::with_capacity(to_process.len());
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for doc in &to_process {
        match rebuild_document_graph(store, doc) {
            Ok((node_id, edge_count)) => {
                succeeded += 1;
                documents.push(GraphDocResult {
                    document_id: doc.id.clone(),
                    node_id: Some(node_id),
                    edge_count,
                    error: None,
                });
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    document_id = %doc.id,
                    error = %e,
                    "rebuild_document_graph failed (best effort continue)"
                );
                documents.push(GraphDocResult {
                    document_id: doc.id.clone(),
                    node_id: None,
                    edge_count: 0,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    Ok(GraphRebuildReport {
        dirty_only,
        candidate_count,
        processed: to_process.len(),
        succeeded,
        failed,
        skipped_cap,
        dry_run: false,
        documents,
    })
}

/// Clear and rebuild `wiki_index` from all `layer=wiki` documents.
pub fn rebuild_wiki_index(store: &Store) -> Result<WikiIndexRebuildReport> {
    let entry_count = store.rebuild_wiki_index_from_docs()?;
    Ok(WikiIndexRebuildReport {
        entry_count,
        dry_run: false,
    })
}

/// Re-embed chunks for every document (capped by `max_docs`) with the live embedder.
///
/// Updates `embedding_manifest` from config when at least one document was
/// processed successfully (or the corpus is empty and we only sync manifest).
/// Does **not** refuse on prior dim mismatch — that is the migration path.
pub async fn reembed_all(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    max_docs: usize,
) -> Result<ReembedAllReport> {
    let max_docs = max_docs.max(1);
    let docs = store.list_documents()?;
    let documents_considered = docs.len();
    let skipped_cap = documents_considered.saturating_sub(max_docs);
    let to_process: Vec<_> = docs.into_iter().take(max_docs).collect();

    let mut documents = Vec::with_capacity(to_process.len());
    let mut documents_succeeded = 0usize;
    let mut documents_failed = 0usize;
    let mut chunks_reembedded = 0usize;

    for doc in &to_process {
        match reembed_one_document(store, embedder, &doc.id).await {
            Ok(chunk_count) => {
                documents_succeeded += 1;
                chunks_reembedded += chunk_count;
                documents.push(ReembedDocResult {
                    document_id: doc.id.clone(),
                    chunk_count,
                    error: None,
                });
            }
            Err(e) => {
                documents_failed += 1;
                tracing::warn!(
                    document_id = %doc.id,
                    error = %e,
                    "reembed document failed (best effort continue)"
                );
                documents.push(ReembedDocResult {
                    document_id: doc.id.clone(),
                    chunk_count: 0,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Sync manifest when we touched the corpus successfully, or when empty
    // (align fingerprint for subsequent ingest).
    let manifest = if documents_failed == 0 || documents_succeeded > 0 {
        Some(store.write_embedding_manifest_from_config(config)?)
    } else {
        // All failed: leave old manifest so mismatch remains visible.
        store.get_embedding_manifest()?
    };

    Ok(ReembedAllReport {
        documents_considered,
        documents_processed: to_process.len(),
        documents_succeeded,
        documents_failed,
        skipped_cap,
        chunks_reembedded,
        dry_run: false,
        manifest,
        documents,
    })
}

async fn reembed_one_document(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    document_id: &str,
) -> Result<usize> {
    let doc = store
        .get_document(document_id)?
        .ok_or_else(|| AppError::not_found(format!("document not found: {document_id}")))?;

    let mut chunks = store.list_chunks_for_document(&doc.id)?;
    let chunk_count = chunks.len();
    if chunks.is_empty() {
        return Ok(0);
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = embedder.embed(&texts).await?;
    if embeddings.len() != chunks.len() {
        return Err(AppError::embeddings(format!(
            "embedder returned {} vectors for {} chunks",
            embeddings.len(),
            chunks.len()
        )));
    }
    for (chunk, emb) in chunks.iter_mut().zip(embeddings.into_iter()) {
        chunk.embedding = emb;
    }
    store.replace_chunks_for_document(&doc.id, &chunks)?;
    Ok(chunk_count)
}

/// Run whitelist refresh actions according to `flags`.
///
/// Always appends an `ops_log` row on apply (not on pure dry_run of zero work).
/// Dry-run still logs with `op=maintain_refresh_dry_run` so the timeline shows intent.
pub async fn maintain_refresh(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    flags: MaintainRefreshFlags,
) -> Result<MaintainRefreshReport> {
    if !flags.has_work() {
        return Err(AppError::config(
            "maintain_refresh: no actions selected; set reindex_fts, rebuild_graph, \
             rebuild_wiki_index, and/or reembed_all",
        ));
    }

    // Safety: only whitelist action names appear in the report.
    let actions: Vec<String> = flags
        .selected_actions()
        .into_iter()
        .filter(|a| REFRESH_ACTION_WHITELIST.contains(a))
        .map(|s| s.to_string())
        .collect();

    let dry_run = flags.dry_run;
    let mut report = MaintainRefreshReport {
        dry_run,
        actions: actions.clone(),
        reindex_fts: None,
        rebuild_graph: None,
        rebuild_wiki_index: None,
        reembed_all: None,
        ops_log_id: None,
        ops_log_seq: None,
    };

    if flags.reindex_fts {
        if dry_run {
            let status = {
                let conn = store.lock()?;
                db::fts_status(&conn)?
            };
            report.reindex_fts = Some(ReindexFtsReport {
                backend: status
                    .as_ref()
                    .map(|s| s.backend.as_str().to_string())
                    .unwrap_or_else(|| "unknown".into()),
                stemmer: status
                    .map(|s| s.stemmer)
                    .unwrap_or_else(|| config.fts_stemmer.clone()),
                dry_run: true,
            });
        } else {
            report.reindex_fts = Some(reindex_fts(store, Some(&config.fts_stemmer))?);
        }
    }

    if flags.rebuild_graph {
        if dry_run {
            let docs = store.list_documents()?;
            let mut candidate_count = 0usize;
            for doc in &docs {
                if flags.graph_dirty_only {
                    if store.find_node_by_document_id(&doc.id)?.is_none() {
                        candidate_count += 1;
                    }
                } else {
                    candidate_count += 1;
                }
            }
            let processed = candidate_count.min(flags.max_docs);
            report.rebuild_graph = Some(GraphRebuildReport {
                dirty_only: flags.graph_dirty_only,
                candidate_count,
                processed,
                succeeded: 0,
                failed: 0,
                skipped_cap: candidate_count.saturating_sub(flags.max_docs),
                dry_run: true,
                documents: Vec::new(),
            });
        } else {
            report.rebuild_graph = Some(rebuild_graph_for_all_or_dirty(
                store,
                flags.graph_dirty_only,
                flags.max_docs,
            )?);
        }
    }

    if flags.rebuild_wiki_index {
        if dry_run {
            let n = store.list_documents_by_layer("wiki")?.len();
            report.rebuild_wiki_index = Some(WikiIndexRebuildReport {
                entry_count: n,
                dry_run: true,
            });
        } else {
            report.rebuild_wiki_index = Some(rebuild_wiki_index(store)?);
        }
    }

    if flags.reembed_all {
        if dry_run {
            let docs = store.list_documents()?;
            let n = docs.len();
            let mut chunk_est = 0usize;
            for doc in docs.iter().take(flags.max_docs) {
                chunk_est += store.list_chunks_for_document(&doc.id)?.len();
            }
            report.reembed_all = Some(ReembedAllReport {
                documents_considered: n,
                documents_processed: n.min(flags.max_docs),
                documents_succeeded: 0,
                documents_failed: 0,
                skipped_cap: n.saturating_sub(flags.max_docs),
                chunks_reembedded: chunk_est,
                dry_run: true,
                manifest: store.get_embedding_manifest()?,
                documents: Vec::new(),
            });
        } else {
            report.reembed_all = Some(
                reembed_all(store, embedder, config, flags.max_docs).await?,
            );
        }
    }

    // ops_log every run (including dry_run) for auditability.
    let op = if dry_run {
        "maintain_refresh_dry_run"
    } else {
        "maintain_refresh"
    };
    let message = if dry_run {
        format!(
            "dry_run maintain_refresh actions=[{}]",
            actions.join(",")
        )
    } else {
        format!("maintain_refresh actions=[{}]", actions.join(","))
    };
    let payload = serde_json::to_string(&report).unwrap_or_else(|_| "{}".into());
    let written = store.append_ops_log(&OpsLogEntry {
        id: Uuid::new_v4().to_string(),
        seq: 0,
        ts: Utc::now(),
        op: op.into(),
        prefix: Some("MAINT".into()),
        message,
        entity_id: None,
        entity_kind: Some("corpus".into()),
        payload_json: payload,
        agent_name: None,
    })?;
    report.ops_log_id = Some(written.id);
    report.ops_log_seq = Some(written.seq);

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::models::{Chunk, Document, SearchMode};
    use crate::wiki::{write_wiki_page, LAYER_WIKI};
    use std::path::PathBuf;

    fn test_config(db_path: PathBuf, dims: usize) -> Config {
        Config {
            db_path,
            embedding_provider: crate::config::EmbeddingProviderKind::Mock,
            embedding_base_url: "https://api.openai.com/v1".to_string(),
            embedding_api_key: String::new(),
            embedding_model: "mock".to_string(),
            embedding_dims: dims,
            chunk_size: 800,
            chunk_overlap: 120,
            default_top_k: 5,
            ingest_roots: Vec::new(),
            max_context_tokens: 4096,
            max_chunks_per_doc: 3,
            fts_stemmer: "porter".to_string(),
            default_search_mode: SearchMode::Vec,
            llm_base_url: "http://127.0.0.1:11434/v1".to_string(),
            llm_provider: crate::llm::LlmProviderKind::Ollama,
            llm_model: "llama3.2".to_string(),
            llm_api_key: "ollama".to_string(),
            llm_enabled: false,
            llm_timeout_secs: 120,
            llm_max_tokens: 4096,
            maint_max_docs: 50,
            maint_near_dup_threshold: 0.92,
            tool_surface: crate::mcp::ToolSurface::Full,
            http_bind: None,
            wiki_require_if_match: false,
        }
    }

    fn seed_doc(store: &Store, id: &str, title: &str, content: &str, layer: &str) {
        let now = Utc::now();
        store
            .upsert_document(&Document {
                id: id.into(),
                uri: format!("text://{id}"),
                title: title.into(),
                content: content.into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: layer.into(),
                kind: if layer == LAYER_WIKI {
                    "wiki".into()
                } else {
                    "document".into()
                },
                ..Default::default()
            })
            .unwrap();
        store
            .insert_chunks(&[Chunk {
                id: format!("{id}-c0"),
                document_id: id.into(),
                chunk_index: 0,
                content: content.into(),
                embedding: vec![0.1; 8],
                char_start: 0,
                char_end: content.len() as i32,
            }])
            .unwrap();
    }

    #[test]
    fn whitelist_contains_expected_actions() {
        for a in [
            "reindex_fts",
            "rebuild_graph",
            "rebuild_wiki_index",
            "reembed_all",
        ] {
            assert!(REFRESH_ACTION_WHITELIST.contains(&a));
        }
    }

    #[test]
    fn flags_default_safe_suite_when_none_set() {
        let f = MaintainRefreshFlags::from_options(None, None, None, None, None, None, 50);
        assert!(f.reindex_fts);
        assert!(f.rebuild_graph);
        assert!(f.graph_dirty_only);
        assert!(f.rebuild_wiki_index);
        assert!(!f.reembed_all);
    }

    #[test]
    fn flags_explicit_single_action() {
        let f = MaintainRefreshFlags::from_options(
            Some(true),
            None,
            None,
            None,
            None,
            Some(true),
            10,
        );
        assert!(f.reindex_fts);
        assert!(!f.rebuild_graph);
        assert!(!f.rebuild_wiki_index);
        assert!(!f.reembed_all);
        assert!(f.dry_run);
        assert_eq!(f.max_docs, 10);
    }

    #[test]
    fn reindex_fts_and_wiki_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refresh.duckdb");
        let store = Store::open(&path).unwrap();
        seed_doc(&store, "d1", "Alpha", "hello world [[Beta]] #tag", "raw");
        seed_doc(&store, "w1", "Wiki One", "compiled page about alpha", "wiki");

        let fts = reindex_fts(&store, Some("porter")).unwrap();
        assert!(!fts.backend.is_empty());
        assert_eq!(fts.stemmer, "porter");

        let idx = rebuild_wiki_index(&store).unwrap();
        assert_eq!(idx.entry_count, 1);
        assert_eq!(store.list_wiki_index().unwrap().len(), 1);
    }

    #[test]
    fn rebuild_graph_dirty_then_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("graph.duckdb");
        let store = Store::open(&path).unwrap();
        seed_doc(&store, "d1", "Alpha", "see [[Beta]] and #topic", "raw");
        seed_doc(&store, "d2", "Gamma", "plain text no links", "raw");

        // Dirty: neither has a document node.
        let r1 = rebuild_graph_for_all_or_dirty(&store, true, 50).unwrap();
        assert_eq!(r1.candidate_count, 2);
        assert_eq!(r1.succeeded, 2);
        assert!(store.find_node_by_document_id("d1").unwrap().is_some());

        // Dirty again: none left.
        let r2 = rebuild_graph_for_all_or_dirty(&store, true, 50).unwrap();
        assert_eq!(r2.candidate_count, 0);
        assert_eq!(r2.processed, 0);

        // All: rebuild both again.
        let r3 = rebuild_graph_for_all_or_dirty(&store, false, 50).unwrap();
        assert_eq!(r3.candidate_count, 2);
        assert_eq!(r3.succeeded, 2);
    }

    #[test]
    fn graph_respects_max_docs_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cap.duckdb");
        let store = Store::open(&path).unwrap();
        for i in 0..5 {
            seed_doc(
                &store,
                &format!("d{i}"),
                &format!("Doc{i}"),
                "content",
                "raw",
            );
        }
        let r = rebuild_graph_for_all_or_dirty(&store, false, 2).unwrap();
        assert_eq!(r.candidate_count, 5);
        assert_eq!(r.processed, 2);
        assert_eq!(r.skipped_cap, 3);
        assert_eq!(r.succeeded, 2);
    }

    #[tokio::test]
    async fn maintain_refresh_dry_run_and_apply() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("maint.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 8usize;
        let config = test_config(path.clone(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        seed_doc(&store, "d1", "Alpha", "hello [[Beta]] #x", "raw");
        seed_doc(&store, "w1", "Wiki", "wiki body", "wiki");
        store
            .write_embedding_manifest_from_config(&config)
            .unwrap();

        let dry_flags = MaintainRefreshFlags {
            reindex_fts: true,
            rebuild_graph: true,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: true,
            max_docs: 50,
        };
        let dry = maintain_refresh(&store, &embedder, &config, dry_flags)
            .await
            .unwrap();
        assert!(dry.dry_run);
        assert!(dry.reindex_fts.as_ref().unwrap().dry_run);
        assert_eq!(dry.rebuild_graph.as_ref().unwrap().candidate_count, 2);
        // dry_run does not create nodes
        assert!(store.find_node_by_document_id("d1").unwrap().is_none());
        assert!(dry.ops_log_id.is_some());

        let apply_flags = MaintainRefreshFlags {
            reindex_fts: true,
            rebuild_graph: true,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: false,
            max_docs: 50,
        };
        let applied = maintain_refresh(&store, &embedder, &config, apply_flags)
            .await
            .unwrap();
        assert!(!applied.dry_run);
        assert_eq!(applied.rebuild_graph.as_ref().unwrap().succeeded, 2);
        assert_eq!(applied.rebuild_wiki_index.as_ref().unwrap().entry_count, 1);
        assert!(store.find_node_by_document_id("d1").unwrap().is_some());
        assert_eq!(store.list_wiki_index().unwrap().len(), 1);

        let ops = store.list_ops_log(10).unwrap();
        assert!(ops.iter().any(|o| o.op == "maintain_refresh"));
        assert!(ops.iter().any(|o| o.op == "maintain_refresh_dry_run"));
    }

    #[tokio::test]
    async fn reembed_all_updates_vectors_and_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("reembed.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 16usize;
        let config = test_config(path.clone(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        seed_doc(&store, "d1", "A", "reembed body one", "raw");
        // Wrong-dim chunks from seed (8); reembed should fix to 16.
        store
            .set_embedding_manifest(&EmbeddingManifest {
                id: "default".into(),
                provider: "mock".into(),
                model: "mock".into(),
                dims: 8,
                base_url: None,
                content_fingerprint: None,
                updated_at: Utc::now(),
            })
            .unwrap();

        let report = reembed_all(&store, &embedder, &config, 50).await.unwrap();
        assert_eq!(report.documents_succeeded, 1);
        assert!(report.chunks_reembedded >= 1);
        let man = report.manifest.unwrap();
        assert_eq!(man.dims, dims as i32);

        let chunks = store.list_chunks_for_document("d1").unwrap();
        assert!(chunks.iter().all(|c| c.embedding.len() == dims));
        store
            .require_embedding_dims_match(dims)
            .expect("dims match after reembed_all");
    }

    #[tokio::test]
    async fn write_wiki_then_refresh_index() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wiki.duckdb");
        let store = Store::open(&path).unwrap();
        let dims = 8usize;
        let config = test_config(path.clone(), dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        store
            .write_embedding_manifest_from_config(&config)
            .unwrap();

        write_wiki_page(
            &store,
            &embedder,
            &config,
            "alpha-page",
            "Alpha Page",
            "Body with [[Other]]",
            "wiki",
            Some("notes"),
            Some("summary line"),
            None,
        )
        .await
        .unwrap();

        // Wipe index and rebuild via refresh.
        {
            let conn = store.lock().unwrap();
            conn.execute("DELETE FROM wiki_index", []).unwrap();
        }
        assert!(store.list_wiki_index().unwrap().is_empty());

        let flags = MaintainRefreshFlags {
            reindex_fts: false,
            rebuild_graph: false,
            graph_dirty_only: true,
            rebuild_wiki_index: true,
            reembed_all: false,
            dry_run: false,
            max_docs: 50,
        };
        let r = maintain_refresh(&store, &embedder, &config, flags)
            .await
            .unwrap();
        assert_eq!(r.rebuild_wiki_index.unwrap().entry_count, 1);
    }
}
