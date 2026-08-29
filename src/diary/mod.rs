//! Per-agent chronological diary notes and session checkpoints.
//!
//! Diary documents use `layer=diary`, `kind=diary`, wing `agents/<name>`,
//! room `<agent_name>`. Content is stored verbatim (no summarize on write).

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::chunking::{from_config, Chunker};
use crate::config::Config;
use crate::db::Store;
use crate::embeddings::EmbeddingProvider;
use crate::error::{AppError, Result};
use crate::graph::rebuild_document_graph;
use crate::models::{
    Chunk, DiaryEntry, Document, DrawerListItem, OpsLogEntry, StatusReport, WakeUpReport,
    WakeUpSchemaSnippet,
};
use crate::util::content_hash;

/// Document layer for agent diary notes.
pub const LAYER_DIARY: &str = "diary";
/// Document kind for diary entries.
pub const KIND_DIARY: &str = "diary";
/// Legacy room name; new writes use `room = agent_name` for list filters.
pub const ROOM_DIARY: &str = "diary";

/// Result of writing a diary entry (includes embed/graph stats).
#[derive(Debug, Clone, Serialize)]
pub struct DiaryWriteResult {
    pub entry: DiaryEntry,
    pub chunk_count: usize,
    pub node_id: String,
    pub edge_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_log_id: Option<String>,
}

/// Result of `checkpoint`: always one ops_log row; optional diary write.
#[derive(Debug, Clone, Serialize)]
pub struct CheckpointResult {
    /// Session summary (verbatim).
    pub summary: String,
    /// Agent recorded on the ops_log / diary when provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    /// Appended ops_log row (`op=checkpoint`).
    pub ops_log: OpsLogEntry,
    /// Present when diary content was written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diary: Option<DiaryWriteResult>,
}

/// Normalize agent name: trim, lowercase, collapse unsafe path chars.
///
/// Empty after normalize → error.
pub fn normalize_agent_name(raw: &str) -> Result<String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() {
        return Err(AppError::config("agent_name must be non-empty"));
    }
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for ch in s.chars() {
        let ok = ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.';
        if ok {
            out.push(ch);
            prev_sep = false;
        } else if !prev_sep {
            out.push('_');
            prev_sep = true;
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        return Err(AppError::config(
            "agent_name must contain at least one alphanumeric character",
        ));
    }
    if out.len() > 128 {
        return Err(AppError::config("agent_name exceeds 128 characters"));
    }
    Ok(out)
}

/// Wing placement for an agent diary: `agents/<name>`.
pub fn agent_wing(agent_name: &str) -> String {
    format!("agents/{agent_name}")
}

/// Write a verbatim diary entry for `agent_name`.
///
/// Stores a new document each call (`layer=diary`, `kind=diary`, wing
/// `agents/<name>` unless `wing` override, room = agent name). Chunks + embeds.
/// Appends `ops_log` with `op=diary_write` unless `log_ops` is false.
pub async fn diary_write(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    agent_name: &str,
    content: &str,
    wing: Option<&str>,
    topic: Option<&str>,
    title: Option<&str>,
    log_ops: bool,
) -> Result<DiaryWriteResult> {
    let agent = normalize_agent_name(agent_name)?;
    let body = content.trim();
    if body.is_empty() {
        return Err(AppError::config("diary content must be non-empty"));
    }

    let topic = topic
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("general")
        .to_string();
    let wing = wing
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| agent_wing(&agent));
    let now = Utc::now();
    let hash = content_hash(body);
    let short = &hash[..12.min(hash.len())];
    let stamp = now.format("%Y%m%d_%H%M%S%.3f");
    let uri = format!("diary://{agent}/{stamp}-{short}");
    let title = title
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("{topic} @ {}", now.format("%Y-%m-%d %H:%M UTC")));

    let meta = serde_json::json!({
        "agent_name": agent,
        "topic": topic,
        "type": "diary_entry",
    });

    let document_id = Uuid::new_v4().to_string();
    // Convention (store list_diary_entries / wake_up): wing=agents/<name>, room=<name>.
    let doc = Document {
        id: document_id.clone(),
        uri: uri.clone(),
        title: title.clone(),
        content: body.to_string(),
        metadata_json: meta.to_string(),
        created_at: now,
        updated_at: now,
        wing: Some(wing.clone()),
        room: Some(agent.clone()),
        source_file: None,
        layer: LAYER_DIARY.into(),
        kind: KIND_DIARY.into(),
        content_hash: Some(hash),
        ..Default::default()
    };

    store.upsert_document(&doc)?;

    let chunk_count = embed_and_store_chunks(store, embedder, config, &doc).await?;
    let (node_id, edge_count) = rebuild_document_graph(store, &doc)?;

    let mut ops_log_id = None;
    if log_ops {
        let written = store.append_ops_log(&OpsLogEntry {
            id: String::new(),
            seq: 0,
            ts: now,
            op: "diary_write".into(),
            prefix: Some("DIARY".into()),
            message: format!("diary entry for agent '{agent}' topic={topic}"),
            entity_id: Some(document_id.clone()),
            entity_kind: Some(KIND_DIARY.into()),
            payload_json: serde_json::json!({
                "agent_name": agent,
                "topic": topic,
                "uri": uri,
                "wing": wing,
            })
            .to_string(),
            agent_name: Some(agent.clone()),
        })?;
        ops_log_id = Some(written.id);
    }

    Ok(DiaryWriteResult {
        entry: DiaryEntry {
            id: document_id,
            agent_name: agent,
            title,
            content: body.to_string(),
            wing,
            content_hash: doc.content_hash,
            created_at: now,
            updated_at: now,
        },
        chunk_count,
        node_id,
        edge_count,
        ops_log_id,
    })
}

