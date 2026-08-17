import { defineStore } from 'pinia'
import { computed, ref, watch } from 'vue'
import { api } from '@/api/client'
import type { BacklinkItem, DocumentBody, WikiListParams, WikiPageMeta } from '@/api/types'
import { useUiStore } from './ui'

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

export const useWikiStore = defineStore('wiki', () => {
  const pages = ref<WikiPageMeta[]>([])
  const filter = ref('')
  /** Active filter chip: all | wiki (kind) | category from catalog. */
  const facet = ref<CatalogFacet>({ type: 'all' })
  /** Distinct categories for chips (kept across kind/category server filters). */
  const categories = ref<string[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const current = ref<DocumentBody | null>(null)
  const backlinks = ref<BacklinkItem[]>([])
  const editing = ref(false)
  const draftTitle = ref('')
  const draftContent = ref('')
  const dirty = ref(false)
  const history = ref<string[]>([])
  const recent = ref<RecentPage[]>(loadRecent())
  const favorites = ref<FavoritePage[]>(loadFavorites())
  /** Last server `q` used for `pages` (null = unfiltered catalog). */
  const catalogQ = ref<string | null>(null)
  /**
   * When set, openPage for this id enters edit mode (create flow).
   * Kept until cancel/save or navigation to another page so concurrent
   * route watchers (SideNav + WikiView) do not race out of the editor.
   */
  const pendingEditId = ref<string | null>(null)
  const creating = ref(false)

  let filterTimer: ReturnType<typeof setTimeout> | null = null
  let catalogSeq = 0

  const filtered = computed(() => {
    let list = pages.value
    const f = facet.value
    // Client-side facet as a safety net (server already scopes via kind/category).
    if (f.type === 'kind') {
      list = list.filter((p) => pageMatchesKind(p, f.value))
    } else if (f.type === 'category') {
      const want = f.value.toLowerCase()
      list = list.filter((p) => (p.category ?? '').trim().toLowerCase() === want)
    }
    const q = filter.value.trim().toLowerCase()
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
  const favoritePages = computed((): FavoritePage[] => {
    const byId = new Map(pages.value.map((p) => [p.id, p]))
    return favorites.value.map((f) => {
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

  function facetParams(): Pick<WikiListParams, 'kind' | 'category'> {
    const f = facet.value
    if (f.type === 'kind') return { kind: f.value }
    if (f.type === 'category') return { category: f.value }
    return {}
  }

  function mergeCategoryChips(list: WikiPageMeta[], replace: boolean) {
    if (replace) {
      categories.value = collectCategories(list)
      return
    }
    if (!list.length) return
    const set = new Set(categories.value)
    for (const p of list) {
      const c = p.category?.trim()
      if (c) set.add(c)
    }
    categories.value = Array.from(set).sort((a, b) => a.localeCompare(b))
  }

  function setFacet(next: CatalogFacet) {
    if (facetsEqual(facet.value, next)) return
    facet.value = next
    void loadCatalog()
  }

  function facetIsAll(): boolean {
    return facet.value.type === 'all'
  }

  function facetIsKind(kind: string): boolean {
    return facet.value.type === 'kind' && facet.value.value.toLowerCase() === kind.toLowerCase()
  }

  function facetIsCategory(category: string): boolean {
    return (
      facet.value.type === 'category' &&
      facet.value.value.toLowerCase() === category.toLowerCase()
    )
  }

  function persistRecent() {
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(recent.value))
    } catch {
      /* quota / private mode */
    }
  }

  function persistFavorites() {
    try {
      localStorage.setItem(FAVORITES_KEY, JSON.stringify(favorites.value))
    } catch {
      /* quota / private mode */
    }
  }

  function isFavorite(id: string): boolean {
    return favorites.value.some((f) => f.id === id)
  }

  function toggleFavorite(id: string) {
    if (!id) return
    if (favorites.value.some((f) => f.id === id)) {
      favorites.value = favorites.value.filter((f) => f.id !== id)
      persistFavorites()
      return
    }
    const meta = pages.value.find((p) => p.id === id)
    const cur = current.value?.id === id ? current.value : null
    const slug =
      meta?.slug || (cur ? cur.uri.replace(/^wiki:\/\//, '') : '') || id
    const entry: FavoritePage = {
      id,
      title: meta?.title || cur?.title || slug || id,
      slug,
      category: meta?.category ?? null,
      revision: meta?.revision ?? cur?.revision ?? undefined,
    }
    favorites.value = [...favorites.value, entry]
    persistFavorites()
  }

  /** Refresh favorite titles/slugs from the catalog when available. */
  function syncFavoritesFromCatalog() {
    if (!pages.value.length || !favorites.value.length) return
    const byId = new Map(pages.value.map((p) => [p.id, p]))
    let changed = false
    const next = favorites.value.map((f) => {
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
      favorites.value = next
      persistFavorites()
    }
  }

  /** Push or promote a page to the head of the recent list (max 8). */
  function touchRecent(page: RecentPage) {
    const entry: RecentPage = {
      id: page.id,
      title: page.title || page.slug || page.id,
      slug: page.slug || page.id,
    }
    recent.value = [entry, ...recent.value.filter((r) => r.id !== entry.id)].slice(0, RECENT_MAX)
    persistRecent()
  }

  /** Refresh recent titles/slugs from the catalog when available. */
  function syncRecentFromCatalog() {
    if (!pages.value.length || !recent.value.length) return
    const byId = new Map(pages.value.map((p) => [p.id, p]))
    let changed = false
    const next = recent.value.map((r) => {
      const meta = byId.get(r.id)
      if (!meta) return r
      if (meta.title === r.title && meta.slug === r.slug) return r
      changed = true
      return { id: r.id, title: meta.title || r.title, slug: meta.slug || r.slug }
    })
    if (changed) {
      recent.value = next
      persistRecent()
    }
  }

  function uniqueSlug(base: string): string {
    const taken = new Set(pages.value.map((p) => p.slug))
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
  async function loadCatalog(params?: WikiListParams) {
    const trimmed = filter.value.trim()
    const merged: WikiListParams = { ...facetParams(), ...params }
    if (merged.q === undefined && trimmed.length >= SERVER_Q_MIN) {
      merged.q = trimmed
    }
    const seq = ++catalogSeq
    loading.value = true
    error.value = null
    try {
      const res = await api.wikiList(merged)
      if (seq !== catalogSeq) return
      pages.value = res.pages ?? []
      catalogQ.value = merged.q?.trim() ? merged.q.trim() : null
      // Rebuild chip categories from full unscoped catalog; otherwise union so
      // kind/category shelves do not wipe other category chips.
      const unscoped = !merged.kind && !merged.category
      mergeCategoryChips(pages.value, unscoped)
      syncRecentFromCatalog()
      syncFavoritesFromCatalog()
    } catch (e) {
      if (seq !== catalogSeq) return
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      if (seq === catalogSeq) loading.value = false
    }
  }

  // Debounced server reload when filter enters/changes/leaves the `q` range.
  watch(filter, (next, prev) => {
    const n = next.trim()
    const p = (prev ?? '').trim()
    const needServer =
      n.length >= SERVER_Q_MIN || p.length >= SERVER_Q_MIN || catalogQ.value !== null
    if (!needServer) return
    if (n === p) return
    if (filterTimer != null) clearTimeout(filterTimer)
    filterTimer = setTimeout(() => {
      filterTimer = null
      void loadCatalog()
    }, FILTER_DEBOUNCE_MS)
  })

  async function openPage(id: string, pushHistory = true) {
    loading.value = true
    error.value = null
    try {
      if (pushHistory && current.value?.id && current.value.id !== id) {
        history.value.push(current.value.id)
      }
      const doc = await api.document({ id })
      current.value = doc
      draftTitle.value = doc.title
      draftContent.value = doc.content
      dirty.value = false
      if (pendingEditId.value && pendingEditId.value !== id) {
        pendingEditId.value = null
      }
      editing.value = pendingEditId.value === id
      const slug = doc.uri.replace(/^wiki:\/\//, '') || doc.id
      touchRecent({ id: doc.id, title: doc.title, slug })
      try {
        const bl = await api.backlinks(id)
        backlinks.value = bl.backlinks ?? []
      } catch {
        backlinks.value = []
      }
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
      current.value = null
    } finally {
      loading.value = false
    }
  }

  function goBack() {
    const prev = history.value.pop()
    if (prev) void openPage(prev, false)
  }

  function startEdit() {
    if (!current.value) return
    draftTitle.value = current.value.title
    draftContent.value = current.value.content
    editing.value = true
    dirty.value = false
  }

  function cancelEdit() {
    editing.value = false
    dirty.value = false
    pendingEditId.value = null
    if (current.value) {
      draftTitle.value = current.value.title
      draftContent.value = current.value.content
    }
  }

  /**
   * Prompt for a title (unless given), PUT /v1/wiki to create, refresh catalog.
   * Returns the new document id; caller should route to /wiki/:id so openPage
   * enters the editor via pendingEditId.
   */
  async function createPage(title?: string): Promise<string | null> {
    const ui = useUiStore()
    if (creating.value) return null
    let t = title?.trim() ?? ''
    if (!t) {
      const raw = window.prompt('New page title')
      if (raw == null) return null
      t = raw.trim()
    }
    if (!t) {
      ui.toast('Title is required', 'error')
      return null
    }
    const slug = uniqueSlug(slugifyTitle(t))
    creating.value = true
    try {
      const res = await api.putWiki({
        slug,
        title: t,
        content: `# ${t}\n\n`,
        kind: 'wiki',
      })
      const id = res.document_id
      if (!id) {
        ui.toast('Create succeeded but no document_id returned', 'error')
        return null
      }
      pendingEditId.value = id
      // Clear filter/facet so the new page is visible in the catalog list.
      if (filter.value.trim()) filter.value = ''
      if (facet.value.type !== 'all') facet.value = { type: 'all' }
      await loadCatalog()
      touchRecent({ id, title: t, slug })
      ui.toast('Page created', 'ok')
      return id
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      ui.toast(msg, 'error')
      throw e
    } finally {
      creating.value = false
    }
  }

  function isCasConflict(e: unknown): boolean {
    const msg = e instanceof Error ? e.message : String(e)
    // api client throws `HTTP ${status}: …`; gateway may also say "conflict"
    return /\bHTTP 409\b/.test(msg) || /\bconflict\b/i.test(msg)
  }

  /** After a 409: toast, re-fetch remote, offer keep-draft re-save vs reload. */
  async function handleCasConflict(id: string, slug: string) {
    const ui = useUiStore()
    ui.toast(
      'CAS conflict (409): page was updated elsewhere. Your draft is still in the editor.',
      'error',
    )

    let remote: DocumentBody
    try {
      remote = await api.document({ id })
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      ui.toast(`Could not reload after conflict: ${msg}`, 'error')
      throw e
    }

    const keepDraft = window.confirm(
      [
        'Save conflict: this page was changed by another writer (CAS 409).',
        '',
        `Remote: r${remote.revision ?? '?'} - "${remote.title}"`,
        `Your draft: "${draftTitle.value}"`,
        '',
        'OK - keep your draft and save again using the remote revision',
        'Cancel - discard draft and reload the remote page',
      ].join('\n'),
    )

    if (!keepDraft) {
      pendingEditId.value = null
      current.value = remote
      draftTitle.value = remote.title
      draftContent.value = remote.content
      dirty.value = false
      editing.value = false
      try {
        const bl = await api.backlinks(id)
        backlinks.value = bl.backlinks ?? []
      } catch {
        backlinks.value = []
      }
      ui.toast('Reloaded remote page; draft discarded', 'info')
      return
    }

    // Adopt remote CAS tokens; keep local draft title/content for overwrite re-save.
    current.value = remote
    editing.value = true
    dirty.value = true

    try {
      const res = await api.putWiki({
        slug: remote.uri.replace(/^wiki:\/\//, '') || slug || remote.id,
        id: remote.id,
        uri: remote.uri,
        title: draftTitle.value,
        content: draftContent.value,
        if_match_revision: remote.revision ?? undefined,
        if_match_etag: remote.etag ?? undefined,
      })
      pendingEditId.value = null
      await openPage(res.document_id || remote.id, false)
      await loadCatalog()
      editing.value = false
      dirty.value = false
      ui.toast('Saved over remote revision', 'ok')
    } catch (e) {
      if (isCasConflict(e)) {
        ui.toast(
          'Still conflicting after retry. Reload remote or merge manually, then save again.',
          'error',
        )
      } else {
        const msg = e instanceof Error ? e.message : String(e)
        ui.toast(msg, 'error')
      }
      throw e
    }
  }

  async function save() {
    const ui = useUiStore()
    const cur = current.value
    if (!cur) return
    const slug = cur.uri.replace(/^wiki:\/\//, '') || cur.id
    try {
      const res = await api.putWiki({
        slug,
        id: cur.id,
        uri: cur.uri,
        title: draftTitle.value,
        content: draftContent.value,
        if_match_revision: cur.revision ?? undefined,
        if_match_etag: cur.etag ?? undefined,
      })
      pendingEditId.value = null
      await openPage(res.document_id || cur.id, false)
      await loadCatalog()
      editing.value = false
      dirty.value = false
      ui.toast('Saved', 'ok')
    } catch (e) {
      if (isCasConflict(e)) {
        await handleCasConflict(cur.id, slug)
        return
      }
      const msg = e instanceof Error ? e.message : String(e)
      ui.toast(msg, 'error')
      throw e
    }
  }

  function markDirty() {
    dirty.value = true
  }

  function saveScrollPosition(pageId: string, top: number) {
    if (!pageId) return
    const y = Math.max(0, Math.round(top))
    const map = readScrollMap()
    if (map[pageId] === y) return
    map[pageId] = y
    writeScrollMap(map)
  }

  function getScrollPosition(pageId: string): number {
    if (!pageId) return 0
    const y = readScrollMap()[pageId]
    return typeof y === 'number' && Number.isFinite(y) ? Math.max(0, y) : 0
  }

  function clearScrollPosition(pageId: string) {
    if (!pageId) return
    const map = readScrollMap()
    if (!(pageId in map)) return
    delete map[pageId]
    writeScrollMap(map)
  }

  return {
    pages,
    filter,
    facet,
    categories,
    loading,
    error,
    current,
    backlinks,
    editing,
    draftTitle,
    draftContent,
    dirty,
    history,
    recent,
    favorites,
    catalogQ,
    pendingEditId,
    creating,
    filtered,
    favoritePages,
    setFacet,
    facetIsAll,
    facetIsKind,
    facetIsCategory,
    isFavorite,
    toggleFavorite,
    loadCatalog,
    openPage,
    goBack,
    startEdit,
    cancelEdit,
    createPage,
    save,
    markDirty,
    touchRecent,
    saveScrollPosition,
    getScrollPosition,
    clearScrollPosition,
  }
})
