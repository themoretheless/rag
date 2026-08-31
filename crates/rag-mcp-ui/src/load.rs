//! Snapshot / vault graph.json loaders (Mode C) and exclusive live Store open (Mode A).
//! Dual-live DuckDB write with MCP is forbidden forever.

use rag_mcp::{
    GraphEdge, GraphFilter, GraphNode, GraphView, Store, PKB_REL_TYPES, UI_GRAPH_EXPORT_MAX_NODES,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Hard layout caps (EGUI_GRAPH_VIEW §8.1).
pub const UI_HARD_MAX_NODES: usize = 300;
pub const UI_MAX_DRAW_EDGES: usize = 2000;
pub const UI_LOCAL_MAX_NODES: u32 = 100;
pub const UI_DEFAULT_DEPTH: u32 = 1;

/// CLI open mode (XOR snapshot | db | http service).
#[derive(Debug, Clone)]
pub enum CliSource {
    Snapshot(PathBuf),
    Db(PathBuf),
    /// Base URL of rag-mcp HTTP API (`RAG_HTTP_BIND`), e.g. `http://127.0.0.1:7432`.
    Http(String),
}

/// Server-side `get_graph` default node cap (GRAPH_EGUI_DECISIONS / graph export).
pub const EXPORT_DEFAULT_MAX_NODES: u32 = 500;

/// Top-level CLI: optional `export` subcommand, else GUI open flags.
#[derive(Debug, Clone, clap::Parser)]
#[command(
    name = "rag-mcp-ui",
    about = "Optional read-only GraphView inspector (snapshot default; dual-live MCP write unsupported)",
    subcommand_negates_reqs = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    #[command(flatten)]
    pub open: OpenArgs,
}

/// Subcommands (headless Mode C tooling).
#[derive(Debug, Clone, clap::Subcommand)]
pub enum Commands {
    /// Export GraphView topology JSON from DuckDB for Mode C (`--snapshot` open path).
    ///
    /// Opens the DB exclusively via `Store`, dumps topology-only nodes/edges (no positions),
    /// writes pretty JSON, and exits without starting the GUI. Prefer this over dual-live
    /// DuckDB while MCP holds the file.
    Export(ExportArgs),
}

/// `rag-mcp-ui export --db PATH [-o graph.json]` arguments.
#[derive(Debug, Clone, clap::Args)]
pub struct ExportArgs {
    /// DuckDB path to read (exclusive open; fail if another process holds the file).
    #[arg(long)]
    pub db: PathBuf,

    /// Output path for GraphView JSON (default: graph.json).
    #[arg(long, short = 'o', default_value = "graph.json")]
    pub output: PathBuf,

    /// Max nodes to export (default 500, same as MCP `get_graph`).
    #[arg(long, default_value_t = EXPORT_DEFAULT_MAX_NODES)]
    pub max_nodes: u32,

    /// Restrict edge rel_types (comma-separated). Default: all types (full dump).
    #[arg(long, value_delimiter = ',')]
    pub rel_types: Option<Vec<String>>,

    /// Use PKB default rel_types only: wikilink,related.
    #[arg(long, conflicts_with = "rel_types")]
    pub pkb: bool,

    /// Restrict node kinds (comma-separated: document,tag,stub,entity).
    #[arg(long, value_delimiter = ',')]
    pub kinds: Option<Vec<String>>,

    /// Optional seed node ids (export only those nodes and edges between them).
    #[arg(long, value_delimiter = ',')]
    pub seed_ids: Option<Vec<String>>,

    /// Write optional envelope `{version, source, db_path, graph}` (EGUI_GRAPH_VIEW §10.1).
    #[arg(long)]
    pub envelope: bool,
}

/// Parsed CLI open args (GUI path; no subcommand).
#[derive(Debug, Clone, clap::Args)]
pub struct OpenArgs {
    /// Path to GraphView JSON snapshot (Mode C).
    #[arg(long)]
    pub snapshot: Option<PathBuf>,

    /// Exclusive DuckDB path (Mode A). Fails clearly if the file cannot be opened.
    #[arg(long)]
    pub db: Option<PathBuf>,

    /// rag-mcp HTTP API base (Mode S — same process as MCP holds DB).
    /// Example: `http://127.0.0.1:7432` (server: `RAG_HTTP_BIND=127.0.0.1:7432`).
    #[arg(long)]
    pub http: Option<String>,

    /// Seed node id, label, or document_id (required before local paint).
    #[arg(long)]
    pub seed: Option<String>,

    /// Neighbor depth for local view (default 1, max 3).
    #[arg(long, default_value_t = UI_DEFAULT_DEPTH)]
    pub depth: u32,

    /// Max nodes in the local neighbor view (default 100, clamped to hard cap 300).
    #[arg(long, default_value_t = UI_LOCAL_MAX_NODES)]
    pub max_nodes: u32,

    /// Resolved exclusive source after validate (not a clap field).
    #[arg(skip)]
    pub source: Option<CliSource>,
}

impl OpenArgs {
    pub fn validate(&self) -> Result<(), String> {
        let n = usize::from(self.snapshot.is_some())
            + usize::from(self.db.is_some())
            + usize::from(self.http.as_ref().is_some_and(|s| !s.trim().is_empty()));
        if n > 1 {
            return Err(
                "use exactly one of --http, --snapshot, or --db (with live MCP prefer --http)"
                    .into(),
            );
        }
        Ok(())
    }

    /// Finalize source field after parse.
    pub fn with_source(mut self) -> Self {
        self.depth = self.depth.clamp(1, 3);
        // Keep local load under layout hard cap; floor at 1 so BFS is defined.
        self.max_nodes = self.max_nodes.clamp(1, UI_HARD_MAX_NODES as u32);
        self.source = match (
            &self.snapshot,
            &self.db,
            self.http.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty()),
        ) {
            (Some(p), None, None) => Some(CliSource::Snapshot(p.clone())),
            (None, Some(p), None) => Some(CliSource::Db(p.clone())),
            (None, None, Some(url)) => Some(CliSource::Http(url.to_string())),
            _ => None,
        };
        self
    }
}

