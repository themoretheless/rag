import { untrack } from 'svelte'
import { api } from '@/api/client'
import type { WikiListParams, WikiPageMeta } from '@/api/types'
import { createRequestGate, debounce, errorMessage } from '@/lib/async'

/** Sidebar catalog facet chip: All / kind=wiki / category from pages. */
export type CatalogFacet =
  { type: 'all' } | { type: 'kind'; value: string } | { type: 'category'; value: string }

/** Min filter length before sending `q` to GET /v1/wiki. */
const SERVER_Q_MIN = 2
/** Debounce for filter-driven catalog reloads (ms). */
const FILTER_DEBOUNCE_MS = 250
const CATALOG_PAGE_SIZE = 50

function pageMatchesKind(p: WikiPageMeta, kind: string): boolean {
  const want = kind.toLowerCase()
  const k = (p.kind || '').toLowerCase()
  if (k === want) return true
  // Empty kind rows match filter "wiki" (gateway list convention).
  if (want === 'wiki' && (!k || p.uri.startsWith('wiki://'))) return true
  return false
}

function collectCategories(list: readonly WikiPageMeta[], known: Iterable<string> = []): string[] {
  const set = new Set<string>(known)
  for (const p of list) {
    const c = p.category?.trim()
    if (c) set.add(c)
  }
  return Array.from(set).sort((a, b) => a.localeCompare(b))
}

function facetsEqual(a: CatalogFacet, b: CatalogFacet): boolean {
  if (a.type !== b.type) return false
  if (a.type === 'all') return true
  if (a.type === 'kind' && b.type === 'kind') return a.value === b.value
  if (a.type === 'category' && b.type === 'category') return a.value === b.value
  return false
}

/**
 * The wiki catalog: offset-paginated page list, sidebar filter (server `q` from two
 * characters, client-side below that) and the kind / category facet.
 */
export class WikiCatalog {
  /** Loaded pages; replaced on every load, never mutated. */
  pages = $state.raw<WikiPageMeta[]>([])
  filter = $state('')
  /** Active filter chip: all | wiki (kind) | category from catalog. */
  facet = $state<CatalogFacet>({ type: 'all' })
  /** Distinct categories for chips (kept across kind/category server filters). */
  categories = $state.raw<string[]>([])
  loading = $state(false)
  error = $state<string | null>(null)
  /** Last server `q` used for `pages` (null = unfiltered catalog). */
  catalogQ = $state<string | null>(null)
  total = $state<number | null>(null)
  hasMore = $state(false)
  loadingMore = $state(false)
  loadMoreError = $state<string | null>(null)

  private readonly gate = createRequestGate()
  private params: WikiListParams = {}
  private nextOffset = 0

  /** `onPages` runs after every successful page load (first page or "load more"). */
  constructor(private readonly onPages: (pages: readonly WikiPageMeta[]) => void = () => undefined) {}

  readonly filtered = $derived.by(() => {
    let list = this.pages
    const f = this.facet
    // Client-side facet as a safety net (server already scopes via kind/category).
    if (f.type === 'kind') {
      list = list.filter((p) => pageMatchesKind(p, f.value))
    } else if (f.type === 'category') {
      const want = f.value.toLowerCase()
      list = list.filter((p) => (p.category ?? '').trim().toLowerCase() === want)
    }
    const q = this.filter.trim().toLowerCase()
    if (!q) return list
    // Server applies `q` when length >= SERVER_Q_MIN; still client-filter for
    // length-1 and snappy typing while a debounced reload is in flight.
    return list.filter((p) => {
      const hay = `${p.title} ${p.slug} ${p.summary ?? ''} ${p.category ?? ''} ${p.kind}`.toLowerCase()
      return hay.includes(q)
    })
  })

  private facetParams(): Pick<WikiListParams, 'kind' | 'category'> {
    const f = this.facet
    if (f.type === 'kind') return { kind: f.value }
    if (f.type === 'category') return { category: f.value }
    return {}
  }

  setFacet(next: CatalogFacet) {
    if (facetsEqual(this.facet, next)) return
    this.facet = next
    void this.load()
  }

  facetIsAll(): boolean {
    return this.facet.type === 'all'
  }

  facetIsKind(kind: string): boolean {
    return this.facet.type === 'kind' && this.facet.value.toLowerCase() === kind.toLowerCase()
  }

  facetIsCategory(category: string): boolean {
    return this.facet.type === 'category' && this.facet.value.toLowerCase() === category.toLowerCase()
  }

