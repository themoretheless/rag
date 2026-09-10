import { untrack } from 'svelte'
import { api } from '@/api/client'
import type { BacklinkItem, DocumentBody, WikiListParams, WikiPageMeta, WikiPutResult } from '@/api/types'
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
const CATALOG_PAGE_SIZE = 50
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
export class WikiStore {
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
  catalogTotal = $state<number | null>(null)
  catalogHasMore = $state(false)
  loadingMore = $state(false)
  loadMoreError = $state<string | null>(null)
  /**
   * When set, openPage for this id enters edit mode (create flow).
   * Kept until cancel/save or navigation to another page so concurrent
   * route watchers (SideNav + WikiView) do not race out of the editor.
   */
  pendingEditId = $state<string | null>(null)
  creating = $state(false)
  saving = $state(false)

  private catalogSeq = 0
  private pageSeq = 0
  private catalogParams: WikiListParams = {}
  private catalogNextOffset = 0
  private drafts = new Map<string, { title: string; content: string }>()

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
    const merged: WikiListParams = { limit: CATALOG_PAGE_SIZE, offset: 0, ...this.facetParams(), ...params }
    if (merged.q === undefined && trimmed.length >= SERVER_Q_MIN) {
      merged.q = trimmed
    }
    const seq = ++this.catalogSeq
    this.catalogLoading = true
    this.catalogError = null
    this.catalogHasMore = false
    this.loadingMore = false
    this.loadMoreError = null
    try {
      const res = await api.wikiList(merged)
      if (seq !== this.catalogSeq) return
      this.pages = res.pages ?? []
      this.catalogParams = merged
      this.catalogNextOffset = (res.offset ?? merged.offset ?? 0) + this.pages.length
      this.catalogTotal = res.total ?? null
      this.catalogHasMore = this.catalogTotal !== null
        ? this.catalogNextOffset < this.catalogTotal
        : this.pages.length >= (res.limit ?? merged.limit ?? CATALOG_PAGE_SIZE)
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

  async loadMore() {
    if (!this.catalogHasMore || this.catalogLoading || this.loadingMore) return
    const seq = this.catalogSeq
    const offset = this.catalogNextOffset
    this.loadingMore = true
    this.loadMoreError = null
    try {
      const res = await api.wikiList({ ...this.catalogParams, offset })
      if (seq !== this.catalogSeq) return
      const batch = res.pages ?? []
      const known = new Set(this.pages.map((page) => page.id))
      const additions = batch.filter((page) => {
        if (known.has(page.id)) return false
        known.add(page.id)
        return true
      })
      if ((res.offset !== undefined && res.offset !== offset) || additions.length !== batch.length || (!batch.length && (res.total ?? this.catalogTotal ?? 0) > offset)) {
        throw new Error('Каталог изменился или сервер повторил страницу. Обновите список.')
      }
      this.pages = [...this.pages, ...additions]
      this.catalogNextOffset = offset + batch.length
      this.catalogTotal = res.total ?? this.catalogTotal
      this.catalogHasMore = this.catalogTotal !== null
        ? this.catalogNextOffset < this.catalogTotal
        : batch.length >= (res.limit ?? this.catalogParams.limit ?? CATALOG_PAGE_SIZE)
      this.mergeCategoryChips(batch, false)
      this.syncRecentFromCatalog()
      this.syncFavoritesFromCatalog()
    } catch (cause) {
      if (seq === this.catalogSeq) this.loadMoreError = cause instanceof Error ? cause.message : String(cause)
    } finally {
      if (seq === this.catalogSeq) this.loadingMore = false
    }
  }

  private rememberDraft() {
    if (this.current && this.editing && this.dirty) {
      this.drafts.set(this.current.id, { title: this.draftTitle, content: this.draftContent })
    }
  }

  async openPage(id: string, pushHistory = true) {
    this.rememberDraft()
    const seq = ++this.pageSeq
    const previousId = this.current?.id ?? null
    this.pageLoading = true
    this.pageError = null
    try {
      const doc = await api.document({ id })
      if (seq !== this.pageSeq) return
      if (pushHistory && previousId && previousId !== id) this.history.push(previousId)
      this.rememberDraft()
      this.current = doc
      const draft = this.drafts.get(id)
      this.draftTitle = draft?.title ?? doc.title
      this.draftContent = draft?.content ?? doc.content
      this.dirty = Boolean(draft && (draft.title !== doc.title || draft.content !== doc.content))
      if (this.pendingEditId && this.pendingEditId !== id) {
        this.pendingEditId = null
      }
      this.editing = Boolean(draft) || this.pendingEditId === id
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
    this.rememberDraft()
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
    if (!this.current || this.saving || this.editing) return
    this.draftTitle = this.current.title
    this.draftContent = this.current.content
    this.editing = true
    this.dirty = false
  }

  cancelEdit() {
    if (this.saving) return
    this.editing = false
    this.dirty = false
    this.pendingEditId = null
    if (this.current) {
      this.drafts.delete(this.current.id)
      this.draftTitle = this.current.title
      this.draftContent = this.current.content
    }
  }

  /**
   * Prompt for a title (unless given), POST /v1/wiki to create, refresh catalog.
   * Returns the new document id; caller should route to /wiki/:id so openPage
   * enters the editor via pendingEditId.
   */
  async createPage(title?: string): Promise<string | null> {
    if (this.creating || this.saving) return null
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
      const res = await api.createWiki({
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
      if (this.isCasConflict(e)) {
        ui.toast(ui.locale === 'ru'
          ? 'Страница с таким адресом уже существует. Выберите другое название.'
          : 'A page with this address already exists. Choose another title.', 'error')
        return null
      }
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

  private isCurrentPage(id: string, seq: number): boolean {
    return this.pageSeq === seq && this.current?.id === id
  }

  private async writeSnapshot(cur: DocumentBody, title: string, content: string) {
    return api.putWiki({
      slug: cur.uri.replace(/^wiki:\/\//, '') || cur.id,
      id: cur.id,
      uri: cur.uri,
      title,
      content,
      if_match_revision: cur.revision ?? undefined,
      if_match_etag: cur.etag ?? undefined,
    })
  }

  /** Advance only the saved baseline; newer keystrokes remain an editable draft. */
  private applySaved(cur: DocumentBody, seq: number, title: string, content: string, res: WikiPutResult) {
    const cached = this.drafts.get(cur.id)
    if (cached?.title === title && cached.content === content) this.drafts.delete(cur.id)
    if (!this.isCurrentPage(cur.id, seq)) return
    this.current = { ...cur, title, content, revision: res.revision, etag: res.etag }
    this.pendingEditId = null
    this.dirty = this.draftTitle !== title || this.draftContent !== content
    this.editing = this.dirty
    if (this.dirty) this.rememberDraft()
    else this.drafts.delete(cur.id)
    this.touchRecent({ id: cur.id, title, slug: res.slug })
  }

  /** A conflict dialog belongs only to the page where this save started. */
  private async handleCasConflict(cur: DocumentBody, seq: number) {
    if (!this.isCurrentPage(cur.id, seq)) return
    ui.toast(ui.t('saveConflictToast'), 'error')
    const remote = await api.document({ id: cur.id })
    if (!this.isCurrentPage(cur.id, seq)) return
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
    if (!this.isCurrentPage(cur.id, seq)) return
    this.current = remote
    if (!keepDraft) {
      this.drafts.delete(cur.id)
      this.pendingEditId = null
      this.draftTitle = remote.title
      this.draftContent = remote.content
      this.dirty = false
      this.editing = false
      try {
        const bl = await api.backlinks(cur.id)
        if (this.isCurrentPage(cur.id, seq)) this.backlinks = bl.backlinks ?? []
      } catch {
        if (this.isCurrentPage(cur.id, seq)) this.backlinks = []
      }
      ui.toast(ui.t('saveConflictReloaded'), 'info')
      return
    }

    // Snapshot the latest draft after the user explicitly chose to re-save.
    const title = this.draftTitle
    const content = this.draftContent
    this.editing = true
    this.dirty = true
    try {
      const res = await this.writeSnapshot(remote, title, content)
      this.applySaved(remote, seq, title, content, res)
      await this.loadCatalog()
      ui.toast(ui.t('saveConflictSavedOver'), 'ok')
    } catch (cause) {
      if (this.isCasConflict(cause)) ui.toast(ui.t('saveConflictStillFailing'), 'error')
      throw cause
    }
  }

  async save() {
    const cur = this.current
    if (!cur || this.saving || !this.editing || !this.dirty) return
    const seq = this.pageSeq
    const title = this.draftTitle
    const content = this.draftContent
    this.saving = true
    try {
      try {
        const res = await this.writeSnapshot(cur, title, content)
        this.applySaved(cur, seq, title, content, res)
        await this.loadCatalog()
        ui.toast(ui.t('saved'), 'ok')
      } catch (cause) {
        if (!this.isCasConflict(cause)) throw cause
        await this.handleCasConflict(cur, seq)
      }
    } catch (cause) {
      ui.toast(cause instanceof Error ? cause.message : String(cause), 'error')
      throw cause
    } finally {
      this.saving = false
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