/// Where the topology came from (status line).
#[derive(Debug, Clone)]
pub enum GraphSourceKind {
    LiveStore { path: PathBuf },
    SnapshotFile {
        path: PathBuf,
        mtime: Option<SystemTime>,
    },
    VaultGraphJson {
        path: PathBuf,
        mtime: Option<SystemTime>,
    },
    /// Loaded via rag-mcp HTTP API (same process as DuckDB writer).
    HttpService { base: String },
}

/// PKB default edge types for live Store load (EGUI_GRAPH_VIEW §7.1 / GRAPH_DESIGN §7.1).
/// Alias of library constant so UI and server stay aligned.
pub const PKB_DEFAULT_REL_TYPES: &[&str] = PKB_REL_TYPES;

impl GraphSourceKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::LiveStore { .. } => "live_store",
            Self::SnapshotFile { .. } => "snapshot",
            Self::VaultGraphJson { .. } => "vault_graph_json",
            Self::HttpService { .. } => "http_service",
        }
    }

    /// Filesystem path of the open source (db or snapshot). Empty for HTTP.
    pub fn path(&self) -> &Path {
        match self {
            Self::LiveStore { path }
            | Self::SnapshotFile { path, .. }
            | Self::VaultGraphJson { path, .. } => path.as_path(),
            Self::HttpService { .. } => Path::new(""),
        }
    }

    /// Snapshot / vault-json mtime when known (None for live store / http).
    pub fn mtime(&self) -> Option<SystemTime> {
        match self {
            Self::LiveStore { .. } | Self::HttpService { .. } => None,
            Self::SnapshotFile { mtime, .. } | Self::VaultGraphJson { mtime, .. } => *mtime,
        }
    }
}

/// Result of a successful topology load.
#[derive(Debug, Clone)]
pub struct LoadedGraph {
    pub view: GraphView,
    pub source: GraphSourceKind,
    pub truncated: bool,
    pub raw_node_count: usize,
    pub health: Option<GatewayHealth>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct GatewayHealth {
    pub backend: String,
    pub schema_version: i32,
    pub fts_ready: bool,
    pub documents: u64,
    pub chunks: u64,
    pub wal_bytes: u64,
    pub wal_warn_bytes: u64,
    pub wal_too_large: bool,
    pub documents_without_chunks: u64,
    pub relational_integrity_ok: bool,
}

/// Optional envelope around bare `GraphView` (EGUI_GRAPH_VIEW §10.1).
#[derive(Debug, Deserialize)]
struct SnapshotEnvelope {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    graph: Option<GraphView>,
    #[serde(default)]
    nodes: Option<Vec<GraphNode>>,
    #[serde(default)]
    edges: Option<Vec<GraphEdge>>,
}

/// Load a GraphView JSON snapshot from disk (read-only).
///
/// Accepts bare `GraphView` serde JSON (same shape as MCP `get_graph` / models)
/// or the optional envelope in EGUI_GRAPH_VIEW §10.1.
pub fn load_snapshot_path(path: &Path) -> Result<LoadedGraph, String> {
    if !path.exists() {
        return Err(format!(
            "cannot open snapshot: path does not exist: {}",
            path.display()
        ));
    }
    let bytes = fs::read(path).map_err(|e| {
        format!(
            "cannot open snapshot {}: {e}",
            path.display()
        )
    })?;
    let view = parse_graph_json(&bytes).map_err(|e| format!("parse {}: {e}", path.display()))?;
    let raw_node_count = view.nodes.len();
    let truncated = raw_node_count > UI_HARD_MAX_NODES;
    let mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let source = if path.file_name().and_then(|s| s.to_str()) == Some("graph.json") {
        GraphSourceKind::VaultGraphJson {
            path: path.to_path_buf(),
            mtime,
        }
    } else {
        GraphSourceKind::SnapshotFile {
            path: path.to_path_buf(),
            mtime,
        }
    };
    Ok(LoadedGraph {
        view,
        source,
        truncated,
        raw_node_count,
        health: None,
    })
}

/// Document body for graph UI "Read content" (from HTTP or exclusive DB).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DocumentBody {
    pub id: String,
    pub uri: String,
    pub title: String,
    pub layer: String,
    pub kind: String,
    pub content: String,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub revision: Option<i64>,
    #[serde(default)]
    pub etag: Option<String>,
}

/// Defaults for partial JSON (e.g. PUT write-result that omits content/title).
#[derive(Debug, Clone, Copy, Default)]
struct DocumentBodyJsonDefaults<'a> {
    content: Option<&'a str>,
    title: Option<&'a str>,
    uri: Option<&'a str>,
    /// Used when JSON has no `revision` (e.g. optimistic next rev after CAS).
    revision: Option<i64>,
}

/// Build [`DocumentBody`] from API JSON: full body, nested `document`, or flat fields.
///
/// Shared by GET document fetch and PUT wiki response parsing.
fn document_body_from_json(
    v: &serde_json::Value,
    defaults: DocumentBodyJsonDefaults<'_>,
) -> Option<DocumentBody> {
    if let Ok(body) = serde_json::from_value::<DocumentBody>(v.clone()) {
        if !body.id.is_empty() {
            return Some(body);
        }
    }
    if let Some(doc) = v.get("document") {
        if let Ok(body) = serde_json::from_value::<DocumentBody>(doc.clone()) {
            if !body.id.is_empty() {
                return Some(body);
            }
        }
    }
    // Flat write-result / partial echo: require id (or document_id) and content.
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("document_id").and_then(|x| x.as_str()))?;
    if id.is_empty() {
        return None;
    }
    let content = v
        .get("content")
        .and_then(|x| x.as_str())
        .or(defaults.content)?;
    let json_revision = v.get("revision").and_then(|x| x.as_i64());
    Some(DocumentBody {
        id: id.to_string(),
        uri: v
            .get("uri")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| defaults.uri.map(|s| s.to_string()))
            .unwrap_or_default(),
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .or(defaults.title)
            .unwrap_or("")
            .to_string(),
        layer: v
            .get("layer")
            .and_then(|x| x.as_str())
            .unwrap_or("wiki")
            .to_string(),
        kind: v
            .get("kind")
            .and_then(|x| x.as_str())
            .unwrap_or("wiki")
            .to_string(),
        content: content.to_string(),
        content_hash: v
            .get("content_hash")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        updated_at: v
            .get("updated_at")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        revision: json_revision.or(defaults.revision),
        // Prefer JSON etag; else derive from JSON revision only (not defaulted rev).
        etag: v
            .get("etag")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| json_revision.map(|r| format!("W/\"{r}\""))),
    })
}

