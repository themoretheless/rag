//! Wire and view-model types shared by the HTTP, snapshot and direct-db loaders.

use rag_mcp_core::GraphView;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Where the topology came from (status line).
#[derive(Debug, Clone)]
pub enum GraphSourceKind {
    #[cfg_attr(not(feature = "direct-db"), allow(dead_code))]
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

impl GraphSourceKind {
    /// Stable machine label for this source kind.
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
    /// Authoritative gateway/local-store catalog. `None` means the graph loaded
    /// but the independent project-catalog request failed; callers must retain
    /// their previous catalog instead of deriving one from the bounded graph.
    pub projects: Option<Vec<String>>,
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
    pub wal_too_large: bool,
    pub documents_without_chunks: u64,
    pub relational_integrity_ok: bool,
}

pub use rag_mcp_core::ActivityEvent;

/// Document body for graph UI "Read content" (from HTTP or read-only DB).
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
pub(crate) struct DocumentBodyJsonDefaults<'a> {
    pub(crate) content: Option<&'a str>,
    pub(crate) title: Option<&'a str>,
    pub(crate) uri: Option<&'a str>,
    /// Used when JSON has no `revision` (e.g. optimistic next rev after CAS).
    pub(crate) revision: Option<i64>,
}

/// Build [`DocumentBody`] from API JSON: full body, nested `document`, or flat fields.
///
/// Shared by GET document fetch and PUT wiki response parsing.
pub(crate) fn document_body_from_json(
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
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
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

/// Backlink row from `GET /v1/backlinks?id=`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BacklinkItem {
    pub label: String,
    pub id: String,
}

/// Body for `PUT /v1/wiki` (must match server `WikiPutBody`: slug-keyed write).
#[derive(Debug, Clone, Serialize)]
pub struct WikiPutRequest {
    /// Existing document UUID. The gateway uses it to select update semantics
    /// and preserve fields that this editor does not expose.
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

/// Result of a successful CLI `export` run.
#[derive(Debug, Clone)]
pub struct ExportResult {
    pub output: PathBuf,
    pub node_count: usize,
    pub edge_count: usize,
    pub truncated: bool,
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