/// Read recent diary entries for an agent (newest first).
///
/// Uses [`Store::list_diary_entries`] (`kind=diary`, wing `agents/<name>` or room).
pub fn diary_read(store: &Store, agent_name: &str, last_n: usize) -> Result<Vec<DiaryEntry>> {
    let agent = normalize_agent_name(agent_name)?;
    let limit = last_n.clamp(1, 100);
    store.list_diary_entries(Some(&agent), limit)
}

/// Session bootstrap: status + recent diary + pinned docs + optional schema snippet.
///
/// Does **not** auto-create `schema://agents` (only includes schema when present).
pub fn wake_up(
    store: &Store,
    status: StatusReport,
    agent_name: Option<&str>,
    diary_limit: usize,
    pinned_limit: usize,
) -> Result<WakeUpReport> {
    let agent_scope = match agent_name {
        Some(a) if !a.trim().is_empty() => Some(normalize_agent_name(a)?),
        _ => None,
    };
    let diary_limit = diary_limit.clamp(1, 100);
    let pinned_limit = pinned_limit.clamp(1, 200);

    let diary = store.list_diary_entries(agent_scope.as_deref(), diary_limit)?;
    let pinned_docs = store.list_pinned_documents(pinned_limit)?;
    let pinned: Vec<DrawerListItem> = pinned_docs.iter().map(DrawerListItem::from).collect();

    let schema = store
        .get_schema_document()?
        .map(|d| WakeUpSchemaSnippet::from_document(&d));

    Ok(WakeUpReport {
        status,
        diary,
        pinned,
        schema,
        agent_name: agent_scope,
    })
}

