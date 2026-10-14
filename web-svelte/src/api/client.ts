import type {
  BacklinksResponse,
  DocumentBody,
  GraphView,
  HealthResponse,
  WikiListParams,
  WikiListResponse,
  WikiCreateBody,
  WikiPutBody,
  WikiPutResult,
} from './types'
import { t } from '@/lib/i18n'

const base = () => (import.meta.env.VITE_API_BASE ?? '').replace(/\/$/, '')
const tokenKey = () => `rag-gateway-token:${base() || 'same-origin'}`

function gatewayToken(): string {
  try { return sessionStorage.getItem(tokenKey()) ?? '' }
  catch { return '' }
}

export const hasGatewayToken = () => Boolean(gatewayToken())

/** Keep credentials in this tab's session, never in URLs or persistent storage. */
export function setGatewayToken(value: string): void {
  const token = value.trim()
  if (token && !/^[A-Za-z0-9._~+/-]+=*$/.test(token)) {
    throw new Error(t('tokenInvalidChars'))
  }
  try {
    if (token) sessionStorage.setItem(tokenKey(), token)
    else sessionStorage.removeItem(tokenKey())
  } catch {
    throw new Error(t('tokenStorageDenied'))
  }
}

/** Resolve a gateway path for browser links as well as fetch requests. */
export const apiUrl = (path: string) => `${base()}${path}`

async function requestResponse(path: string, init?: RequestInit): Promise<Response> {
  if (!path.startsWith('/') || path.startsWith('//') || /[\\\u0000-\u0020\u007f]/.test(path)) {
    throw new Error(t('apiInvalidPath'))
  }
  const url = apiUrl(path)
  const headers = new Headers(init?.headers)
  headers.set('Accept', 'application/json')
  if (init?.body) headers.set('Content-Type', 'application/json')
  const token = gatewayToken()
  if (token) headers.set('Authorization', `Bearer ${token}`)
  const res = await fetch(url, {
    ...init,
    headers,
    redirect: 'error',
  })
  if (!res.ok) {
    if (res.status === 401) throw new ApiError(res.status, t('apiUnauthorized'))
    if (res.status === 403) throw new ApiError(res.status, t('apiForbidden'))
    const text = await res.text().catch(() => '')
    throw new ApiError(res.status, t('apiHttpError', { status: res.status, detail: text.slice(0, 400) || res.statusText }))
  }
  return res
}

/** Error carrying the HTTP status of a failed or unparseable gateway response. */
export class ApiError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'ApiError'
    this.status = status
  }
}

/**
 * Parse a JSON body. Empty bodies (204/205, Content-Length 0, or blank text) resolve to
 * `undefined`; a non-empty body that is not JSON throws an `ApiError` with the status.
 */
export async function parseJsonBody<T>(res: Response): Promise<T> {
  if (res.status === 204 || res.status === 205 || res.headers.get('Content-Length') === '0') {
    return undefined as T
  }
  const text = await res.text()
  if (!text.trim()) return undefined as T
  try {
    return JSON.parse(text) as T
  } catch {
    throw new ApiError(res.status, t('apiInvalidJson', { status: res.status }))
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await requestResponse(path, init)
  return parseJsonBody<T>(res)
}

/**
 * Run independent dashboard loaders without letting one failed panel hide the rest.
 * Returns `label: message` for each failed panel.
 */
export async function loadPanels(
  panels: ReadonlyArray<readonly [label: string, load: () => Promise<void>]>,
): Promise<string[]> {
  const outcomes = await Promise.all(
    panels.map(async ([label, load]) => {
      try {
        await load()
        return null
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        return message ? `${label}: ${message}` : label
      }
    }),
  )
  return outcomes.filter((label): label is string => label !== null)
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

export interface ReviewEvidence {
  wiki_id: string; wiki_uri: string; wiki_title: string; raw_id: string; raw_uri: string; raw_title: string; raw_updated_at: string | null; link_kind: string
}

export interface WikiProposal {
  id: string; base: DocumentBody; sources: { document_id: string; uri: string; content_hash: string | null; content: string | null }[];
  content: string; revision: number; status: 'pending' | 'accepted' | 'rejected'
}

export const api = {
  proposals: () => request<{ items: WikiProposal[] }>('/v1/wiki-proposals'),
  changeProposal: (body: { action: 'create'; document_id: string } | { action: 'save'; id: string; revision: number; content: string } | { action: 'accept' | 'reject'; id: string; revision: number }) => request<WikiProposal>('/v1/wiki-proposals', { method: 'POST', body: JSON.stringify(body) }),
  wikiReview: () => request<{ items: ReviewEvidence[] }>('/v1/wiki-review'),
  // The shell polls this: skip the server's integrity scans (see /v1/doctor).
  health: () => request<HealthResponse>('/health?summary=true'),

  /** Ordinary browser links cannot attach Bearer credentials. */
  sourceFile: async (documentId: string) => {
    const res = await requestResponse(`/v1/source-file?document_id=${encodeURIComponent(documentId)}`)
    return res.blob()
  },

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

  /** Create a page only if its wiki URI is unoccupied. */
  createWiki: (body: WikiCreateBody) =>
    request<WikiPutResult>('/v1/wiki', { method: 'POST', body: JSON.stringify(body) }),

  /** Update a wiki page using its current revision / ETag. */
  putWiki: (body: WikiPutBody) =>
    request<WikiPutResult>('/v1/wiki', {
      method: 'PUT',
      body: JSON.stringify(body),
    }),

  backlinks: (id: string) =>
    request<BacklinksResponse>(`/v1/backlinks?id=${encodeURIComponent(id)}`),

  graph: (opts?: { max_nodes?: number; include_tags?: boolean; project?: string }) => {
    const q = new URLSearchParams()
    if (opts?.max_nodes) q.set('max_nodes', String(opts.max_nodes))
    if (opts?.include_tags) q.set('include_tags', 'true')
    if (opts?.project) q.set('project', opts.project)
    const s = q.toString()
    return request<GraphView>(`/v1/graph${s ? `?${s}` : ''}`)
  },

  neighbors: (seed: string, depth = 1, max_nodes = 100, include_tags = false, project = '') => {
    const q = new URLSearchParams({
      seed,
      depth: String(depth),
      max_nodes: String(max_nodes),
    })
    if (include_tags) q.set('include_tags', 'true')
    if (project) q.set('project', project)
    return request<GraphView>(`/v1/neighbors?${q}`)
  },

  findNode: (q: string) =>
    request<unknown>(`/v1/find?q=${encodeURIComponent(q)}`),

  get: <T = Record<string, unknown>>(path: string) => request<T>(path),
  put: <T = Record<string, unknown>>(path: string, body: unknown) => request<T>(path, { method:'PUT', body:JSON.stringify(body) }),
  post: <T = Record<string, unknown>>(path: string, body: unknown = {}) =>
    request<T>(path, { method: 'POST', body: JSON.stringify(body) }),
}