  /** Drop the filter and facet so a freshly created page is visible in the list. */
  clearScope() {
    if (this.filter.trim()) this.filter = ''
    if (this.facet.type !== 'all') this.facet = { type: 'all' }
  }

  /** A slug not used by any loaded page: `base`, `base-2`, `base-3`, … */
  uniqueSlug(base: string): string {
    const taken = new Set(this.pages.map((p) => p.slug))
    if (!taken.has(base)) return base
    let n = 2
    while (taken.has(`${base}-${n}`)) n += 1
    return `${base}-${n}`
  }

  private hasMoreAfter(batchLength: number, limit: number | undefined): boolean {
    return this.total !== null
      ? this.nextOffset < this.total
      : batchLength >= (limit ?? this.params.limit ?? CATALOG_PAGE_SIZE)
  }

  /**
   * Load wiki catalog. When sidebar `filter` is length >= 2 and caller did not
   * pass an explicit `q`, attach server `q=` so filtering runs in the gateway.
   * Active facet supplies `kind` / `category` unless the caller overrides them.
   */
  async load(params?: WikiListParams) {
    const trimmed = this.filter.trim()
    const merged: WikiListParams = {
      limit: CATALOG_PAGE_SIZE,
      offset: 0,
      ...this.facetParams(),
      ...params,
    }
    if (merged.q === undefined && trimmed.length >= SERVER_Q_MIN) {
      merged.q = trimmed
    }
    const ticket = this.gate.next()
    this.loading = true
    this.error = null
    this.hasMore = false
    this.loadingMore = false
    this.loadMoreError = null
    try {
      const res = await api.wikiList(merged)
      if (!ticket.current) return
      this.pages = res.pages
      this.params = merged
      this.nextOffset = (res.offset ?? merged.offset ?? 0) + this.pages.length
      this.total = res.total ?? null
      this.hasMore = this.hasMoreAfter(this.pages.length, res.limit)
      this.catalogQ = merged.q?.trim() ? merged.q.trim() : null
      // Rebuild chip categories from full unscoped catalog; otherwise union so
      // kind/category shelves do not wipe other category chips.
      const unscoped = !merged.kind && !merged.category
      if (unscoped) this.categories = collectCategories(this.pages)
      else if (this.pages.length) this.categories = collectCategories(this.pages, this.categories)
      this.onPages(this.pages)
    } catch (e) {
      if (!ticket.current) return
      this.error = errorMessage(e)
    } finally {
      if (ticket.current) this.loading = false
    }
  }

  /** Append the next offset page of the loaded query. */
  async loadMore() {
    if (!this.hasMore || this.loading || this.loadingMore) return
    const ticket = this.gate.latest
    const offset = this.nextOffset
    this.loadingMore = true
    this.loadMoreError = null
    try {
      const res = await api.wikiList({ ...this.params, offset })
      if (!ticket.current) return
      const batch = res.pages
      const known = new Set(this.pages.map((page) => page.id))
      const additions = batch.filter((page) => {
        if (known.has(page.id)) return false
        known.add(page.id)
        return true
      })
      if (
        (res.offset !== undefined && res.offset !== offset) ||
        additions.length !== batch.length ||
        (!batch.length && (res.total ?? this.total ?? 0) > offset)
      ) {
        throw new Error('Каталог изменился или сервер повторил страницу. Обновите список.')
      }
      this.pages = [...this.pages, ...additions]
      this.nextOffset = offset + batch.length
      this.total = res.total ?? this.total
      this.hasMore = this.hasMoreAfter(batch.length, res.limit)
      if (batch.length) this.categories = collectCategories(batch, this.categories)
      this.onPages(this.pages)
    } catch (cause) {
      if (ticket.current) this.loadMoreError = errorMessage(cause)
    } finally {
      if (ticket.current) this.loadingMore = false
    }
  }

  /**
   * Start the debounced server reload that follows the sidebar filter as it enters,
   * changes within, or leaves the server `q` range. Returns the stop function.
   */
  start(): () => void {
    const reload = debounce(() => void this.load(), FILTER_DEBOUNCE_MS)
    let prev = this.filter.trim()
    const stopEffect = $effect.root(() => {
      $effect(() => {
        const n = this.filter.trim()
        const p = prev
        prev = n
        const catalogQ = untrack(() => this.catalogQ)
        const needServer = n.length >= SERVER_Q_MIN || p.length >= SERVER_Q_MIN || catalogQ !== null
        if (!needServer) return
        if (n === p) return
        reload()
      })
    })
    return () => {
      reload.cancel()
      stopEffect()
    }
  }
}
