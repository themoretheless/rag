import type {
  BacklinksResponse,
  DocumentBody,
  GraphView,
  HealthResponse,
  WikiListParams,
  WikiListResponse,
  WikiPutBody,
  WikiPutResult,
} from './types'

const base = () => (import.meta.env.VITE_API_BASE ?? '').replace(/\/$/, '')

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const url = `${base()}${path}`
  const res = await fetch(url, {
    ...init,
    headers: {
      Accept: 'application/json',
      ...(init?.body ? { 'Content-Type': 'application/json' } : {}),
      ...init?.headers,
    },
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    throw new Error(`HTTP ${res.status}: ${text.slice(0, 400) || res.statusText}`)
  }
  return res.json() as Promise<T>
}

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

export const api = {
  health: () => request<HealthResponse>('/health'),

  /**
   * `GET /v1/wiki?q=&limit=&offset=&kind=&category=&wing=&room=`
   * Server-side `q` is case-insensitive substring on title/slug/uri/summary/category/kind.
   */
  wikiList: (params?: WikiListParams) => {
    const s = wikiListQuery(params)
    return request<WikiListResponse>(`/v1/wiki${s ? `?${s}` : ''}`)
  },

  document: (opts: { id?: string; uri?: string; q?: string }) => {
    const q = new URLSearchParams()
    if (opts.id) q.set('id', opts.id)
    if (opts.uri) q.set('uri', opts.uri)
    if (opts.q) q.set('q', opts.q)
    return request<DocumentBody>(`/v1/document?${q}`)
  },

  /** Create or update a wiki page (slug-keyed upsert). Omit if_match_* for create. */
  putWiki: (body: WikiPutBody) =>
    request<WikiPutResult>('/v1/wiki', {
      method: 'PUT',
      body: JSON.stringify(body),
    }),

  backlinks: (id: string) =>
    request<BacklinksResponse>(`/v1/backlinks?id=${encodeURIComponent(id)}`),

  graph: (opts?: { max_nodes?: number; include_tags?: boolean }) => {
    const q = new URLSearchParams()
    if (opts?.max_nodes) q.set('max_nodes', String(opts.max_nodes))
    if (opts?.include_tags) q.set('include_tags', 'true')
    const s = q.toString()
    return request<GraphView>(`/v1/graph${s ? `?${s}` : ''}`)
  },

  neighbors: (seed: string, depth = 1, max_nodes = 100) => {
    const q = new URLSearchParams({
      seed,
      depth: String(depth),
      max_nodes: String(max_nodes),
    })
    return request<GraphView>(`/v1/neighbors?${q}`)
  },

  findNode: (q: string) =>
    request<unknown>(`/v1/find?q=${encodeURIComponent(q)}`),
}
