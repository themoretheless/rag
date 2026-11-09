/**
 * Types matching rag-mcp HTTP gateway JSON.
 *
 * GraphNode, GraphEdge, SearchHit and SearchExplanation are checked against the
 * Rust DTOs by `tests/it/web_types_contract_test.rs`.
 */

export interface HealthResponse {
  ok: boolean
  backend?: string
  fts_ready?: boolean
  documents?: number
  chunks?: number
  nodes?: number
  edges?: number
  mcp_http?: boolean
  db_path?: string
  runtime?: {
    http_bind?: string
    ready?: boolean
    version?: string
    build_commit?: string
  }
}

export interface ResourcesResponse {
  ok: boolean
  process: {
    pid: number
    uptime_seconds: number
    cpu_percent: number
    logical_cpus: number
    tokio_worker_threads: number
    rss_bytes: number
    virtual_bytes: number
    system_memory_total: number
    system_memory_used: number
    load_avg_1m: number
    load_avg_5m: number
    load_avg_15m: number
    sampled_at: string
  }
  embedder: {
    concurrency_limit: number
    in_flight: number
    calls_total: number
    texts_total: number
    wait_ms_total: number
    embed_ms_total: number
    last_batch_ms: number
    max_batch_ms: number
  }
  database: {
    path: string
    bytes: number
    wal_bytes: number
    wal_warn_bytes: number
    threads: number
    memory_limit?: string | null
  }
  runtime: {
    version: string
    build_commit: string
    worker_threads_env?: string | null
  }
}

export interface WikiPageMeta {
  id: string
  uri: string
  slug: string
  title: string
  kind: string
  summary?: string | null
  category?: string | null
  revision: number
  etag: string
  updated_at: string
}

/** Query params for `GET /v1/wiki` (server-side catalog filters). */
export interface WikiListParams {
  /** Case-insensitive substring on title / slug / uri / summary / category / kind. */
  q?: string
  limit?: number
  offset?: number
  kind?: string
  category?: string
  wing?: string
  room?: string
}

export interface WikiListResponse {
  ok: boolean
  count: number
  pages: WikiPageMeta[]
  /** Total matches before limit/offset (when server provides it). */
  total?: number
  limit?: number
  offset?: number
}

export interface DocumentBody {
  source_versions?: { document_id?: string; uri?: string }[]
  id: string
  uri: string
  title: string
  layer: string
  kind: string
  content: string
  content_hash?: string | null
  wing?: string | null
  room?: string | null
  source_file?: string | null
  updated_at?: string | null
  revision?: number | null
  etag?: string | null
}

export interface WikiPutBody {
  slug: string
  title: string
  content: string
  id?: string
  uri?: string
  kind?: string
  category?: string
  summary?: string
  if_match_revision?: number
  if_match_etag?: string
}

/** Create-only writes cannot select an existing document identity or revision. */
export type WikiCreateBody = Omit<WikiPutBody, 'id' | 'uri' | 'if_match_revision' | 'if_match_etag'>

export interface WikiPutResult {
  ok: boolean
  document_id: string
  uri: string
  slug: string
  revision: number
  etag: string
  chunk_count?: number
  node_id?: string
  edge_count?: number
}

export interface BacklinkItem {
  contexts?: string[]
  id: string
  label: string
}

export interface BacklinksResponse {
  ok: boolean
  count: number
  backlinks: BacklinkItem[]
}

export interface GraphNode {
  id: string
  label: string
  kind?: string
  document_id?: string | null
  uri?: string | null
  resolved?: boolean
  /** JSON object text; wing/room live here, not on the node. */
  metadata_json?: string
}

export interface GraphEdge {
  id?: string
  source_id: string
  target_id: string
  rel_type: string
  weight?: number
  context?: string | null
}

export interface GraphView {
  nodes: GraphNode[]
  edges: GraphEdge[]
  ok?: boolean
}

export interface SearchExplanation {
  reasons?: string[]
  retrieval_ms: number
  postprocess_ms: number
  total_ms: number
  embed_ms?: number
  vec_ms?: number
  lex_ms?: number
  rrf_k?: number
  deduplication?: string
}

export interface SearchHit {
  chunk_id: string
  document_id: string
  document_title: string
  document_uri: string
  chunk_index: number
  content: string
  score: number
  score_vec?: number
  score_lex?: number
  score_rrf?: number
  rank_vec?: number
  rank_lex?: number
  snippet?: string
  char_start?: number
  char_end?: number
  heading_path?: string[]
  section?: string
  explanation?: SearchExplanation
}

export interface SearchResponse {
  ok: boolean
  mode: 'lex' | 'vec' | 'hybrid'
  count: number
  timings?: SearchExplanation
  items: SearchHit[]
}

export interface PackContextResponse {
  ok: boolean
  total_tokens: number
  max_tokens: number
  omitted_count: number
  context_text: string
  hits: SearchHit[]
}

/* ---------- Operational dashboards (console / models / agents / evaluation) ---------- */

