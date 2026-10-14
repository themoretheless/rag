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

/** `GET /v1/status` (fields are optional: older gateways omit some). */
export interface StorageStatus {
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

/** `GET /v1/agents` row (only fields read by the console). */
export interface AgentSummary { agent?: string; online?: boolean }

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

export interface ItemsResponse<T> { items?: T[] }

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
