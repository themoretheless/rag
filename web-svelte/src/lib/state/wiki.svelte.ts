import { untrack } from 'svelte'
import { api } from '@/api/client'
import type { BacklinkItem, DocumentBody, WikiListParams, WikiPageMeta } from '@/api/types'
import { ui } from './ui.svelte'

/** Recently opened wiki page (MRU, client-only). */
export interface RecentPage {
  id: string
  title: string
  slug: string
}

/** Pinned favorite (client-only; order = pin order). Survives catalog facets. */
export interface FavoritePage {
  id: string
  title: string
  slug: string
  category?: string | null
  revision?: number
}

/** Sidebar catalog facet chip: All / kind=wiki / category from pages. */
export type CatalogFacet =
  | { type: 'all' }
  | { type: 'kind'; value: string }
  | { type: 'category'; value: string }

const RECENT_KEY = 'rag-wiki-recent'
const RECENT_MAX = 8
const FAVORITES_KEY = 'rag-wiki-favorites'
/** Min filter length before sending `q` to GET /v1/wiki. */
const SERVER_Q_MIN = 2
/** Debounce for filter-driven catalog reloads (ms). */
const FILTER_DEBOUNCE_MS = 250
/** sessionStorage key: map of wiki page id → article scrollTop */
const SCROLL_STORAGE_KEY = 'rag-wiki-scroll'

/** ASCII slug for wiki:// keys; mirrors gateway slugify defaults. */
export function slugifyTitle(title: string): string {
  let out = ''
  for (const c of title.trim()) {
    if (/[a-zA-Z0-9]/.test(c)) {
      out += c.toLowerCase()
    } else if (c === ' ' || c === '-' || c === '_') {
      if (out.length && !out.endsWith('-')) out += '-'
    }
  }
  out = out.replace(/^-+|-+$/g, '')
  return out || 'page'
}

function loadRecent(): RecentPage[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    return parsed
      .filter(
        (p): p is RecentPage =>
          !!p &&
          typeof p === 'object' &&
          typeof (p as RecentPage).id === 'string' &&
          typeof (p as RecentPage).title === 'string',
      )
      .map((p) => ({
        id: p.id,
        title: p.title,
        slug: typeof p.slug === 'string' ? p.slug : p.id,
      }))
      .slice(0, RECENT_MAX)
  } catch {
    return []
  }
}