/// Wiki catalog entry for sidebar (Obsidian-like page list).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct WikiPageMeta {
    pub id: String,
    pub uri: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub revision: i64,
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct WikiListResponse {
    #[serde(default)]
    pages: Vec<WikiPageMeta>,
    #[serde(default)]
    #[allow(dead_code)]
    count: usize,
}

/// Backlink row from `GET /v1/backlinks?id=`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BacklinkItem {
    pub label: String,
    pub id: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct BacklinksResponse {
    #[serde(default)]
    backlinks: Vec<BacklinkItem>,
}

// ---------------------------------------------------------------------------
// HTTP transport seam (DIP)
//
// Free helpers below (`fetch_*_http`, `put_wiki_http`, `load_http`) are the
// application-facing API. They currently own reqwest::blocking directly.
//
// Future: introduce a small `HttpClient` trait (get/put JSON + status/body)
// above these helpers, with a reqwest impl as default and a fake for tests.
// Public function signatures stay stable; only the transport injection point
// moves. Do not split modules until that trait lands and call sites need it.
// ---------------------------------------------------------------------------

/// Normalize gateway base URL (trim whitespace and trailing `/`).
fn normalize_http_base(base: &str) -> &str {
    base.trim().trim_end_matches('/')
}

/// Join gateway base with a path or `path?query` (leading `/` on path optional).
fn http_join(base: &str, path_and_query: &str) -> String {
    let base = normalize_http_base(base);
    let rest = path_and_query.trim_start_matches('/');
    format!("{base}/{rest}")
}

/// Fetch incoming links for a document id.
pub fn fetch_backlinks_http(base: &str, document_id: &str) -> Result<Vec<BacklinkItem>, String> {
    let url = http_join(
        base,
        &format!("v1/backlinks?id={}", urlencoding_minimal(document_id)),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("HTTP GET {url}: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!("HTTP {status}: {}", body.chars().take(200).collect::<String>()));
    }
    let body: BacklinksResponse = resp.json().map_err(|e| format!("parse backlinks: {e}"))?;
    Ok(body.backlinks)
}

/// Fetch wiki page catalog via `GET /v1/wiki`.
pub fn fetch_wiki_list_http(base: &str) -> Result<Vec<WikiPageMeta>, String> {
    let url = http_join(base, "v1/wiki");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("HTTP GET {url} failed: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    let body: WikiListResponse = resp
        .json()
        .map_err(|e| format!("parse wiki list from {url}: {e}"))?;
    let mut pages = body.pages;
    sort_wiki_pages(&mut pages);
    Ok(pages)
}

/// List wiki pages from exclusive DuckDB open (metadata only).
pub fn fetch_wiki_list_db(db_path: &Path) -> Result<Vec<WikiPageMeta>, String> {
    let store = Store::open(db_path).map_err(|e| format!("open db: {e}"))?;
    let items = store.list_wiki_page_metas().map_err(|e| e.to_string())?;
    let mut pages: Vec<WikiPageMeta> = items
        .into_iter()
        .map(|d| WikiPageMeta {
            id: d.id,
            uri: d.uri,
            slug: d.slug,
            title: d.title,
            kind: d.kind,
            summary: d.summary,
            category: d.category,
            revision: d.revision,
            etag: Some(d.etag),
            updated_at: Some(d.updated_at),
        })
        .collect();
    sort_wiki_pages(&mut pages);
    Ok(pages)
}

/// Fetch full document text via `GET /v1/document` on the rag-mcp HTTP gateway.
pub fn fetch_document_http(
    base: &str,
    document_id: Option<&str>,
    uri: Option<&str>,
    q: Option<&str>,
) -> Result<DocumentBody, String> {
    if normalize_http_base(base).is_empty() {
        return Err("http base URL is empty".into());
    }
    let mut parts = Vec::new();
    if let Some(id) = document_id.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("id={}", urlencoding_minimal(id)));
    } else if let Some(u) = uri.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("uri={}", urlencoding_minimal(u)));
    } else if let Some(key) = q.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("q={}", urlencoding_minimal(key)));
    } else {
        return Err("need document_id, uri, or label to load content".into());
    }
    let url = http_join(base, &format!("v1/document?{}", parts.join("&")));

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client.get(&url).send().map_err(|e| {
        format!("HTTP GET {url} failed: {e}. Is rag-mcp running with RAG_HTTP_BIND?")
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {status}: {}",
            body.chars().take(400).collect::<String>()
        ));
    }
    let v: serde_json::Value = resp
        .json()
        .map_err(|e| format!("parse DocumentBody from {url}: {e}"))?;
    document_body_from_json(&v, DocumentBodyJsonDefaults::default()).ok_or_else(|| {
        format!("parse DocumentBody from {url}: missing document fields")
    })
}

