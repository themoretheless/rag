/** Types matching rag-mcp HTTP gateway JSON. */

export interface HealthResponse {
  ok: boolean
  documents?: number
  chunks?: number
  nodes?: number
  edges?: number
  mcp_http?: boolean
  db_path?: string
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
  layer?: string | null
  wing?: string | null
  room?: string | null
}

export interface GraphEdge {
  id?: string
  source_id: string
  target_id: string
  rel_type: string
}

export interface GraphView {
  nodes: GraphNode[]
  edges: GraphEdge[]
  ok?: boolean
}