/// Session checkpoint: append `ops_log` with summary; optionally write diary.
///
/// When `diary` is non-empty, also calls [`diary_write`] (with its own ops_log).
/// `agent_name` defaults to `"agent"` only when diary content is present.
pub async fn checkpoint(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    summary: &str,
    diary: Option<&str>,
    agent_name: Option<&str>,
) -> Result<CheckpointResult> {
    let summary = summary.trim();
    if summary.is_empty() {
        return Err(AppError::config("checkpoint summary must be non-empty"));
    }

    let agent_opt = match agent_name {
        Some(a) if !a.trim().is_empty() => Some(normalize_agent_name(a)?),
        _ => None,
    };

    let diary_body = diary.map(str::trim).filter(|s| !s.is_empty());

    let diary_result = if let Some(body) = diary_body {
        let agent = agent_opt
            .clone()
            .unwrap_or_else(|| "agent".to_string());
        Some(
            diary_write(
                store,
                embedder,
                config,
                &agent,
                body,
                None,
                Some("session-checkpoint"),
                Some(&format!("checkpoint: {}", first_line(summary, 80))),
                true,
            )
            .await?,
        )
    } else {
        None
    };

    let agent_for_log = agent_opt
        .clone()
        .or_else(|| diary_result.as_ref().map(|d| d.entry.agent_name.clone()));

    let mut payload = serde_json::json!({
        "summary": summary,
        "has_diary": diary_result.is_some(),
    });
    if let Some(ref d) = diary_result {
        payload["diary_id"] = serde_json::Value::String(d.entry.id.clone());
        payload["diary_uri"] = serde_json::Value::String(format!(
            "diary://{}/{}",
            d.entry.agent_name, d.entry.id
        ));
    }

    let ops_log = store.append_ops_log(&OpsLogEntry {
        id: String::new(),
        seq: 0,
        ts: Utc::now(),
        op: "checkpoint".into(),
        prefix: Some("CKPT".into()),
        message: summary.to_string(),
        entity_id: diary_result.as_ref().map(|d| d.entry.id.clone()),
        entity_kind: diary_result.as_ref().map(|_| KIND_DIARY.to_string()),
        payload_json: payload.to_string(),
        agent_name: agent_for_log.clone(),
    })?;

    Ok(CheckpointResult {
        summary: summary.to_string(),
        agent_name: agent_for_log,
        ops_log,
        diary: diary_result,
    })
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().next().unwrap_or(s).trim();
    if line.chars().count() <= max {
        line.to_string()
    } else {
        let truncated: String = line.chars().take(max.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

async fn embed_and_store_chunks(
    store: &Store,
    embedder: &Arc<dyn EmbeddingProvider>,
    config: &Config,
    doc: &Document,
) -> Result<usize> {
    let chunker = from_config(config.chunk_size, config.chunk_overlap);
    let pieces: Vec<(String, i32, i32)> = Chunker::chunk(&chunker, &doc.content);
    if pieces.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = pieces.iter().map(|(c, _, _)| c.clone()).collect();
    let embeddings = embedder.embed(&texts).await?;
    if embeddings.len() != pieces.len() {
        return Err(AppError::embeddings(format!(
            "embedder returned {} vectors for {} chunks",
            embeddings.len(),
            pieces.len()
        )));
    }
    let mut chunks = Vec::with_capacity(pieces.len());
    for (i, ((content, char_start, char_end), embedding)) in
        pieces.into_iter().zip(embeddings.into_iter()).enumerate()
    {
        chunks.push(Chunk {
            id: Uuid::new_v4().to_string(),
            document_id: doc.id.clone(),
            chunk_index: i as i32,
            content,
            embedding,
            char_start,
            char_end,
            metadata_json: "{}".into(),
        });
    }
    let n = chunks.len();
    store.insert_chunks(&chunks)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::MockEmbedder;
    use crate::models::SearchMode;
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

    #[test]
    fn normalize_agent_name_rules() {
        assert_eq!(normalize_agent_name("Claude").unwrap(), "claude");
        assert_eq!(normalize_agent_name("  cursor-ide  ").unwrap(), "cursor-ide");
        assert_eq!(normalize_agent_name("My Agent").unwrap(), "my_agent");
        assert!(normalize_agent_name("   ").is_err());
        assert!(normalize_agent_name("@@@").is_err());
    }

    #[tokio::test]
    async fn diary_write_read_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("diary.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        let w1 = diary_write(
            &store,
            &embedder,
            &config,
            "Claude",
            "noticed the FTS index was empty",
            None,
            Some("observations"),
            None,
            true,
        )
        .await
        .expect("write1");
        assert_eq!(w1.entry.agent_name, "claude");
        assert_eq!(w1.entry.wing, "agents/claude");
        assert!(w1.chunk_count >= 1);
        assert!(w1.ops_log_id.is_some());

        let w2 = diary_write(
            &store,
            &embedder,
            &config,
            "claude",
            "second note same agent",
            None,
            None,
            None,
            true,
        )
        .await
        .expect("write2");
        assert_ne!(w1.entry.id, w2.entry.id);

        let w3 = diary_write(
            &store,
            &embedder,
            &config,
            "claude",
            "custom wing note",
            Some("custom/wings/claude"),
            Some("custom"),
            None,
            true,
        )
        .await
        .expect("write3");
        assert_eq!(w3.entry.wing, "custom/wings/claude");

        let read = diary_read(&store, "CLAUDE", 10).expect("read");
        assert!(read.len() >= 2);
        assert!(read.iter().any(|e| e.id == w2.entry.id));
        assert!(read
            .iter()
            .any(|e| e.content == "noticed the FTS index was empty"));

        let ops = store.list_ops_log(10).unwrap();
        assert!(ops.iter().any(|o| o.op == "diary_write"));
    }

    #[tokio::test]
    async fn checkpoint_ops_log_and_optional_diary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("ckpt.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        let r1 = checkpoint(
            &store,
            &embedder,
            &config,
            "session mid-point: wired hybrid search",
            None,
            Some("worker"),
        )
        .await
        .expect("ckpt1");
        assert_eq!(r1.ops_log.op, "checkpoint");
        assert_eq!(r1.ops_log.message, "session mid-point: wired hybrid search");
        assert_eq!(r1.agent_name.as_deref(), Some("worker"));
        assert!(r1.diary.is_none());

        let r2 = checkpoint(
            &store,
            &embedder,
            &config,
            "end of session: diary + ops",
            Some("wrapped hybrid search and left tests green"),
            Some("worker"),
        )
        .await
        .expect("ckpt2");
        assert!(r2.diary.is_some());
        let d = r2.diary.as_ref().unwrap();
        assert_eq!(d.entry.agent_name, "worker");
        assert!(d.entry.content.contains("hybrid search"));

        let read = diary_read(&store, "worker", 5).unwrap();
        assert_eq!(read.len(), 1);

        let ops = store.list_ops_log(20).unwrap();
        let ckpts: Vec<_> = ops.iter().filter(|o| o.op == "checkpoint").collect();
        assert_eq!(ckpts.len(), 2);
        assert!(ops.iter().any(|o| o.op == "diary_write"));
    }

    #[tokio::test]
    async fn checkpoint_rejects_empty_summary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("empty.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 8usize;
        let config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));
        let err = checkpoint(&store, &embedder, &config, "  ", None, None)
            .await
            .expect_err("empty");
        assert!(err.to_string().contains("summary"));
    }

    #[tokio::test]
    async fn wake_up_status_diary_pinned_optional_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("wake.duckdb");
        let store = Store::open(&path).expect("open");
        let dims = 32usize;
        let config = test_config(path, dims);
        let embedder: Arc<dyn EmbeddingProvider> = Arc::new(MockEmbedder::new(dims));

        diary_write(
            &store,
            &embedder,
            &config,
            "alice",
            "session start notes",
            None,
            None,
            None,
            true,
        )
        .await
        .expect("diary");

        let now = Utc::now();
        store
            .upsert_document(&Document {
                id: "pin-1".into(),
                uri: "wiki://pinned".into(),
                title: "Pinned page".into(),
                content: "important".into(),
                metadata_json: "{}".into(),
                created_at: now,
                updated_at: now,
                layer: "wiki".into(),
                kind: "wiki".into(),
                pinned: true,
                boost: 3.0,
                ..Default::default()
            })
            .unwrap();

        let status = StatusReport {
            backend: "duckdb".into(),
            schema_version: 4,
            fts_ready: false,
            document_count: 2,
            chunk_count: 1,
            node_count: 0,
            edge_count: 0,
            raw_count: 0,
            wiki_count: 1,
            index_entry_count: 0,
            index_coverage: 0.0,
            uncompiled_raw_count: 0,
            embedding_manifest_match: true,
            embed_provider: "mock".into(),
            embed_model: "mock".into(),
            wings: Vec::new(),
            embed_dims: dims,
            ready_for_search: true,
            ingest_roots_configured: false,
            db_path: store.path().display().to_string(),
        };
        let report = wake_up(&store, status.clone(), Some("alice"), 5, 20).expect("wake");
        assert_eq!(report.agent_name.as_deref(), Some("alice"));
        assert_eq!(report.diary.len(), 1);
        assert_eq!(report.diary[0].content, "session start notes");
        assert_eq!(report.pinned.len(), 1);
        assert_eq!(report.pinned[0].title, "Pinned page");
        assert!(report.schema.is_none());

        crate::wiki::update_schema(&store, "# conventions\n- be kind\n", None, None).unwrap();
        let report2 = wake_up(&store, status, Some("alice"), 5, 20).expect("wake2");
        assert!(report2.schema.is_some());
        assert!(report2
            .schema
            .as_ref()
            .unwrap()
            .content
            .contains("be kind"));
    }
}