/**
 * `GET /v1/status` (fields are optional: older gateways omit some).
 * `?summary=true` answers with the cheap subset and may use the legacy
 * `documents` / `chunks` counter names.
 */
export interface StorageStatus {
  ok?: boolean
  documents?: number
  chunks?: number
  backend?: string
  db_path?: string
  db_file_bytes?: number
  wal_bytes?: number
  wal_warn_bytes?: number
  schema_version?: number | string
  fts_ready?: boolean
  ready_for_search?: boolean
  storage_capabilities?: string[]
  document_count?: number
  chunk_count?: number
  node_count?: number
  edge_count?: number
  raw_count?: number
  wiki_count?: number
  uncompiled_raw_count?: number
  index_coverage?: number
  index_entry_count?: number
  embed_provider?: string
  embed_model?: string
  embed_dims?: number
  pid?: number
  uptime_seconds?: number
  ingest_roots_configured?: boolean
}

/** `GET /v1/doctor` integrity report. */
export interface DoctorReport {
  ok?: boolean
  schema_ok?: boolean
  schema_version?: number | string
  expected_schema_version?: number | string
  embed_ok?: boolean
  embed_dims?: number
  fts_ready?: boolean
  relational_integrity_ok?: boolean
  documents_without_chunks?: number
  orphan_chunks?: number
  orphan_edges?: number
  unscoped_documents?: number
  wal_bytes?: number
  wal_warn_bytes?: number
  wal_too_large?: boolean
}

/** `GET /v1/runtime` → `runtime`. */
export interface RuntimeInfo {
  pid?: number
  uptime_seconds?: number
  http_bind?: string
  tool_surface?: string
  auto_backup_enabled?: boolean
  auto_backup_last_completed_at?: string | null
}

export interface TaxonomyRoom { room: string; document_count: number }
export interface TaxonomyWing { wing: string; document_count: number; rooms?: TaxonomyRoom[] }
/** `GET /v1/taxonomy` → `taxonomy`. */
export interface TaxonomySummary {
  wings: TaxonomyWing[]
  unscoped_count?: number
  total_documents?: number
}

/** One `ops_log` mutation row. */
export interface OpsLogItem {
  id: string
  ts?: string
  op?: string
  prefix?: string
  agent_name?: string | null
  message?: string
  payload_json?: string | null
}

/** `GET /v1/agents` row: one MCP client known to the gateway. */
export interface AgentInfo {
  agent: string
  transport?: string
  online?: boolean
  last_tool?: string
  last_call_at?: string
  calls_today?: number
  diary_count?: number
}

/** `GET /v1/calls` row: one recent MCP tool call (in-memory stream, not persisted). */
export interface CallRow {
  seq: number
  ts: string
  agent: string
  tool: string
  args: string
  elapsed_ms: number
  ok: boolean
  result_hint?: string
  error?: string
}

/** `GET /v1/activity` row. */
export interface ActivityItem { seq?: number; at?: string; client?: string; action?: string; status?: string }

export interface SyncNode {
  node_id: string
  hostname: string
  role: string
  last_seen_at: string
  last_push_seq: number
  pull_cursor: number
  last_error?: string
}
/** `GET /v1/sync/status` → `sync`. */
export interface SyncState {
  role: string
  node_id: string
  primary_url?: string
  latest_primary_seq: number
  pending_outbox: number
  nodes: SyncNode[]
}

/** `GET /v1/kg/stats` → `stats`. */
export interface KgStats { total_facts?: number }

/** `GET /v1/kg` fact row. */
export interface KgFact {
  id: string
  subject: string
  predicate: string
  object: string
  status?: string
  confidence?: number | null
  valid_from?: string | null
  valid_to?: string | null
}

/** `{ items: [...] }` envelope used by most list endpoints. */
export interface ItemsResponse<T> { items?: T[] }

/* ---------- Corpus (projects / documents) ---------- */

/** `GET /v1/projects` row (a wing with its rooms). */
export interface ProjectSummary {
  project_id: string
  document_count: number
  rooms?: TaxonomyRoom[]
}

/** `GET /v1/project-home` → `project`. */
export interface ProjectHome { source_roots?: { canonical_root: string; file_count: number }[] }

/** `GET /v1/documents` catalog row. */
export interface DocumentRow {
  id: string
  uri: string
  title: string
  wing?: string | null
  room?: string | null
  source_file?: string | null
  layer: string
  kind: string
  status: string
  revision?: number
  pinned?: boolean
  boost?: number
  content_hash?: string | null
  updated_at: string
}

/** `GET /v1/documents` filters; `cursor` continues a previous page. */
export interface DocumentsQuery {
  limit?: number | undefined
  wing?: string | undefined
  room?: string | undefined
  layer?: string | undefined
  status?: string | undefined
  include_archived?: boolean | undefined
  q?: string | undefined
  cursor?: string | undefined
}

/** `GET /v1/documents` page. */
export interface DocumentPage {
  items?: DocumentRow[]
  page?: { total?: number; next_cursor?: string | null }
}