/** Load favorites (supports string[] ids and FavoritePage[] shapes). */
function loadFavorites(): FavoritePage[] {
  try {
    const raw = localStorage.getItem(FAVORITES_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    const out: FavoritePage[] = []
    for (const x of parsed) {
      if (typeof x === 'string' && x.length > 0) {
        out.push({ id: x, title: x, slug: x })
        continue
      }
      if (
        x &&
        typeof x === 'object' &&
        typeof (x as FavoritePage).id === 'string' &&
        (x as FavoritePage).id.length > 0
      ) {
        const p = x as FavoritePage
        out.push({
          id: p.id,
          title: typeof p.title === 'string' ? p.title : p.id,
          slug: typeof p.slug === 'string' ? p.slug : p.id,
          category: typeof p.category === 'string' ? p.category : (p.category ?? null),
          revision: typeof p.revision === 'number' ? p.revision : undefined,
        })
      }
    }
    return out
  } catch {
    return []
  }
}

function readScrollMap(): Record<string, number> {
  try {
    const raw = sessionStorage.getItem(SCROLL_STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return {}
    return parsed as Record<string, number>
  } catch {
    return {}
  }
}

function writeScrollMap(map: Record<string, number>) {
  try {
    sessionStorage.setItem(SCROLL_STORAGE_KEY, JSON.stringify(map))
  } catch {
    /* quota / private mode */
  }
}

function pageMatchesKind(p: WikiPageMeta, kind: string): boolean {
  const want = kind.toLowerCase()
  const k = (p.kind || '').toLowerCase()
  if (k === want) return true
  // Empty kind rows match filter "wiki" (gateway list convention).
  if (want === 'wiki' && (!k || p.uri.startsWith('wiki://'))) return true
  return false
}

function collectCategories(list: WikiPageMeta[]): string[] {
  const set = new Set<string>()
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

/** Wiki catalog + open page + edit/save (CAS) + favorites/recent + scroll. */
class WikiStore {
  pages = $state<WikiPageMeta[]>([])
  filter = $state('')
  /** Active filter chip: all | wiki (kind) | category from catalog. */
  facet = $state<CatalogFacet>({ type: 'all' })
  /** Distinct categories for chips (kept across kind/category server filters). */
  categories = $state<string[]>([])
  private catalogLoading = $state(false)
  private pageLoading = $state(false)
  loading = $derived(this.catalogLoading || this.pageLoading)
  /** Catalog and page failures stay separate so one request cannot mislabel another. */
  catalogError = $state<string | null>(null)
  pageError = $state<string | null>(null)
  current = $state<DocumentBody | null>(null)
  backlinks = $state<BacklinkItem[]>([])
  editing = $state(false)
  draftTitle = $state('')
  draftContent = $state('')
  dirty = $state(false)
  history = $state<string[]>([])
  recent = $state<RecentPage[]>(loadRecent())
  favorites = $state<FavoritePage[]>(loadFavorites())
  /** Last server `q` used for `pages` (null = unfiltered catalog). */
  catalogQ = $state<string | null>(null)
  /**
   * When set, openPage for this id enters edit mode (create flow).
   * Kept until cancel/save or navigation to another page so concurrent
   * route watchers (SideNav + WikiView) do not race out of the editor.
   */
  pendingEditId = $state<string | null>(null)
  creating = $state(false)

  private catalogSeq = 0
  private pageSeq = 0

  filtered = $derived.by(() => {
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
      const hay =
        `${p.title} ${p.slug} ${p.summary ?? ''} ${p.category ?? ''} ${p.kind}`.toLowerCase()
      return hay.includes(q)
    })
  })

  /** Favorites for SideNav; catalog meta preferred when the page is loaded. */
  favoritePages = $derived.by((): FavoritePage[] => {
    const byId = new Map(this.pages.map((p) => [p.id, p]))
    return this.favorites.map((f) => {
      const meta = byId.get(f.id)
      if (!meta) return f
      return {
        id: meta.id,
        title: meta.title || f.title,
        slug: meta.slug || f.slug,
        category: meta.category ?? f.category ?? null,
        revision: meta.revision,
      }
    })
  })

  private facetParams(): Pick<WikiListParams, 'kind' | 'category'> {
    const f = this.facet
    if (f.type === 'kind') return { kind: f.value }
    if (f.type === 'category') return { category: f.value }
    return {}
  }

  private mergeCategoryChips(list: WikiPageMeta[], replace: boolean) {
    if (replace) {
      this.categories = collectCategories(list)
      return
    }
    if (!list.length) return
    const set = new Set(this.categories)
    for (const p of list) {
      const c = p.category?.trim()
      if (c) set.add(c)
    }
    this.categories = Array.from(set).sort((a, b) => a.localeCompare(b))
  }

  setFacet(next: CatalogFacet) {
    if (facetsEqual(this.facet, next)) return
    this.facet = next
    void this.loadCatalog()
  }

  facetIsAll(): boolean {
    return this.facet.type === 'all'
  }

  facetIsKind(kind: string): boolean {
    return this.facet.type === 'kind' && this.facet.value.toLowerCase() === kind.toLowerCase()
  }

  facetIsCategory(category: string): boolean {
    return (
      this.facet.type === 'category' &&
      this.facet.value.toLowerCase() === category.toLowerCase()
    )
  }

  private persistRecent() {
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(this.recent))
    } catch {
      /* quota / private mode */
    }
  }

  private persistFavorites() {
    try {
      localStorage.setItem(FAVORITES_KEY, JSON.stringify(this.favorites))
    } catch {
      /* quota / private mode */
    }
  }

  isFavorite(id: string): boolean {
    return this.favorites.some((f) => f.id === id)
  }

  toggleFavorite(id: string) {
    if (!id) return
    if (this.favorites.some((f) => f.id === id)) {
      this.favorites = this.favorites.filter((f) => f.id !== id)
      this.persistFavorites()
      return
    }
    const meta = this.pages.find((p) => p.id === id)
    const cur = this.current?.id === id ? this.current : null
    const slug = meta?.slug || (cur ? cur.uri.replace(/^wiki:\/\//, '') : '') || id
    const entry: FavoritePage = {
      id,
      title: meta?.title || cur?.title || slug || id,
      slug,
      category: meta?.category ?? null,
      revision: meta?.revision ?? cur?.revision ?? undefined,
    }
    this.favorites = [...this.favorites, entry]
    this.persistFavorites()
  }

  /** Refresh favorite titles/slugs from the catalog when available. */
  private syncFavoritesFromCatalog() {
    if (!this.pages.length || !this.favorites.length) return
    const byId = new Map(this.pages.map((p) => [p.id, p]))
    let changed = false
    const next = this.favorites.map((f) => {
      const meta = byId.get(f.id)
      if (!meta) return f
      if (
        meta.title === f.title &&
        meta.slug === f.slug &&
        (meta.category ?? null) === (f.category ?? null) &&
        meta.revision === f.revision
      ) {
        return f
      }
      changed = true
      return {
        id: f.id,
        title: meta.title || f.title,
        slug: meta.slug || f.slug,
        category: meta.category ?? null,
        revision: meta.revision,
      }
    })
    if (changed) {
      this.favorites = next
      this.persistFavorites()
    }
  }

  /** Push or promote a page to the head of the recent list (max 8). */
  touchRecent(page: RecentPage) {
    const entry: RecentPage = {
      id: page.id,
      title: page.title || page.slug || page.id,
      slug: page.slug || page.id,
    }
    this.recent = [entry, ...this.recent.filter((r) => r.id !== entry.id)].slice(0, RECENT_MAX)
    this.persistRecent()
  }

  /** Refresh recent titles/slugs from the catalog when available. */
  private syncRecentFromCatalog() {
    if (!this.pages.length || !this.recent.length) return
    const byId = new Map(this.pages.map((p) => [p.id, p]))
    let changed = false
    const next = this.recent.map((r) => {
      const meta = byId.get(r.id)
      if (!meta) return r
      if (meta.title === r.title && meta.slug === r.slug) return r
      changed = true
      return { id: r.id, title: meta.title || r.title, slug: meta.slug || r.slug }
    })
    if (changed) {
      this.recent = next
      this.persistRecent()
    }
  }

  private uniqueSlug(base: string): string {
    const taken = new Set(this.pages.map((p) => p.slug))
    if (!taken.has(base)) return base
    let n = 2
    while (taken.has(`${base}-${n}`)) n += 1
    return `${base}-${n}`
  }

  /**
   * Load wiki catalog. When sidebar `filter` is length >= 2 and caller did not
   * pass an explicit `q`, attach server `q=` so filtering runs in the gateway.
   * Active facet supplies `kind` / `category` unless the caller overrides them.
   */
  async loadCatalog(params?: WikiListParams) {
    const trimmed = this.filter.trim()
    const merged: WikiListParams = { ...this.facetParams(), ...params }
    if (merged.q === undefined && trimmed.length >= SERVER_Q_MIN) {
      merged.q = trimmed
    }
    const seq = ++this.catalogSeq
    this.catalogLoading = true
    this.catalogError = null
    try {
      const res = await api.wikiList(merged)
      if (seq !== this.catalogSeq) return
      this.pages = res.pages ?? []
      this.catalogQ = merged.q?.trim() ? merged.q.trim() : null
      // Rebuild chip categories from full unscoped catalog; otherwise union so
      // kind/category shelves do not wipe other category chips.
      const unscoped = !merged.kind && !merged.category
      this.mergeCategoryChips(this.pages, unscoped)
      this.syncRecentFromCatalog()
      this.syncFavoritesFromCatalog()
    } catch (e) {
      if (seq !== this.catalogSeq) return
      this.catalogError = e instanceof Error ? e.message : String(e)
    } finally {
      if (seq === this.catalogSeq) this.catalogLoading = false
    }
  }

  async openPage(id: string, pushHistory = true) {
    const seq = ++this.pageSeq
    const previousId = this.current?.id ?? null
    this.pageLoading = true
    this.pageError = null
    try {
      const doc = await api.document({ id })
      if (seq !== this.pageSeq) return
      if (pushHistory && previousId && previousId !== id) this.history.push(previousId)
      this.current = doc
      this.draftTitle = doc.title
      this.draftContent = doc.content
      this.dirty = false
      if (this.pendingEditId && this.pendingEditId !== id) {
        this.pendingEditId = null
      }
      this.editing = this.pendingEditId === id
      const slug = doc.uri.replace(/^wiki:\/\//, '') || doc.id
      this.touchRecent({ id: doc.id, title: doc.title, slug })
      try {
        const bl = await api.backlinks(id)
        if (seq !== this.pageSeq) return
        this.backlinks = bl.backlinks ?? []
      } catch {
        if (seq === this.pageSeq) this.backlinks = []
      }
    } catch (e) {
      if (seq === this.pageSeq) {
        this.pageError = e instanceof Error ? e.message : String(e)
        this.current = null
      }
    } finally {
      if (seq === this.pageSeq) this.pageLoading = false
    }
  }

  goBack() {
    const prev = this.history.pop()
    if (prev) void this.openPage(prev, false)
  }

  /** Leave the open page (route to /wiki root shows the home dashboard). */
  closePage() {
    ++this.pageSeq
    this.pageLoading = false
    this.current = null
    this.backlinks = []
    this.editing = false
    this.dirty = false
    this.pendingEditId = null
    this.pageError = null
  }

  startEdit() {
    if (!this.current) return
    this.draftTitle = this.current.title
    this.draftContent = this.current.content
    this.editing = true
    this.dirty = false
  }

  cancelEdit() {
    this.editing = false
    this.dirty = false
    this.pendingEditId = null
    if (this.current) {
      this.draftTitle = this.current.title
      this.draftContent = this.current.content
    }
  }

  /**
   * Prompt for a title (unless given), PUT /v1/wiki to create, refresh catalog.
   * Returns the new document id; caller should route to /wiki/:id so openPage
   * enters the editor via pendingEditId.
   */
  async createPage(title?: string): Promise<string | null> {
    if (this.creating) return null
    let t = title?.trim() ?? ''
    if (!t) {
      const raw = window.prompt(ui.t('createPrompt'))
      if (raw == null) return null
      t = raw.trim()
    }
    if (!t) {
      ui.toast(ui.t('createTitleRequired'), 'error')
      return null
    }
    const slug = this.uniqueSlug(slugifyTitle(t))
    this.creating = true
    try {
      const res = await api.putWiki({
        slug,
        title: t,
        content: `# ${t}\n\n`,
        kind: 'wiki',
      })
      const id = res.document_id
      if (!id) {
        ui.toast(ui.t('createNoId'), 'error')
        return null
      }
      this.pendingEditId = id
      // Clear filter/facet so the new page is visible in the catalog list.
      if (this.filter.trim()) this.filter = ''
      if (this.facet.type !== 'all') this.facet = { type: 'all' }
      await this.loadCatalog()
      this.touchRecent({ id, title: t, slug })
      ui.toast(ui.t('pageCreated'), 'ok')
      return id
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      ui.toast(msg, 'error')
      throw e
    } finally {
      this.creating = false
    }
  }

  private isCasConflict(e: unknown): boolean {
    const msg = e instanceof Error ? e.message : String(e)
    // api client throws `HTTP ${status}: …`; gateway may also say "conflict"
    return /\bHTTP 409\b/.test(msg) || /\bconflict\b/i.test(msg)
  }

  /** After a 409: toast, re-fetch remote, offer keep-draft re-save vs reload. */
  private async handleCasConflict(id: string, slug: string) {
    ui.toast(ui.t('saveConflictToast'), 'error')

    let remote: DocumentBody
    try {
      remote = await api.document({ id })
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      ui.toast(ui.t('saveConflictReloadFail', { msg }), 'error')
      throw e
    }

    const keepDraft = window.confirm(
      [
        ui.t('saveConflictTitle'),
        '',
        ui.t('saveConflictRemote', { rev: remote.revision ?? '?', title: remote.title }),
        ui.t('saveConflictDraft', { title: this.draftTitle }),
        '',
        `OK - ${ui.t('saveConflictKeep')}`,
        `Cancel - ${ui.t('saveConflictDiscard')}`,
      ].join('\n'),
    )

    if (!keepDraft) {
      this.pendingEditId = null
      this.current = remote
      this.draftTitle = remote.title
      this.draftContent = remote.content
      this.dirty = false
      this.editing = false
      try {
        const bl = await api.backlinks(id)
        this.backlinks = bl.backlinks ?? []
      } catch {
        this.backlinks = []
      }
      ui.toast(ui.t('saveConflictReloaded'), 'info')
      return
    }

    // Adopt remote CAS tokens; keep local draft title/content for overwrite re-save.
    this.current = remote
    this.editing = true
    this.dirty = true

    try {
      const res = await api.putWiki({
        slug: remote.uri.replace(/^wiki:\/\//, '') || slug || remote.id,
        id: remote.id,
        uri: remote.uri,
        title: this.draftTitle,
        content: this.draftContent,
        if_match_revision: remote.revision ?? undefined,
        if_match_etag: remote.etag ?? undefined,
      })
      this.pendingEditId = null
      await this.openPage(res.document_id || remote.id, false)
      await this.loadCatalog()
      this.editing = false
      this.dirty = false
      ui.toast(ui.t('saveConflictSavedOver'), 'ok')
    } catch (e) {
      if (this.isCasConflict(e)) {
        ui.toast(ui.t('saveConflictStillFailing'), 'error')
      } else {
        const msg = e instanceof Error ? e.message : String(e)
        ui.toast(msg, 'error')
      }
      throw e
    }
  }

  async save() {
    const cur = this.current
    if (!cur) return
    const slug = cur.uri.replace(/^wiki:\/\//, '') || cur.id
    try {
      const res = await api.putWiki({
        slug,
        id: cur.id,
        uri: cur.uri,
        title: this.draftTitle,
        content: this.draftContent,
        if_match_revision: cur.revision ?? undefined,
        if_match_etag: cur.etag ?? undefined,
      })
      this.pendingEditId = null
      await this.openPage(res.document_id || cur.id, false)
      await this.loadCatalog()
      this.editing = false
      this.dirty = false
      ui.toast(ui.t('saved'), 'ok')
    } catch (e) {
      if (this.isCasConflict(e)) {
        await this.handleCasConflict(cur.id, slug)
        return
      }
      const msg = e instanceof Error ? e.message : String(e)
      ui.toast(msg, 'error')
      throw e
    }
  }

  markDirty() {
    this.dirty = true
  }

  saveScrollPosition(pageId: string, top: number) {
    if (!pageId) return
    const y = Math.max(0, Math.round(top))
    const map = readScrollMap()
    if (map[pageId] === y) return
    map[pageId] = y
    writeScrollMap(map)
  }

  getScrollPosition(pageId: string): number {
    if (!pageId) return 0
    const y = readScrollMap()[pageId]
    return typeof y === 'number' && Number.isFinite(y) ? Math.max(0, y) : 0
  }

  clearScrollPosition(pageId: string) {
    if (!pageId) return
    const map = readScrollMap()
    if (!(pageId in map)) return
    delete map[pageId]
    writeScrollMap(map)
  }
}

export const wiki = new WikiStore()

// Debounced server reload when filter enters/changes/leaves the `q` range.
$effect.root(() => {
  let prev = wiki.filter.trim()
  let filterTimer: ReturnType<typeof setTimeout> | null = null
  $effect(() => {
    const n = wiki.filter.trim()
    const p = prev
    prev = n
    const catalogQ = untrack(() => wiki.catalogQ)
    const needServer =
      n.length >= SERVER_Q_MIN || p.length >= SERVER_Q_MIN || catalogQ !== null
    if (!needServer) return
    if (n === p) return
    if (filterTimer != null) clearTimeout(filterTimer)
    filterTimer = setTimeout(() => {
      filterTimer = null
      void wiki.loadCatalog()
    }, FILTER_DEBOUNCE_MS)
  })
})
