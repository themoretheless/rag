//! Command-line surface: GUI open flags and the headless `export` subcommand.

use rag_mcp_core::UI_GRAPH_EXPORT_MAX_NODES;
use std::path::PathBuf;

use crate::topology::{UI_DEFAULT_DEPTH, UI_LOCAL_MAX_NODES};

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
    about = "Native rag-mcp console (HTTP product UI; DB/snapshot modes stay read-only)",
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
    /// Opens the DB read-only via `Store`, dumps topology-only nodes/edges (no positions),
    /// writes pretty JSON, and exits without starting the GUI.
    Export(ExportArgs),
}

/// `rag-mcp-ui export --db PATH [-o graph.json]` arguments.
#[derive(Debug, Clone, clap::Args)]
pub struct ExportArgs {
    /// Existing current-schema DuckDB path to inspect without migration or writes.
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

    /// Max nodes fetched for the local neighbor view (default 100,000).
    /// The renderer applies its own smaller viewport cap after traversal.
    #[arg(long, default_value_t = UI_LOCAL_MAX_NODES)]
    pub max_nodes: u32,

    /// Resolved source after validate (not a clap field).
    #[arg(skip)]
    pub source: Option<CliSource>,
}

impl OpenArgs {
    /// Reject open args that name more than one of `--http`, `--snapshot` and `--db`.
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
        // Keep traversal finite; rendering has an independent viewport cap.
        self.max_nodes = self.max_nodes.clamp(1, UI_GRAPH_EXPORT_MAX_NODES);
        self.source = match (
            &self.snapshot,
            &self.db,
            self.http
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty()),
        ) {
            (Some(p), None, None) => Some(CliSource::Snapshot(p.clone())),
            (None, Some(p), None) => Some(CliSource::Db(p.clone())),
            (None, None, Some(url)) => Some(CliSource::Http(url.to_string())),
            _ => None,
        };
        self
    }
}