/// Load document body via exclusive DuckDB open (Mode A `--db` only).
pub fn fetch_document_db(
    db_path: &Path,
    document_id: Option<&str>,
    uri: Option<&str>,
) -> Result<DocumentBody, String> {
    let store = Store::open(db_path).map_err(|e| {
        format!(
            "cannot open DB {}: {e}. Prefer --http while MCP holds the file.",
            db_path.display()
        )
    })?;
    let doc = if let Some(id) = document_id.map(str::trim).filter(|s| !s.is_empty()) {
        store
            .get_document(id)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("document id '{id}' not found"))?
    } else if let Some(u) = uri.map(str::trim).filter(|s| !s.is_empty()) {
        store
            .find_by_uri(u)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("document uri '{u}' not found"))?
    } else {
        return Err("need document_id or uri".into());
    };
    let rev = doc.revision;
    let etag = doc.etag();
    Ok(DocumentBody {
        id: doc.id,
        uri: doc.uri,
        title: doc.title,
        layer: doc.layer,
        kind: doc.kind,
        content: doc.content,
        content_hash: doc.content_hash,
        updated_at: Some(doc.updated_at.to_rfc3339()),
        revision: Some(rev),
        etag: Some(etag),
    })
}

/// Body for `PUT /v1/wiki` (must match server `WikiPutBody`: slug-keyed write).
#[derive(Debug, Clone, Serialize)]
pub struct WikiPutRequest {
    /// Document UUID (used only for re-fetch after write; not sent if empty).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Wiki slug (required by gateway). Prefer explicit slug over parsing `uri`.
    pub slug: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    pub title: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_match_revision: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub if_match_etag: Option<String>,
}

/// Save wiki page content via HTTP `PUT /v1/wiki` (requires gateway write support).
///
/// Sends CAS fields when known. On 404/405, returns a clear message that the
/// server is read-only and the user should use `--db` or MCP `update_wiki_page`.
pub fn put_wiki_http(base: &str, req: &WikiPutRequest) -> Result<DocumentBody, String> {
    let base = normalize_http_base(base);
    if base.is_empty() {
        return Err("http base URL is empty".into());
    }
    let url = http_join(base, "v1/wiki");
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("http client: {e}"))?;

    let mut builder = client.put(&url).json(req);
    if let Some(etag) = req
        .if_match_etag
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        builder = builder.header("If-Match", etag);
    } else if let Some(rev) = req.if_match_revision {
        builder = builder.header("If-Match", format!("W/\"{rev}\""));
    }

    let resp = builder.send().map_err(|e| {
        format!("HTTP PUT {url} failed: {e}. Is rag-mcp running with RAG_HTTP_BIND?")
    })?;
    let status = resp.status();
    if status.as_u16() == 404 || status.as_u16() == 405 {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {status}: wiki write not available on this gateway (need PUT /v1/wiki). Use --db exclusive mode or MCP update_wiki_page. {}",
            body.chars().take(200).collect::<String>()
        ));
    }
    if status.as_u16() == 409 {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "conflict (revision mismatch): {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    if !status.is_success() {
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {status}: {}",
            body.chars().take(400).collect::<String>()
        ));
    }
    // Accept full DocumentBody or a write-result envelope with document fields.
    let text = resp
        .text()
        .map_err(|e| format!("read PUT response: {e}"))?;
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
        let defaults = DocumentBodyJsonDefaults {
            content: Some(req.content.as_str()),
            title: Some(req.title.as_str()),
            uri: req.uri.as_deref(),
            revision: req.if_match_revision.map(|r| r + 1),
        };
        if let Some(body) = document_body_from_json(&v, defaults) {
            return Ok(body);
        }
    }
    // Server accepted write but returned no body: re-fetch document.
    fetch_document_http(base, Some(&req.id), req.uri.as_deref(), None)
}

/// Save wiki page via exclusive DuckDB (`--db`). Uses CAS when `if_match_revision` is set.
///
/// Updates document title/content (not raw layer). Refreshes wiki index summary
/// from the first content line. Does not re-embed chunks (prefer MCP
/// `update_wiki_page` when search freshness matters).
pub fn save_wiki_db(
    db_path: &Path,
    document_id: &str,
    title: &str,
    content: &str,
    if_match_revision: Option<i64>,
) -> Result<DocumentBody, String> {
    let store = Store::open(db_path).map_err(|e| {
        format!(
            "cannot open DB {}: {e}. Prefer --http while MCP holds the file.",
            db_path.display()
        )
    })?;
    let doc = store
        .get_document(document_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("document id '{document_id}' not found"))?;

    if doc.layer == "raw" {
        return Err(format!(
            "document {document_id} is layer=raw (immutable body); refuse wiki editor save"
        ));
    }
    if let Some(expected) = if_match_revision {
        if doc.revision != expected {
            return Err(format!(
                "conflict: expected revision {expected}, current is {} (reload and retry)",
                doc.revision
            ));
        }
    }

    let title = {
        let t = title.trim();
        if t.is_empty() {
            doc.title.clone()
        } else {
            t.to_string()
        }
    };

    let applied = store
        .update_document_meta(
            document_id,
            &rag_mcp::DocumentMetaUpdate {
                title: Some(title),
                content: Some(content.to_string()),
                ..Default::default()
            },
        )
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("document id '{document_id}' vanished during save"))?;

    // Keep sidebar catalog summary roughly in sync (first non-empty line).
    let slug = applied
        .document
        .uri
        .strip_prefix("wiki://")
        .unwrap_or(applied.document.uri.as_str());
    if !slug.is_empty() {
        let summary = content
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(|l| {
                let chars: String = l.chars().take(240).collect();
                chars
            });
        let _ = store.update_wiki_index_entry_fields(
            slug,
            Some(applied.document.title.clone()),
            summary,
            Some(applied.document.kind.clone()),
            None,
            Some(applied.document.id.clone()),
        );
    }

    let rev = applied.document.revision;
    let etag = applied.document.etag();
    Ok(DocumentBody {
        id: applied.document.id,
        uri: applied.document.uri,
        title: applied.document.title,
        layer: applied.document.layer,
        kind: applied.document.kind,
        content: applied.document.content,
        content_hash: applied.document.content_hash,
        updated_at: Some(applied.document.updated_at.to_rfc3339()),
        revision: Some(rev),
        etag: Some(etag),
    })
}

