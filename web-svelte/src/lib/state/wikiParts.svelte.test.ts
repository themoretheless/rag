// @vitest-environment jsdom
import { flushSync } from 'svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { WikiListResponse, WikiPageMeta } from '@/api/types'
import { WikiCatalog } from './wikiCatalog.svelte'
import { WikiPrefs } from './wikiPrefs.svelte'

vi.mock('@/api/client', () => ({ api: { wikiList: vi.fn() } }))

function meta(id: string, extra: Partial<WikiPageMeta> = {}): WikiPageMeta {
  return {
    id,
    uri: `wiki://${id}`,
    slug: id,
    title: `Title ${id}`,
    kind: 'wiki',
    revision: 1,
    etag: 'W/"1"',
    updated_at: '',
    ...extra,
  }
}

function list(pages: WikiPageMeta[], total = pages.length): WikiListResponse {
  return { ok: true, count: pages.length, pages, total, limit: 50, offset: 0 }
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
  sessionStorage.clear()
  vi.mocked(api.wikiList).mockResolvedValue(list([]))
})

describe('WikiCatalog filter reload', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('does nothing until start() is called', () => {
    const catalog = new WikiCatalog()
    catalog.filter = 'retriever'
    flushSync()
    vi.advanceTimersByTime(1000)
    expect(api.wikiList).not.toHaveBeenCalled()
  })

  it('reloads from the server once, 250 ms after typing stops', () => {
    const catalog = new WikiCatalog()
    const stop = catalog.start()
    flushSync()
    catalog.filter = 're'
    flushSync()
    vi.advanceTimersByTime(200)
    catalog.filter = 'ret'
    flushSync()
    vi.advanceTimersByTime(249)
    expect(api.wikiList).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(api.wikiList).toHaveBeenCalledExactlyOnceWith({ limit: 50, offset: 0, q: 'ret' })
    stop()
  })

  it('keeps one-character filters on the client', () => {
    const catalog = new WikiCatalog()
    const stop = catalog.start()
    flushSync()
    catalog.filter = 'r'
    flushSync()
    vi.advanceTimersByTime(1000)
    expect(api.wikiList).not.toHaveBeenCalled()
    stop()
  })

  it('stop() cancels a pending reload and further reactions', () => {
    const catalog = new WikiCatalog()
    const stop = catalog.start()
    flushSync()
    catalog.filter = 'ret'
    flushSync()
    stop()
    catalog.filter = 'retr'
    flushSync()
    vi.advanceTimersByTime(1000)
    expect(api.wikiList).not.toHaveBeenCalled()
  })
})

describe('WikiCatalog', () => {
  it('notifies its owner after each loaded page and builds category chips', async () => {
    const onPages = vi.fn()
    const catalog = new WikiCatalog(onPages)
    vi.mocked(api.wikiList).mockResolvedValueOnce(
      list([meta('a', { category: 'ops' }), meta('b', { category: ' arch ' }), meta('c')]),
    )
    await catalog.load()
    expect(onPages).toHaveBeenCalledTimes(1)
    expect(catalog.categories).toEqual(['arch', 'ops'])
    expect(catalog.uniqueSlug('a')).toBe('a-2')
    expect(catalog.uniqueSlug('new')).toBe('new')
  })

  it('filters client-side by facet and text', async () => {
    const catalog = new WikiCatalog()
    vi.mocked(api.wikiList).mockResolvedValue(
      list([
        meta('a', { category: 'ops', title: 'Runbook' }),
        meta('b', { category: 'arch', title: 'Design' }),
      ]),
    )
    await catalog.load()
    catalog.setFacet({ type: 'category', value: 'OPS' })
    expect(catalog.filtered.map((page) => page.id)).toEqual(['a'])
    expect(catalog.facetIsCategory('ops')).toBe(true)
    catalog.clearScope()
    catalog.filter = 'des'
    expect(catalog.filtered.map((page) => page.id)).toEqual(['b'])
  })
})

describe('WikiPrefs', () => {
  it('pins with catalog meta, persists, and unpins', () => {
    const prefs = new WikiPrefs()
    prefs.toggleFavorite('a', meta('a', { category: 'ops', revision: 3 }))
    expect(prefs.isFavorite('a')).toBe(true)
    expect(prefs.favorites).toEqual([{ id: 'a', title: 'Title a', slug: 'a', category: 'ops', revision: 3 }])
    expect(new WikiPrefs().favorites).toEqual(prefs.favorites)
    prefs.toggleFavorite('a')
    expect(prefs.isFavorite('a')).toBe(false)
    expect(new WikiPrefs().favorites).toEqual([])
  })

  it('pins an uncatalogued page from the open document or falls back to the id', () => {
    const prefs = new WikiPrefs()
    prefs.toggleFavorite('d1', undefined, {
      id: 'd1',
      uri: 'wiki://deep-page',
      title: 'Deep',
      layer: 'wiki',
      kind: 'wiki',
      content: '',
      revision: 7,
    })
    prefs.toggleFavorite('lonely')
    expect(prefs.favorites).toEqual([
      { id: 'd1', title: 'Deep', slug: 'deep-page', category: null, revision: 7 },
      { id: 'lonely', title: 'lonely', slug: 'lonely', category: null },
    ])
  })

  it('reads legacy string-id favorites', () => {
    localStorage.setItem(
      'rag-wiki-favorites',
      JSON.stringify(['x', { id: 'y', title: 'Y' }, 42, { nope: true }]),
    )
    expect(new WikiPrefs().favorites).toEqual([
      { id: 'x', title: 'x', slug: 'x' },
      { id: 'y', title: 'Y', slug: 'y', category: null },
    ])
  })

  it('keeps the recent list most-recent-first, unique and capped at eight', () => {
    const prefs = new WikiPrefs()
    for (let index = 0; index < 10; index++) {
      prefs.touchRecent({ id: `p${index}`, title: `P${index}`, slug: `p${index}` })
    }
    prefs.touchRecent({ id: 'p5', title: 'P5', slug: 'p5' })
    expect(prefs.recent.map((page) => page.id)).toEqual(['p5', 'p9', 'p8', 'p7', 'p6', 'p4', 'p3', 'p2'])
    expect(new WikiPrefs().recent).toHaveLength(8)
  })

  it('refreshes stored titles from the catalog and only rewrites storage on a change', () => {
    const prefs = new WikiPrefs()
    prefs.touchRecent({ id: 'a', title: 'Old', slug: 'a' })
    prefs.toggleFavorite('a', meta('a', { title: 'Old' }))
    const before = prefs.favorites
    prefs.syncFromCatalog([meta('a', { title: 'Old' })])
    expect(prefs.favorites).toBe(before)
    prefs.syncFromCatalog([meta('a', { title: 'Renamed', revision: 2 })])
    expect(prefs.recent[0]?.title).toBe('Renamed')
    expect(prefs.favorites[0]).toMatchObject({ title: 'Renamed', revision: 2 })
    expect(prefs.favoritePagesFor([meta('a', { title: 'Live' })])[0]?.title).toBe('Live')
  })

  it('stores scroll positions per page for the session', () => {
    const prefs = new WikiPrefs()
    prefs.saveScrollPosition('a', 120.6)
    prefs.saveScrollPosition('b', -5)
    expect(prefs.getScrollPosition('a')).toBe(121)
    expect(prefs.getScrollPosition('b')).toBe(0)
    expect(new WikiPrefs().getScrollPosition('b')).toBe(0)
    expect(prefs.getScrollPosition('')).toBe(0)
  })
})
