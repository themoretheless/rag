//! RAG MCP library: DuckDB-backed store, chunking, embeddings, graph, wiki, FTS, and MCP tools.
//!
//! # Module tree (P0 surface)
//!
//! | Module | Role |
//! |--------|------|
//! | [`db`] | DuckDB store, schema, vector/hybrid search, FTS, graph CRUD, kg |
//! | [`graph`] | Wikilink/tag extract + `rebuild_document_graph` |
//! | [`wiki`] | Karpathy compile layer (raw/wiki pages, schema, index, file_answer) |
//! | [`diary`] | Per-agent diary notes + session `checkpoint` (ops_log) |
//! | [`util`] | Content hash + `RAG_INGEST_ROOTS` path allowlist |
//! | [`mcp`] | MCP server (stdio + streamable HTTP `/mcp`) + tool structs |
//! | [`search_pack`] | Token-budget packing for citation context |
//! | [`llm`] | Optional local chat client (Ollama / OpenAI-compat) |
//! | [`maintain`] | Corpus analyze / plan whitelist / organize / refresh (ops_log) |

pub mod chunking;
pub mod config;
pub mod db;
pub mod diary;
pub mod embeddings;
pub mod error;
pub mod graph;
pub mod http_api;
pub mod llm;
pub mod maintain;
pub mod mcp;
pub mod models;
pub mod search_pack;
pub mod storage;
pub mod util;
pub mod wiki;

// --- Core types & config (integration tests / binary) ---
pub use config::Config;
pub use db::Store;
pub use error::AppError;
pub use models::{
    Chunk, DiaryEntry, Document, DocumentFilter, DocumentMetaApplyResult, DocumentMetaUpdate,
    DoctorReport, DrawerListItem, DuplicateCheckResult, DuplicateMatch, EmbeddingManifest,
    GraphEdge, GraphFilter, GraphNode, GraphStats, GraphView, IndexQueryMatch, IndexQueryPage,
    IndexQueryResult, IngestResult, KgFact, KgStats, LlmStatusReport, OpsLogEntry, PlacementUpdate,
    RoomCount, SearchHit, SearchMode, Stats, StatusReport, Taxonomy, TaxonomyNode, TaxonomyRoom,
    TaxonomyWing, VacuumStoreReport, WakeUpReport, WakeUpSchemaSnippet, WikiIndexEntry, WingCount,
    PKB_NODE_KINDS, PKB_REL_TYPES, UI_GRAPH_EXPORT_MAX_NODES,
};
pub use storage::{BackendKind, BackendMetadata, Storage, StorageCapability};

// --- Graph (domain extract/resolve; DB CRUD lives on [`Store`] via `db::graph`) ---
pub use graph::{
    extract_links, rebuild_document_graph, ExtractedLink, REL_TAGGED, REL_TUNNEL, REL_WIKILINK,
};

// --- FTS / hybrid substrate (owned by `db::fts`, re-exported at `db::`) ---
pub use db::{
    ensure_fts, ensure_fts_with_stemmer, fts_index_present, fts_status, reindex,
    reindex_with_stemmer, search_bm25, FtsBackend, FtsState, LexFilters,
};

// --- Util (hash + ingest path allowlist) ---
pub use util::{
    check_path_allowlist, content_hash, ingest_roots_from_env, parse_ingest_roots,
};

// --- Wiki compile layer ---
pub use wiki::{
    apply_consolidate_proposal, consolidate, file_answer, find_stale_wiki, get_schema,
    get_wiki_page, list_wiki_pages, refresh_stale_wiki, update_schema, update_wiki_page,
    write_wiki_page, ConsolidateOpts, ConsolidateResult, FileAnswerCitation, RawRefreshTarget,
    RefreshError, RefreshStaleWikiResult, SchemaDocumentView, StaleWikiItem, WikiWriteResult,
    LAYER_RAW, LAYER_SCHEMA, LAYER_WIKI, SCHEMA_URI,
};

// --- Agent diary / session checkpoint / wake_up ---
pub use diary::{
    agent_wing, checkpoint, diary_read, diary_write, normalize_agent_name, wake_up, CheckpointResult,
    DiaryWriteResult, KIND_DIARY, LAYER_DIARY, ROOM_DIARY,
};

// --- Search packing ---
pub use search_pack::{
    estimate_tokens, format_context_block, pack_hits, PackedContext, CHARS_PER_TOKEN,
};

// --- Maintenance (analyze / plan / apply / organize / compress / refresh) ---
pub use maintain::{
    action_from_parts, analyze_corpus, apply_maintenance_plan, filter_plan, heuristic_plan,
    is_allowed_action, log_plan, maintain_compress, maintain_organize, maintain_refresh,
    maintenance_actions_json_schema, maintenance_plan_json_schema, parse_and_filter_plan,
    parse_plan_json, plan_maintenance, validate_action, validate_plan, ActionOutcome,
    ActionOutcomeKind, AnalysisReport, AnalyzeOptions, ApplyMaintenancePlanReport,
    ApplyPlanOptions, CompressOptions, MaintainCompressReport, MaintainRefreshFlags,
    MaintainRefreshReport, MaintenanceAction, MaintenancePlan, MaintenancePlanItem, OrganizeMode,
    OrganizeOptions, OrganizeReport, PlanOptions, PlanSource, RefileApplyResult, RefileSuggestion,
    ALLOWED_ACTIONS, COMPRESS_ACTION_WHITELIST, COMPRESS_LEVEL_MAX, REFRESH_ACTION_WHITELIST,
};