export interface DocumentChunk {
  id: string
  chunk_index: number
  content: string
  char_start: number
  char_end: number
}

/** `POST /v1/multi-get` item: full document plus (optionally) its chunks. */
export interface DocumentDetail {
  document: DocumentRow & { revision: number; metadata_json: string; content: string }
  chunks?: DocumentChunk[]
  chunks_total?: number
  chunks_truncated?: boolean
}

/* ---------- Search requests ---------- */

/** `POST /v1/search` body. */
export interface SearchRequest {
  query: string
  mode: SearchResponse['mode']
  top_k: number
  wing?: string | undefined
  room?: string | undefined
  layer?: string | undefined
  min_score?: number | undefined
  rrf_k?: number | undefined
  max_chunks_per_document?: number | undefined
  context_expansion?: 'neighbors' | undefined
  neighbor_chunks?: number | undefined
  max_context_tokens?: number | undefined
  recency_half_life_days?: number | undefined
  timeout_ms?: number | undefined
  document_id?: string | undefined
}

/** `POST /v1/pack-context` body. */
export interface PackContextRequest {
  hits: SearchHit[]
  max_tokens: number
  context_expansion?: 'neighbors' | undefined
  neighbor_chunks?: number | undefined
}

/* ---------- Wiki review / proposals / knowledge views ---------- */

/** `GET /v1/wiki-review` row: a wiki page whose raw source moved on. */
export interface ReviewEvidence {
  wiki_id: string
  wiki_uri: string
  wiki_title: string
  raw_id: string
  raw_uri: string
  raw_title: string
  raw_updated_at: string | null
  link_kind: string
}

export interface WikiProposal {
  id: string
  base: DocumentBody
  sources: { document_id: string; uri: string; content_hash: string | null; content: string | null }[]
  content: string
  revision: number
  status: 'pending' | 'accepted' | 'rejected'
}

/** `POST /v1/wiki-proposals` body. */
export type WikiProposalChange =
  | { action: 'create'; document_id: string }
  | { action: 'save'; id: string; revision: number; content: string }
  | { action: 'accept' | 'reject'; id: string; revision: number }

export interface KnowledgeProps { document_type: string; review_status: string; owner: string; review_due: string }
/** `GET /v1/knowledge` filters (also stored inside a saved view). */
export interface KnowledgeFilter {
  q: string
  wing: string
  document_type: string
  review_status: string
  owner: string
  due_before: string
  offset: number
}
export interface KnowledgeRow {
  id: string
  uri: string
  title: string
  wing: string | null
  revision: number
  properties: Partial<KnowledgeProps>
}
export interface KnowledgeView { id: string; name: string; revision: number; filters: KnowledgeFilter }

/** `GET /v1/llm-status` → `llm`. */
export interface LlmStatus {
  provider?: string
  model?: string
  base_url?: string
  llm_enabled?: boolean
  reachable?: boolean
  error?: string | null
  embed_provider?: string
  embed_model?: string
  embed_base_url?: string
  embed_dims?: number
}

export interface EmbeddingIdentity { provider?: string; model?: string; dims?: number; base_url?: string }
/** `GET /v1/embedding-manifest`. */
export interface EmbeddingManifestStatus {
  match?: boolean
  live?: EmbeddingIdentity | null
  manifest?: EmbeddingIdentity | null
}

/** `GET /v1/capabilities`. */
export interface Capabilities { api_version?: string | number; tool_count?: number; tool_surface?: string }

/** Retrieval eval history (`GET /v1/eval/history`). */
export interface EvalResult { rank: number; document_title: string; relevance: number }
export interface EvalQueryReport {
  id: string
  recall_at_k?: number
  reciprocal_rank?: number
  ndcg_at_k?: number
  search_ms?: number
  results?: EvalResult[]
}
export interface EvalModeReport {
  mode: string
  recall_at_k?: number
  mrr?: number
  ndcg_at_k?: number
  p95_search_ms?: number
  queries?: EvalQueryReport[]
}
export interface EvalScaleRecommendation {
  path?: string
  reason?: string
  chunk_threshold?: number
  p95_search_ms_threshold?: number
}
export interface EvalRun {
  dataset_name?: string
  dataset_version?: string | number
  top_k?: number
  sampling?: { queries?: number }
  corpus?: { chunks?: number }
  modes?: EvalModeReport[]
  scale_recommendation?: EvalScaleRecommendation
}
export interface EvalHistoryResponse { items?: EvalRun[]; configured?: boolean; path?: string }

/** `GET /v1/eval/feedback` row: a real question saved from the search screen. */
export interface EvalQuestion {
  id: string
  search: { query: string; mode: string }
  expected: { uri: string }[]
  no_answer: boolean
}
/** `POST /v1/eval/feedback` body. */
export interface EvalFeedbackBody {
  id: string
  search: Record<string, unknown>
  expected: string[]
  no_answer: boolean
}
/** `POST /v1/eval/feedback/run` result. */
export interface EvalFeedbackRun {
  status: string
  recall?: number
  mrr?: number
  empty_result_for_no_answer?: boolean
}
