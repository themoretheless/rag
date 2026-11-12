/**
 * Pure logic of the search lab: form snapshots, request bodies, the rank-flow
 * geometry and the wiki draft built from a packed context. No state, no I/O.
 */
import type {
  PackContextRequest,
  PackContextResponse,
  SearchHit,
  SearchRequest,
  SearchResponse,
  WikiCreateBody,
} from '@/api/types'

export type SearchMode = SearchResponse['mode']
export const SEARCH_MODES: readonly SearchMode[] = ['lex', 'vec', 'hybrid']

/** Editable search parameters, exactly as the form holds them. */
export interface SearchForm {
  query: string
  mode: SearchMode
  wing: string
  room: string
  layer: string
  topK: number
  minScore: number
  rrfK: number
  maxPerDocument: number
  neighborChunks: number
  maxTokens: number
  recencyDays: number
  timeoutMs: number
  documentId: string
}

export function defaultSearchForm(): SearchForm {
  return {
    query: '',
    mode: 'hybrid',
    wing: '',
    room: '',
    layer: '',
    topK: 8,
    minScore: 0,
    rrfK: 60,
    maxPerDocument: 2,
    neighborChunks: 1,
    maxTokens: 2000,
    recencyDays: 30,
    timeoutMs: 5000,
    documentId: '',
  }
}

/** Trimmed, frozen copy of the form; `signature` identifies the parameter set. */
export interface SearchSnapshot extends SearchForm {
  signature: string
}

export function snapshotOf(form: SearchForm): SearchSnapshot {
  // Key order is part of the signature, so list the fields explicitly.
  const values: SearchForm = {
    query: form.query.trim(),
    mode: form.mode,
    wing: form.wing.trim(),
    room: form.room.trim(),
    layer: form.layer.trim(),
    topK: form.topK,
    minScore: form.minScore,
    rrfK: form.rrfK,
    maxPerDocument: form.maxPerDocument,
    neighborChunks: form.neighborChunks,
    maxTokens: form.maxTokens,
    recencyDays: form.recencyDays,
    timeoutMs: form.timeoutMs,
    documentId: form.documentId.trim(),
  }
  return { ...values, signature: JSON.stringify(values) }
}

/**
 * `POST /v1/search` body for a snapshot. The single source for both the executed
 * search and the parameters saved with a relevance assessment. Empty filters and a
 * zero `min_score` are left out so the gateway defaults apply.
 */
export function searchRequest(snapshot: SearchSnapshot): SearchRequest {
  return {
    query: snapshot.query,
    mode: snapshot.mode,
    top_k: snapshot.topK,
    wing: snapshot.wing || undefined,
    room: snapshot.room || undefined,
    layer: snapshot.layer || undefined,
    min_score: snapshot.minScore || undefined,
    rrf_k: snapshot.rrfK,
    max_chunks_per_document: snapshot.maxPerDocument,
    context_expansion: snapshot.neighborChunks > 0 ? 'neighbors' : undefined,
    neighbor_chunks: snapshot.neighborChunks,
    max_context_tokens: snapshot.maxTokens,
    recency_half_life_days: snapshot.recencyDays,
    timeout_ms: snapshot.timeoutMs,
    document_id: snapshot.documentId || undefined,
  }
}

/** `POST /v1/pack-context` body: the ranked hits under the snapshot's token budget. */
export function packRequest(snapshot: SearchSnapshot, hits: readonly SearchHit[]): PackContextRequest {
  return {
    hits: [...hits],
    max_tokens: snapshot.maxTokens,
    context_expansion: snapshot.neighborChunks > 0 ? 'neighbors' : undefined,
    neighbor_chunks: snapshot.neighborChunks,
  }
}

/** Corpus deep link for a hit, keeping the wing / room the search was scoped to. */
export function corpusLink(hit: SearchHit, snapshot: SearchSnapshot | null): string {
  const params = new URLSearchParams({
    document_id: hit.document_id,
    q: hit.document_title || hit.document_uri,
  })
  if (snapshot?.wing) params.set('project', snapshot.wing)
  if (snapshot?.room) params.set('room', snapshot.room)
  return `/corpus?${params.toString()}`
}

/** Share of `total` taken by `value`, clamped to 0..100; 0 when either is unknown. */
export function timingPercent(value?: number, total?: number): number {
  if (value == null || total == null || total <= 0) return 0
  return Math.min(100, Math.max(0, (value / total) * 100))
}

