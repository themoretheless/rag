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
  // eslint-disable-next-line no-control-regex -- rejecting control characters in request paths is the point
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

function isEmptyStatus(res: Response): boolean {
  return res.status === 204 || res.status === 205 || res.headers.get('Content-Length') === '0'
}

function parseText(res: Response, text: string): unknown {
  try {
    return JSON.parse(text) as unknown
  } catch {
    throw new ApiError(res.status, t('apiInvalidJson', { status: res.status }))
  }
}

/**
 * Parse a JSON body for endpoints that must return JSON. An empty body (204/205,
 * Content-Length 0, or blank text) or a non-JSON body throws an `ApiError` with the status.
 */
export async function parseJsonBody<T>(res: Response): Promise<T> {
  const text = isEmptyStatus(res) ? '' : await res.text()
  if (!text.trim()) throw new ApiError(res.status, t('apiEmptyBody', { status: res.status }))
  return parseText(res, text) as T
}

/**
 * Parse a JSON body for endpoints that may legitimately answer 204 No Content.
 * Empty bodies resolve to `undefined`; a non-empty non-JSON body throws an `ApiError`.
 */
export async function parseOptionalJsonBody<T>(res: Response): Promise<T | undefined> {
  if (isEmptyStatus(res)) return undefined
  const text = await res.text()
  if (!text.trim()) return undefined
  return parseText(res, text) as T
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await requestResponse(path, init)
  return parseJsonBody<T>(res)
}

async function requestOptional<T>(path: string, init?: RequestInit): Promise<T | undefined> {
  const res = await requestResponse(path, init)
  return parseOptionalJsonBody<T>(res)
}

/** Per-call options accepted by every verb. */
export interface RequestOptions {
  /** Abort the request, e.g. from `createRequestGate()` when a newer one supersedes it. */
  signal?: AbortSignal | undefined
}

const withSignal = (init: RequestInit, options?: RequestOptions): RequestInit =>
  options?.signal ? { ...init, signal: options.signal } : init

const jsonBody = (method: string, body: unknown): RequestInit => ({ method, body: JSON.stringify(body) })

/**
 * Untyped HTTP verbs. Only `src/api/endpoints/*` should call these; screens and
 * stores use the typed `api` facade from `./client`.
 */
export const http = {
  get: <T>(path: string, options?: RequestOptions) => request<T>(path, withSignal({}, options)),
  put: <T>(path: string, body: unknown, options?: RequestOptions) =>
    request<T>(path, withSignal(jsonBody('PUT', body), options)),
  post: <T>(path: string, body: unknown = {}, options?: RequestOptions) =>
    request<T>(path, withSignal(jsonBody('POST', body), options)),
  /** POST to an endpoint that may answer 204 No Content (e.g. operations, feedback). */
  postOptional: <T>(path: string, body: unknown = {}, options?: RequestOptions) =>
    requestOptional<T>(path, withSignal(jsonBody('POST', body), options)),
  /** Authenticated binary download (plain browser links cannot attach Bearer credentials). */
  blob: async (path: string, options?: RequestOptions) => {
    const res = await requestResponse(path, withSignal({}, options))
    return res.blob()
  },
}

