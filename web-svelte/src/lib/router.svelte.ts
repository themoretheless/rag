/**
 * Tiny history-API router (runes module). Routes:
 *   /            → redirect /wiki
 *   /wiki/:id?   → wiki view (id optional)
 *   /graph?seed= → graph view
 *   /search      → search view
 */
export type RouteName = 'console' | 'corpus' | 'search' | 'graph' | 'wiki' | 'agents' | 'evaluation' | 'models'

export interface RouteState {
  name: RouteName
  /** Wiki page id for /wiki/:id, else null. */
  pageId: string | null
  /** Current path (without query). */
  path: string
  /** Query params of the current location. */
  query: URLSearchParams
}

interface Parsed {
  redirect: string | null
  state: RouteState
}

function parseLocation(pathname: string, search: string): Parsed {
  const query = new URLSearchParams(search)
  if (pathname === '/' || pathname === '') {
    return { redirect: '/console', state: { name: 'console', pageId: null, path: '/console', query: new URLSearchParams() } }
  }
  if (pathname === '/wiki' || pathname.startsWith('/wiki/')) {
    const rest = pathname.slice('/wiki'.length).replace(/^\//, '')
    const pageId = rest ? decodeURIComponent(rest) : null
    return { redirect: null, state: { name: 'wiki', pageId, path: pathname, query } }
  }
  if (pathname === '/graph') {
    return { redirect: null, state: { name: 'graph', pageId: null, path: pathname, query } }
  }
  if (pathname === '/search') {
    return { redirect: null, state: { name: 'search', pageId: null, path: pathname, query } }
  }
  const simple = pathname.slice(1) as RouteName
  if (['console', 'corpus', 'agents', 'evaluation', 'models'].includes(simple)) {
    return { redirect: null, state: { name: simple, pageId: null, path: pathname, query } }
  }
  return { redirect: '/console', state: { name: 'console', pageId: null, path: '/console', query: new URLSearchParams() } }
}

export const route = $state<RouteState>({
  name: 'wiki',
  pageId: null,
  path: '/wiki',
  query: new URLSearchParams(),
})

function applyState(next: RouteState) {
  route.name = next.name
  route.pageId = next.pageId
  route.path = next.path
  route.query = next.query
}

function syncFromLocation() {
  const parsed = parseLocation(window.location.pathname, window.location.search)
  if (parsed.redirect) {
    window.history.replaceState({}, '', parsed.redirect)
  }
  applyState(parsed.state)
}

let started = false

/** Start listening to popstate and sync the initial location. Call once from App. */
export function startRouter() {
  if (started) return
  started = true
  window.addEventListener('popstate', syncFromLocation)
  syncFromLocation()
}

/** Navigate to an app-relative URL (e.g. `/wiki/abc`, `/graph?seed=x`). */
export function navigate(to: string, opts?: { replace?: boolean }) {
  const url = new URL(to, window.location.origin)
  const target = url.pathname + url.search
  const current = window.location.pathname + window.location.search
  if (!opts?.replace && target === current) {
    applyState(parseLocation(url.pathname, url.search).state)
    return
  }
  if (opts?.replace) window.history.replaceState({}, '', target)
  else window.history.pushState({}, '', target)
  const parsed = parseLocation(url.pathname, url.search)
  if (parsed.redirect) {
    window.history.replaceState({}, '', parsed.redirect)
  }
  applyState(parsed.state)
}

/** Route helpers mirroring the old vue-router push/replace call sites. */
export function goWiki(id?: string, opts?: { replace?: boolean }) {
  navigate(id ? `/wiki/${encodeURIComponent(id)}` : '/wiki', opts)
}

export function goGraph(seed?: string | null, opts?: { replace?: boolean }) {
  const q = new URLSearchParams()
  if (seed) q.set('seed', seed)
  const s = q.toString()
  navigate(`/graph${s ? `?${s}` : ''}`, opts)
}

export function goSearch() {
  navigate('/search')
}

export function go(name: RouteName) {
  navigate(`/${name}`)
}

/** Seed param of the current /graph location (trimmed, null when absent). */
export function graphSeedParam(): string | null {
  if (route.name !== 'graph') return null
  const s = route.query.get('seed')
  return s && s.trim() ? s.trim() : null
}

/** Write/remove `?seed=` on /graph via history.replace (shareable, no push). */
export function setGraphSeedQuery(seed: string | null) {
  if (route.name !== 'graph') return
  const q = new URLSearchParams(route.query)
  if (seed) q.set('seed', seed)
  else q.delete('seed')
  const s = q.toString()
  navigate(`/graph${s ? `?${s}` : ''}`, { replace: true })
}