/* ---------- Rank flow (lex → fused ← vec) ---------- */

/** Rows shown per rank-flow column. */
export const FLOW_ROWS = 8

export interface FlowRow {
  hit: SearchHit
  /** Position of the hit in the fused result list (0-based). */
  fusedIndex: number
}

export interface FlowConnection extends FlowRow {
  /** Row of the hit in the lex / vec column, -1 when that source did not return it. */
  lexIndex: number
  vecIndex: number
}

export interface RankFlowModel {
  /** Top fused hits, in fused order. */
  fused: FlowRow[]
  /** The same hits ordered by their lexical / vector rank. */
  lex: FlowRow[]
  vec: FlowRow[]
  connections: FlowConnection[]
}

/**
 * Source columns contain the same displayed fused hits, sorted by their real API
 * ranks, so every visible path is traceable end-to-end. Each row carries its fused
 * index, which the template would otherwise have to look up per attribute.
 */
export function rankFlow(hits: readonly SearchHit[]): RankFlowModel {
  const fused: FlowRow[] = hits.slice(0, FLOW_ROWS).map((hit, fusedIndex) => ({ hit, fusedIndex }))
  const byRank = (rank: (hit: SearchHit) => number | undefined) =>
    fused.filter((row) => rank(row.hit) != null).sort((a, b) => (rank(a.hit) ?? 999) - (rank(b.hit) ?? 999))
  const lex = byRank((hit) => hit.rank_lex)
  const vec = byRank((hit) => hit.rank_vec)
  const rowOf = (rows: FlowRow[]) => new Map(rows.map((row, index) => [row.fusedIndex, index]))
  const lexRow = rowOf(lex)
  const vecRow = rowOf(vec)
  const connections = fused.map((row) => ({
    ...row,
    lexIndex: lexRow.get(row.fusedIndex) ?? -1,
    vecIndex: vecRow.get(row.fusedIndex) ?? -1,
  }))
  return { fused, lex, vec, connections }
}

const FLOW_PALETTE = [
  'var(--l1)',
  'var(--l3)',
  'var(--l0)',
  'var(--l2)',
  'var(--l4)',
  'var(--ok)',
  'var(--warn)',
  'var(--accent)',
] as const

/** Stable colour of a fused rank. */
export function flowColor(fusedIndex: number): string {
  return FLOW_PALETTE[fusedIndex % FLOW_PALETTE.length] ?? FLOW_PALETTE[0]
}

/** Vertical centre of a rank-flow row in the 1000×289 link viewBox. */
export function flowY(row: number): number {
  return 40.5 + row * 31
}

/** Horizontal S-curve between two points of the link viewBox. */
export function flowPath(fromX: number, fromY: number, toX: number, toY: number): string {
  const controlX = (fromX + toX) / 2
  return `M ${fromX} ${fromY} C ${controlX} ${fromY}, ${controlX} ${toY}, ${toX} ${toY}`
}

/* ---------- Wiki draft from a packed context ---------- */

/**
 * Wiki page that preserves a packed context: the query, the parameters that produced
 * it and the exact text an agent would have received. `now` only feeds the slug.
 */
export function buildSearchContextPage(
  snapshot: SearchSnapshot,
  packed: PackContextResponse,
  sourceHitCount: number,
  now: number,
): WikiCreateBody {
  const cleanQuery = snapshot.query.replace(/\s+/g, ' ')
  const title = `Контекст поиска: ${cleanQuery.slice(0, 72) || 'без названия'}`
  const content = [
    `# ${title}`,
    '',
    '> Черновик создан из pack_context. Это извлечённый контекст, а не сгенерированный ответ.',
    '',
    '## Запрос',
    '',
    snapshot.query,
    '',
    '## Параметры поиска',
    '',
    `- mode: ${snapshot.mode}`,
    `- hits: ${packed.hits.length} из ${sourceHitCount}`,
    `- tokens: ${packed.total_tokens} из ${packed.max_tokens}`,
    '',
    '## Упакованный контекст',
    '',
    packed.context_text,
  ].join('\n')
  return {
    slug: `search-context-${now.toString(36)}`,
    title,
    content,
    kind: 'wiki',
    category: 'search-context',
    summary: `Черновик из ${packed.hits.length} хитов для запроса «${cleanQuery.slice(0, 96)}»`,
  }
}