/// Minimal query-string escape for path-ish ids/uris (no full url crate).
fn urlencoding_minimal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            b':' => out.push_str("%3A"),
            b'/' => out.push_str("%2F"),
            b'?' => out.push_str("%3F"),
            b'#' => out.push_str("%23"),
            b'&' => out.push_str("%26"),
            b'=' => out.push_str("%3D"),
            b'+' => out.push_str("%2B"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Load GraphView from rag-mcp HTTP API (same process holds DuckDB; no second open).
///
/// Server: `RAG_HTTP_BIND=127.0.0.1:7432`  
/// UI: `--http http://127.0.0.1:7432`
pub fn load_http(base: &str, _seed: Option<&str>, _depth: u32) -> Result<LoadedGraph, String> {
    let base = normalize_http_base(base);
    if base.is_empty() {
        return Err("--http base URL is empty".into());
    }
    let url = http_join(
        base,
        &format!("v1/graph?max_nodes={UI_GRAPH_EXPORT_MAX_NODES}&include_tags=false"),
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let health = client.get(http_join(base, "health")).send().ok()
        .filter(|response| response.status().is_success())
        .and_then(|response| response.json::<GatewayHealth>().ok());
    let resp = client.get(&url).send().map_err(|e| {
        format!(
            "HTTP GET {url} failed: {e}. Is rag-mcp running with RAG_HTTP_BIND set?"
        )
    })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        return Err(format!(
            "HTTP {status} from {url}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }
    let view: GraphView = resp
        .json()
        .map_err(|e| format!("parse GraphView from {url}: {e}"))?;
    let raw_node_count = view.nodes.len();
    let truncated = raw_node_count > UI_HARD_MAX_NODES;
    Ok(LoadedGraph {
        view,
        source: GraphSourceKind::HttpService {
            base: base.to_string(),
        },
        truncated,
        raw_node_count,
        health,
    })
}

/// Exclusive live Store open (Mode A): `Store::open` + [`Store::export_graph_for_ui`].
///
/// PKB defaults centralized on Store: `rel_types = [wikilink, related]`, tags
/// excluded, `max_nodes = 300` ([`UI_GRAPH_EXPORT_MAX_NODES`]).
/// Fails clearly if the path cannot be opened (missing file, lock, corrupt DB).
/// Dual-live write with a concurrent MCP process is unsupported forever; if open
/// fails while MCP holds the file, switch to `--snapshot` (Mode C).
///
/// `seed` / `depth` are not applied here; the full filtered topology is returned
/// and local seed BFS is done client-side (same as snapshot path).
pub fn load_live_db(
    path: &Path,
    _seed: Option<&str>,
    _depth: u32,
) -> Result<LoadedGraph, String> {
    if !path.exists() {
        return Err(format!(
            "cannot open database: path does not exist: {}. Dual-live write with MCP is forbidden; use --snapshot if the agent holds the file.",
            path.display()
        ));
    }

    let store = Store::open(path).map_err(|e| {
        format!(
            "cannot open exclusive DuckDB at {}: {}. If MCP (or another process) has this file open, close it and retry, or use --snapshot Mode C. Dual-live write is forbidden.",
            path.display(),
            e
        )
    })?;

    let view = store
        .export_graph_for_ui(Some(UI_GRAPH_EXPORT_MAX_NODES), false)
        .map_err(|e| {
            format!(
                "export_graph_for_ui failed on {}: {}. Check schema / graph tables.",
                path.display(),
                e
            )
        })?;

    let total_nodes = store
        .graph_stats()
        .map(|s| s.total_nodes as usize)
        .unwrap_or(view.nodes.len());
    let truncated = total_nodes > view.nodes.len()
        || view.nodes.len() >= UI_HARD_MAX_NODES
        || view.nodes.len() as u32 >= UI_GRAPH_EXPORT_MAX_NODES;

    Ok(LoadedGraph {
        view,
        source: GraphSourceKind::LiveStore {
            path: path.to_path_buf(),
        },
        truncated,
        raw_node_count: total_nodes,
        health: None,
    })
}

/// Live-load filter: PKB rel types, no tags, hard UI node cap.
///
/// Prefer [`Store::export_graph_for_ui`]; this mirrors
/// [`GraphFilter::pkb_ui_export`] for callers that only need the filter value.
#[allow(dead_code)] // public helper for tests / callers building filters without Store
pub fn pkb_live_filter() -> GraphFilter {
    GraphFilter::pkb_ui_export(Some(UI_HARD_MAX_NODES as u32), false)
}

/// Parse bare `GraphView` or envelope `{ "graph": { ... } }`.
///
/// Bare form matches `rag_mcp::models::GraphView` serde (MCP tool wire JSON).
pub fn parse_graph_json(bytes: &[u8]) -> Result<GraphView, String> {
    if let Ok(view) = serde_json::from_slice::<GraphView>(bytes) {
        return Ok(view);
    }
    let env: SnapshotEnvelope = serde_json::from_slice(bytes)
        .map_err(|e| format!("JSON is neither bare GraphView nor snapshot envelope: {e}"))?;
    if let Some(graph) = env.graph {
        return Ok(graph);
    }
    if let (Some(nodes), Some(edges)) = (env.nodes, env.edges) {
        return Ok(GraphView { nodes, edges });
    }
    if env.version.is_some() {
        return Ok(GraphView::default());
    }
    Err("snapshot JSON missing graph / nodes+edges".into())
}

/// Serialize a `GraphView` to the same serde JSON format as `rag_mcp` models / MCP tools.
pub fn export_graph_view_json(view: &GraphView) -> Result<String, String> {
    serde_json::to_string_pretty(view).map_err(|e| format!("serialize GraphView: {e}"))
}

/// Optional Mode C envelope written by `export --envelope` (EGUI_GRAPH_VIEW §10.1).
#[derive(Debug, Serialize)]
struct SnapshotEnvelopeOut<'a> {
    version: u32,
    source: &'static str,
    db_path: String,
    graph: &'a GraphView,
}

/// Write a topology-only GraphView snapshot file (Mode C refresh source).
pub fn write_graph_snapshot(path: &Path, view: &GraphView) -> Result<(), String> {
    let json = export_graph_view_json(view)?;
    write_snapshot_bytes(path, json.as_bytes())
}

fn write_snapshot_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "create snapshot parent {}: {e}",
                    parent.display()
                )
            })?;
        }
    }
    fs::write(path, bytes).map_err(|e| format!("write snapshot {}: {e}", path.display()))
}

