import { http, type RequestOptions } from '../http'
import type {
  BacklinksResponse,
  KnowledgeFilter,
  KnowledgeProps,
  KnowledgeRow,
  KnowledgeView,
  ReviewEvidence,
  WikiCreateBody,
  WikiListParams,
  WikiListResponse,
  WikiProposal,
  WikiProposalChange,
  WikiPutBody,
  WikiPutResult,
} from '../types'

/** Build query string from wiki list params (skips undefined / empty). */
export function wikiListQuery(params?: WikiListParams): string {
  const q = new URLSearchParams()
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== '') q.set(k, String(v))
    }
  }
  return q.toString()
}

/** Wiki catalog, CAS writes, backlinks, review queue, proposals and knowledge views. */
export const wikiApi = {
  /**
   * `GET /v1/wiki?q=&limit=&offset=&kind=&category=&wing=&room=`
   * Server-side `q` is case-insensitive substring on title/slug/uri/summary/category/kind.
   */
  wikiList: (params?: WikiListParams, options?: RequestOptions) => {
    const s = wikiListQuery(params)
    return http.get<WikiListResponse>(`/v1/wiki${s ? `?${s}` : ''}`, options)
  },
  /** Create a page only if its wiki URI is unoccupied. */
  createWiki: (body: WikiCreateBody) => http.post<WikiPutResult>('/v1/wiki', body),
  /** Update a wiki page using its current revision / ETag; a stale write answers 409. */
  putWiki: (body: WikiPutBody) => http.put<WikiPutResult>('/v1/wiki', body),
  backlinks: (id: string, options?: RequestOptions) =>
    http.get<BacklinksResponse>(`/v1/backlinks?id=${encodeURIComponent(id)}`, options),

  wikiReview: () => http.get<{ items: ReviewEvidence[] }>('/v1/wiki-review'),
  proposals: () => http.get<{ items: WikiProposal[] }>('/v1/wiki-proposals'),
  changeProposal: (body: WikiProposalChange) => http.post<WikiProposal>('/v1/wiki-proposals', body),

  /** Wiki pages filtered by their knowledge properties. */
  knowledge: (filter: KnowledgeFilter) => {
    const query = new URLSearchParams(Object.entries(filter).map(([k, v]) => [k, String(v)]))
    return http.get<{ items: KnowledgeRow[]; total: number }>(`/v1/knowledge?${query.toString()}`)
  },
  /** CAS update of one page's knowledge properties. */
  saveKnowledgeProperties: (body: { id: string; revision: number; properties: KnowledgeProps }) =>
    http.put<{ revision: number }>('/v1/knowledge', body),
  knowledgeViews: async (): Promise<KnowledgeView[]> =>
    (await http.get<{ items: KnowledgeView[] }>('/v1/knowledge/views')).items,
  /** Create a saved view (no id) or CAS-update an existing one. */
  saveKnowledgeView: (body: {
    id?: string | undefined
    revision?: number | undefined
    name: string
    filters: KnowledgeFilter
  }) => http.post<KnowledgeView>('/v1/knowledge/views', body),
}
