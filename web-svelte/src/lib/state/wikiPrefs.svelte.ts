import type { DocumentBody, WikiPageMeta } from '@/api/types'

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

const RECENT_KEY = 'rag-wiki-recent'
const RECENT_MAX = 8
const FAVORITES_KEY = 'rag-wiki-favorites'
/** sessionStorage key: map of wiki page id → article scrollTop */
const SCROLL_STORAGE_KEY = 'rag-wiki-scroll'

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
          category: typeof p.category === 'string' ? p.category : null,
          ...(typeof p.revision === 'number' ? { revision: p.revision } : {}),
        })
      }
    }
    return out
  } catch {
    return []
  }
}

function persist(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    /* quota / private mode */
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

/**
 * Per-browser wiki preferences: pinned favorites and the recent list (localStorage)
 * and per-page article scroll positions (sessionStorage). Never sent to the gateway.
 */
export class WikiPrefs {
  recent = $state<RecentPage[]>(loadRecent())
  favorites = $state<FavoritePage[]>(loadFavorites())

  isFavorite(id: string): boolean {
    return this.favorites.some((f) => f.id === id)
  }

  /**
   * Pin or unpin a page. `meta` (catalog row) and `current` (open document) supply
   * the title and slug for a new pin when they are known.
   */
  toggleFavorite(id: string, meta?: WikiPageMeta, current?: DocumentBody | null) {
    if (!id) return
    if (this.favorites.some((f) => f.id === id)) {
      this.favorites = this.favorites.filter((f) => f.id !== id)
      persist(FAVORITES_KEY, this.favorites)
      return
    }
    const cur = current?.id === id ? current : null
    const slug = meta?.slug || (cur ? cur.uri.replace(/^wiki:\/\//, '') : '') || id
    const revision = meta?.revision ?? cur?.revision
    const entry: FavoritePage = {
      id,
      title: meta?.title || cur?.title || slug || id,
      slug,
      category: meta?.category ?? null,
      ...(revision != null ? { revision } : {}),
    }
    this.favorites = [...this.favorites, entry]
    persist(FAVORITES_KEY, this.favorites)
  }

  /** Favorites for display; catalog meta is preferred when the page is loaded. */
  favoritePagesFor(pages: readonly WikiPageMeta[]): FavoritePage[] {
    const byId = new Map(pages.map((p) => [p.id, p]))
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
  }

  /** Push or promote a page to the head of the recent list (max 8). */
  touchRecent(page: RecentPage) {
    const entry: RecentPage = {
      id: page.id,
      title: page.title || page.slug || page.id,
      slug: page.slug || page.id,
    }
    this.recent = [entry, ...this.recent.filter((r) => r.id !== entry.id)].slice(0, RECENT_MAX)
    persist(RECENT_KEY, this.recent)
  }

  /** Refresh stored titles / slugs of favorites and recents from freshly loaded catalog rows. */
  syncFromCatalog(pages: readonly WikiPageMeta[]) {
    if (!pages.length) return
    const byId = new Map(pages.map((p) => [p.id, p]))
    this.syncRecent(byId)
    this.syncFavorites(byId)
  }

  private syncFavorites(byId: Map<string, WikiPageMeta>) {
    if (!this.favorites.length) return
    // Unchanged entries keep their identity, so reference inequality marks a real change.
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
      return {
        id: f.id,
        title: meta.title || f.title,
        slug: meta.slug || f.slug,
        category: meta.category ?? null,
        revision: meta.revision,
      }
    })
    if (next.some((entry, index) => entry !== this.favorites[index])) {
      this.favorites = next
      persist(FAVORITES_KEY, this.favorites)
    }
  }

  private syncRecent(byId: Map<string, WikiPageMeta>) {
    if (!this.recent.length) return
    // Unchanged entries keep their identity, so reference inequality marks a real change.
    const next = this.recent.map((r) => {
      const meta = byId.get(r.id)
      if (!meta) return r
      if (meta.title === r.title && meta.slug === r.slug) return r
      return { id: r.id, title: meta.title || r.title, slug: meta.slug || r.slug }
    })
    if (next.some((entry, index) => entry !== this.recent[index])) {
      this.recent = next
      persist(RECENT_KEY, this.recent)
    }
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
    const { [pageId]: _dropped, ...rest } = map
    writeScrollMap(rest)
  }
}