/// Result of a successful CLI `export` run.
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub output: PathBuf,
    pub node_count: usize,
    pub edge_count: usize,
    pub truncated: bool,
}

/// Export GraphView topology from DuckDB to a Mode C snapshot file.
///
/// Uses exclusive `Store::open` (same writer rule as Mode A). Topology only: no
/// positions. Default `max_nodes` matches MCP `get_graph` (500). Dual-live write
/// with a concurrent MCP process is forbidden; if open fails while MCP holds the
/// file, close MCP first or export from a copy.
pub fn export_graph_snapshot(args: &ExportArgs) -> Result<ExportResult, String> {
    if !args.db.exists() {
        return Err(format!(
            "cannot open database: path does not exist: {}. Dual-live write with MCP is forbidden; close the agent or copy the DB first.",
            args.db.display()
        ));
    }

    let store = Store::open(&args.db).map_err(|e| {
        format!(
            "cannot open exclusive DuckDB at {}: {}. If MCP (or another process) has this file open, close it and retry. Dual-live write is forbidden.",
            args.db.display(),
            e
        )
    })?;

    let max_nodes = args.max_nodes.max(1);
    let view = if args.pkb
        && args.kinds.is_none()
        && args.seed_ids.is_none()
        && args.rel_types.is_none()
    {
        // Centralized Store helper (tags off, wikilink+related).
        store
            .export_graph_for_ui(Some(max_nodes), false)
            .map_err(|e| {
                format!(
                    "export_graph_for_ui failed on {}: {}. Check schema / graph tables.",
                    args.db.display(),
                    e
                )
            })?
    } else {
        let rel_types = if args.pkb {
            Some(
                PKB_DEFAULT_REL_TYPES
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect(),
            )
        } else {
            args.rel_types.clone()
        };
        let filter = GraphFilter {
            kinds: args.kinds.clone(),
            rel_types,
            seed_ids: args.seed_ids.clone(),
            max_nodes: Some(max_nodes),
        };
        store.get_graph_view(filter).map_err(|e| {
            format!(
                "get_graph_view failed on {}: {}. Check schema / graph tables.",
                args.db.display(),
                e
            )
        })?
    };

    let node_count = view.nodes.len();
    let edge_count = view.edges.len();
    let truncated = node_count >= max_nodes as usize;

    if args.envelope {
        let env = SnapshotEnvelopeOut {
            version: 1,
            source: "duckdb",
            db_path: args.db.display().to_string(),
            graph: &view,
        };
        let json = serde_json::to_string_pretty(&env)
            .map_err(|e| format!("serialize snapshot envelope: {e}"))?;
        write_snapshot_bytes(&args.output, json.as_bytes())?;
    } else {
        write_graph_snapshot(&args.output, &view)?;
    }

    Ok(ExportResult {
        output: args.output.clone(),
        node_count,
        edge_count,
        truncated,
    })
}

/// Load from a resolved CLI source (snapshot XOR exclusive db XOR http).
pub fn load_cli_source(
    source: &CliSource,
    seed: Option<&str>,
    depth: u32,
) -> Result<LoadedGraph, String> {
    match source {
        CliSource::Snapshot(path) => load_snapshot_path(path),
        CliSource::Db(path) => load_live_db(path, seed, depth),
        CliSource::Http(base) => load_http(base, seed, depth),
    }
}

/// Resolve seed by id, document_id, exact label, then substring label.
pub fn resolve_seed(view: &GraphView, query: &str) -> Result<String, String> {
    let q = query.trim();
    if q.is_empty() {
        return Err("empty seed".into());
    }
    if let Some(n) = view.nodes.iter().find(|n| n.id == q) {
        return Ok(n.id.clone());
    }
    if let Some(n) = view
        .nodes
        .iter()
        .find(|n| n.document_id.as_deref() == Some(q))
    {
        return Ok(n.id.clone());
    }
    let lower = q.to_lowercase();
    if let Some(n) = view
        .nodes
        .iter()
        .find(|n| n.label.to_lowercase() == lower)
    {
        return Ok(n.id.clone());
    }
    let matches: Vec<&GraphNode> = view
        .nodes
        .iter()
        .filter(|n| n.label.to_lowercase().contains(&lower))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => Err(format!("No node matches “{q}”")),
        many => {
            let shown: Vec<String> = many
                .iter()
                .take(5)
                .map(|n| format!("{} ({})", n.label, n.id))
                .collect();
            let more = if many.len() > 5 { ", …" } else { "" };
            Err(format!(
                "ambiguous seed “{q}” ({} matches): {}{}",
                many.len(),
                shown.join(", "),
                more
            ))
        }
    }
}

/// Case-insensitive title sort for the wiki sidebar catalog.
pub fn sort_wiki_pages(pages: &mut [WikiPageMeta]) {
    pages.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.title.cmp(&b.title))
    });
}

/// Client-side undirected BFS neighbors on a loaded snapshot (Mode C expand).
pub fn local_neighbors(
    full: &GraphView,
    seed_id: &str,
    depth: u32,
    max_nodes: usize,
) -> GraphView {
    let mut adj: HashMap<String, Vec<String>> = HashMap::new();
    for e in &full.edges {
        adj.entry(e.source_id.clone())
            .or_default()
            .push(e.target_id.clone());
        adj.entry(e.target_id.clone())
            .or_default()
            .push(e.source_id.clone());
    }

    let mut keep: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, u32)> = VecDeque::new();
    if full.nodes.iter().any(|n| n.id == seed_id) {
        queue.push_back((seed_id.to_string(), 0));
        keep.insert(seed_id.to_string());
    }

    while let Some((id, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        if let Some(neis) = adj.get(&id) {
            for n in neis {
                if keep.contains(n) {
                    continue;
                }
                if keep.len() >= max_nodes {
                    break;
                }
                keep.insert(n.clone());
                queue.push_back((n.clone(), d + 1));
            }
        }
        if keep.len() >= max_nodes {
            break;
        }
    }

    subgraph_from_keep(full, &keep)
}

