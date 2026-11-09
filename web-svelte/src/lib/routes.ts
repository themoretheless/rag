import type { Component } from 'svelte'

/** Left-rail entry of a route. */
export interface RouteNav {
  label: string
  /** `NavIcon` glyph name. */
  icon: string
  /** Layer accent class (`l0`…`l4`) for the active marker. */
  layer?: string
  /** `bottom` pins the entry to the foot of the rail. */
  slot?: 'bottom'
}

/** Top-bar breadcrumb of a route. */
export interface RouteCrumb {
  title: string
  layer: string
  color: string
}

interface RouteDef {
  name: string
  /** Exact pathname of the screen. */
  path: string
  /** When true, `${path}/:param` is accepted too (the wiki page id). */
  param?: boolean
  /** Lazy screen component; each screen is its own chunk. */
  load: () => Promise<{ default: Component }>
  nav?: RouteNav
  crumb?: RouteCrumb
}

/** Breadcrumb of the default route; also shown for routes that declare none. */
const DEFAULT_CRUMB: RouteCrumb = { title: 'Пульт', layer: 'L0–L4 · обзор', color: 'var(--accent)' }

/**
 * The single route table: it drives `RouteName`, URL parsing, the screen rendered
 * by App, the side rail and the top-bar breadcrumb. Rail order = table order.
 */
export const ROUTES = [
  {
    name: 'console',
    path: '/console',
    load: () => import('@/views/ConsoleView.svelte'),
    nav: { label: 'Пульт', icon: 'console' },
    crumb: DEFAULT_CRUMB,
  },
  {
    name: 'corpus',
    path: '/corpus',
    load: () => import('@/views/CorpusView.svelte'),
    nav: { label: 'Корпус', icon: 'corpus', layer: 'l0' },
    crumb: { title: 'Корпус', layer: 'L0 · сырой корпус', color: 'var(--l0)' },
  },
  {
    name: 'search',
    path: '/search',
    load: () => import('@/views/SearchView.svelte'),
    nav: { label: 'Поиск', icon: 'search', layer: 'l1' },
    crumb: { title: 'Поиск', layer: 'L1 · извлечение', color: 'var(--l1)' },
  },
  {
    name: 'graph',
    path: '/graph',
    load: () => import('@/views/GraphView.svelte'),
    nav: { label: 'Граф', icon: 'graph', layer: 'l2' },
    crumb: { title: 'Граф', layer: 'L2 · граф объектов', color: 'var(--l2)' },
  },
  {
    name: 'wiki',
    path: '/wiki',
    param: true,
    load: () => import('@/views/WikiView.svelte'),
    nav: { label: 'Вики', icon: 'wiki', layer: 'l3' },
    crumb: { title: 'Вики', layer: 'L3 · знание', color: 'var(--l3)' },
  },
  {
    name: 'agents',
    path: '/agents',
    load: () => import('@/views/AgentsView.svelte'),
    nav: { label: 'Агенты', icon: 'agents', layer: 'l4' },
    crumb: { title: 'Агенты · Журнал', layer: 'L4 · MCP-клиенты', color: 'var(--l4)' },
  },
  {
    name: 'sync',
    path: '/sync',
    load: () => import('@/views/SyncView.svelte'),
    nav: { label: 'Синхр.', icon: 'sync' },
    crumb: { title: 'Синхронизация БД', layer: 'local ↔ primary', color: 'var(--l2)' },
  },
  {
    name: 'evaluation',
    path: '/evaluation',
    load: () => import('@/views/EvaluationView.svelte'),
    nav: { label: 'Оценка', icon: 'evaluation' },
    crumb: { title: 'Оценка', layer: 'eval · retrieval', color: 'var(--l1)' },
  },
  {
    // No breadcrumb of its own yet: the top bar falls back to the default route's.
    name: 'resources',
    path: '/resources',
    load: () => import('@/views/ResourcesView.svelte'),
    nav: { label: 'Ресурсы', icon: 'resources' },
  },
  {
    name: 'models',
    path: '/models',
    load: () => import('@/views/ModelsView.svelte'),
    nav: { label: 'Модели', icon: 'models', slot: 'bottom' },
    crumb: { title: 'Модели и пайплайн', layer: 'RAG_* env · runtime', color: 'var(--l3)' },
  },
] as const satisfies readonly RouteDef[]

export type RouteName = (typeof ROUTES)[number]['name']
export type RouteEntry = RouteDef & { name: RouteName }

const TABLE: readonly RouteEntry[] = ROUTES

/** Where `/` and unknown paths land. */
export const DEFAULT_ROUTE: RouteEntry = ROUTES[0]

export function routeByName(name: RouteName): RouteEntry {
  return TABLE.find((entry) => entry.name === name) ?? DEFAULT_ROUTE
}

export function routeCrumb(name: RouteName): RouteCrumb {
  return routeByName(name).crumb ?? DEFAULT_CRUMB
}

/** Rail entries in table order; `slot` selects the main list or the pinned foot. */
export function navRoutes(slot: 'main' | 'bottom'): (RouteEntry & { nav: RouteNav })[] {
  return TABLE.filter(
    (entry): entry is RouteEntry & { nav: RouteNav } =>
      entry.nav !== undefined && (entry.nav.slot === 'bottom') === (slot === 'bottom'),
  )
}

/**
 * Resolve a pathname against the table. `param` is the decoded trailing segment of a
 * parameterised route (`/wiki/:id`), null when absent. Null result = unknown path.
 */
export function matchRoute(pathname: string): { entry: RouteEntry; param: string | null } | null {
  for (const entry of TABLE) {
    if (pathname === entry.path) return { entry, param: null }
    if (entry.param && pathname.startsWith(`${entry.path}/`)) {
      const rest = pathname.slice(entry.path.length + 1)
      return { entry, param: rest ? decodeURIComponent(rest) : null }
    }
  }
  return null
}

/** Warm a screen's chunk (e.g. on rail hover) without rendering it. */
export function preloadRoute(name: RouteName): void {
  void routeByName(name)
    .load()
    .catch(() => undefined)
}