/// Merge `extra` nodes into `base`, preferring existing nodes, capped at `max_nodes`.
///
/// Edges are taken from `edge_source` when provided (full loaded graph), otherwise
/// the union of base+extra edges restricted to the kept node set.
pub fn merge_graph_views(
    base: &GraphView,
    extra: &GraphView,
    edge_source: Option<&GraphView>,
    max_nodes: usize,
) -> GraphView {
    let max_nodes = max_nodes.max(1);
    let mut nodes = base.nodes.clone();
    let mut keep: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();

    // Stable order for new nodes: by id.
    let mut newcomers: Vec<&GraphNode> = extra
        .nodes
        .iter()
        .filter(|n| !keep.contains(&n.id))
        .collect();
    newcomers.sort_by(|a, b| a.id.cmp(&b.id));
    for n in newcomers {
        if nodes.len() >= max_nodes {
            break;
        }
        keep.insert(n.id.clone());
        nodes.push(n.clone());
    }

    let edges = match edge_source {
        Some(full) => full
            .edges
            .iter()
            .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
            .cloned()
            .collect(),
        None => {
            let mut by_id: HashMap<String, GraphEdge> = HashMap::new();
            for e in base.edges.iter().chain(extra.edges.iter()) {
                if keep.contains(&e.source_id) && keep.contains(&e.target_id) {
                    by_id.entry(e.id.clone()).or_insert_with(|| e.clone());
                }
            }
            let mut edges: Vec<GraphEdge> = by_id.into_values().collect();
            edges.sort_by(|a, b| a.id.cmp(&b.id));
            edges
        }
    };

    GraphView { nodes, edges }
}

/// Expand neighbors of `selected_id` by one hop (depth + 1 from that node) on a
/// loaded topology, merge into `current`, cap at `max_nodes`.
///
/// Used for Mode C snapshot / vault json (and any path where `full` is the
/// complete in-memory graph).
pub fn expand_neighbors_local(
    full: &GraphView,
    current: &GraphView,
    selected_id: &str,
    max_nodes: usize,
) -> GraphView {
    let max_nodes = max_nodes.max(1);
    // One hop from selected; budget = full cap so merge can prefer existing.
    let extra = local_neighbors(full, selected_id, 1, max_nodes);
    merge_graph_views(current, &extra, Some(full), max_nodes)
}

/// Expand neighbors via exclusive Store BFS (`Store::neighbors`), depth 1 from
/// the selected node, merge into `current` under `max_nodes`.
///
/// Mode A (`--db`) only. Re-opens DuckDB briefly; dual-live write with MCP is
/// still forbidden. `edge_source` (optional export already in memory) fills
/// cross edges between old and new nodes when present.
pub fn expand_neighbors_store(
    db_path: &Path,
    current: &GraphView,
    selected_id: &str,
    max_nodes: u32,
    edge_source: Option<&GraphView>,
) -> Result<GraphView, String> {
    let max_nodes = max_nodes.max(1);
    if !db_path.exists() {
        return Err(format!(
            "cannot expand: database does not exist: {}. Use --snapshot if the agent holds the file.",
            db_path.display()
        ));
    }
    let store = Store::open(db_path).map_err(|e| {
        format!(
            "cannot open exclusive DuckDB for expand at {}: {}. Dual-live write is forbidden; close MCP or use a snapshot.",
            db_path.display(),
            e
        )
    })?;
    // depth=1: one hop (depth+1 from selected). Cap matches UI max_nodes.
    let extra = store
        .neighbors(selected_id, 1, max_nodes)
        .map_err(|e| format!("Store::neighbors failed on {}: {e}", db_path.display()))?;
    if extra.nodes.is_empty() && !current.nodes.iter().any(|n| n.id == selected_id) {
        return Err(format!(
            "expand: no node id “{selected_id}” in store (or isolated with no neighbors)"
        ));
    }
    Ok(merge_graph_views(
        current,
        &extra,
        edge_source,
        max_nodes as usize,
    ))
}

fn subgraph_from_keep(full: &GraphView, keep: &HashSet<String>) -> GraphView {
    let nodes: Vec<GraphNode> = full
        .nodes
        .iter()
        .filter(|n| keep.contains(&n.id))
        .cloned()
        .collect();
    let edges: Vec<GraphEdge> = full
        .edges
        .iter()
        .filter(|e| keep.contains(&e.source_id) && keep.contains(&e.target_id))
        .cloned()
        .collect();
    GraphView { nodes, edges }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bare_graph_view() {
        let json = r#"{"nodes":[{"id":"a","kind":"document","label":"A","document_id":null,"uri":null,"resolved":true,"metadata_json":"{}"}],"edges":[]}"#;
        let v = parse_graph_json(json.as_bytes()).unwrap();
        assert_eq!(v.nodes.len(), 1);
    }

    #[test]
    fn export_roundtrips_models_graph_view() {
        let view = GraphView {
            nodes: vec![node("a", "A")],
            edges: vec![edge("e1", "a", "a")],
        };
        let json = export_graph_view_json(&view).unwrap();
        let back = parse_graph_json(json.as_bytes()).unwrap();
        assert_eq!(back.nodes.len(), 1);
        assert_eq!(back.nodes[0].id, "a");
        assert_eq!(back.edges[0].rel_type, "wikilink");
    }

    #[test]
    fn export_graph_snapshot_from_store() {
        let dir = std::env::temp_dir().join(format!(
            "rag-mcp-ui-export-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&dir);
        let db = dir.join("t.duckdb");
        let out = dir.join("graph.json");
        let _ = fs::remove_file(&db);
        let _ = fs::remove_file(&out);

        let store = Store::open(&db).expect("open temp store");
        store
            .upsert_graph_node(&node("n1", "Note One"))
            .expect("upsert node");
        store
            .upsert_graph_node(&node("n2", "Note Two"))
            .expect("upsert node");
        store
            .insert_graph_edges(&[edge("e1", "n1", "n2")])
            .expect("insert edge");
        drop(store);

        let args = ExportArgs {
            db: db.clone(),
            output: out.clone(),
            max_nodes: 500,
            rel_types: None,
            pkb: false,
            kinds: None,
            seed_ids: None,
            envelope: false,
        };
        let res = export_graph_snapshot(&args).expect("export");
        assert_eq!(res.node_count, 2);
        assert_eq!(res.edge_count, 1);
        assert!(!res.truncated);

        let loaded = load_snapshot_path(&out).expect("reload snapshot");
        assert_eq!(loaded.view.nodes.len(), 2);
        assert_eq!(loaded.view.edges.len(), 1);

        // envelope form
        let out_env = dir.join("graph.envelope.json");
        let args_env = ExportArgs {
            output: out_env.clone(),
            envelope: true,
            ..args
        };
        export_graph_snapshot(&args_env).expect("export envelope");
        let env_loaded = load_snapshot_path(&out_env).expect("reload envelope");
        assert_eq!(env_loaded.view.nodes.len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pkb_live_filter_defaults() {
        let f = pkb_live_filter();
        let rels = f.rel_types.expect("rels");
        assert!(rels.iter().any(|r| r == "wikilink"));
        assert!(rels.iter().any(|r| r == "related"));
        assert_eq!(f.max_nodes, Some(UI_HARD_MAX_NODES as u32));
        let kinds = f.kinds.expect("kinds");
        assert!(kinds.iter().any(|k| k == "document"));
        assert!(!kinds.iter().any(|k| k == "tag"));
    }

    #[test]
    fn parse_envelope_graph() {
        let json = r#"{"version":1,"graph":{"nodes":[{"id":"x","kind":"document","label":"X","document_id":null,"uri":null,"resolved":true,"metadata_json":"{}"}],"edges":[]}}"#;
        let v = parse_graph_json(json.as_bytes()).unwrap();
        assert_eq!(v.nodes[0].id, "x");
    }

    #[test]
    fn local_neighbors_depth1() {
        let view = GraphView {
            nodes: vec![
                node("a", "A"),
                node("b", "B"),
                node("c", "C"),
            ],
            edges: vec![
                edge("e1", "a", "b"),
                edge("e2", "b", "c"),
            ],
        };
        let local = local_neighbors(&view, "a", 1, 100);
        let ids: HashSet<_> = local.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(!ids.contains("c"));
    }

    #[test]
    fn expand_neighbors_local_merges_one_hop() {
        // seed a depth-1: {a,b}; expand b → pulls c
        let full = GraphView {
            nodes: vec![node("a", "A"), node("b", "B"), node("c", "C"), node("d", "D")],
            edges: vec![
                edge("e1", "a", "b"),
                edge("e2", "b", "c"),
                edge("e3", "c", "d"),
            ],
        };
        let current = local_neighbors(&full, "a", 1, 100);
        assert_eq!(current.nodes.len(), 2);
        let expanded = expand_neighbors_local(&full, &current, "b", 100);
        let ids: HashSet<_> = expanded.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));
        assert!(!ids.contains("d"));
        assert!(expanded
            .edges
            .iter()
            .any(|e| e.source_id == "b" && e.target_id == "c"));
    }

    #[test]
    fn expand_neighbors_local_respects_max_nodes() {
        let mut nodes = vec![node("seed", "Seed"), node("hub", "Hub")];
        let mut edges = vec![edge("e0", "seed", "hub")];
        for i in 0..20 {
            let id = format!("n{i}");
            nodes.push(node(&id, &id));
            edges.push(edge(&format!("eh{i}"), "hub", &id));
        }
        let full = GraphView { nodes, edges };
        let current = local_neighbors(&full, "seed", 1, 100);
        // current = seed + hub
        assert_eq!(current.nodes.len(), 2);
        let expanded = expand_neighbors_local(&full, &current, "hub", 5);
        assert!(expanded.nodes.len() <= 5);
        assert!(expanded.nodes.iter().any(|n| n.id == "seed"));
        assert!(expanded.nodes.iter().any(|n| n.id == "hub"));
    }

    #[test]
    fn expand_neighbors_store_one_hop() {
        let dir = std::env::temp_dir().join(format!(
            "rag-mcp-ui-expand-{}",
            std::process::id()
        ));
        let _ = fs::create_dir_all(&dir);
        let db = dir.join("t.duckdb");
        let _ = fs::remove_file(&db);

        let store = Store::open(&db).expect("open");
        store.upsert_graph_node(&node("a", "A")).unwrap();
        store.upsert_graph_node(&node("b", "B")).unwrap();
        store.upsert_graph_node(&node("c", "C")).unwrap();
        store
            .insert_graph_edges(&[edge("e1", "a", "b"), edge("e2", "b", "c")])
            .unwrap();
        drop(store);

        let current = GraphView {
            nodes: vec![node("a", "A"), node("b", "B")],
            edges: vec![edge("e1", "a", "b")],
        };
        let expanded =
            expand_neighbors_store(&db, &current, "b", 100, None).expect("expand store");
        let ids: HashSet<_> = expanded.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"));

        let _ = fs::remove_dir_all(&dir);
    }

    fn node(id: &str, label: &str) -> GraphNode {
        GraphNode {
            id: id.into(),
            kind: "document".into(),
            label: label.into(),
            document_id: None,
            uri: None,
            resolved: true,
            metadata_json: "{}".into(),
        }
    }

    fn edge(id: &str, s: &str, t: &str) -> GraphEdge {
        GraphEdge {
            id: id.into(),
            source_id: s.into(),
            target_id: t.into(),
            rel_type: "wikilink".into(),
            weight: 1.0,
            context: None,
        }
    }
}
