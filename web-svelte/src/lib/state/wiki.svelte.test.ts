// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { DocumentBody, WikiListResponse, WikiPageMeta, WikiPutResult } from '@/api/types'
import { ui } from './ui.svelte'
import { WikiStore } from './wiki.svelte'

vi.mock('@/api/client', () => ({ api: {
  wikiList: vi.fn(), document: vi.fn(), backlinks: vi.fn(), putWiki: vi.fn(), createWiki: vi.fn(),
} }))
vi.mock('./ui.svelte', () => ({ ui: { locale: 'ru', toast: vi.fn(), t: (key: string) => key } }))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}

function page(i: number): WikiPageMeta {
  return { id: `${i}`, uri: `wiki://page-${i}`, slug: `page-${i}`, title: `Page ${i}`, kind: 'wiki', revision: 1, etag: 'W/"1"', updated_at: '' }
}

function batch(offset: number, count: number, total: number): WikiListResponse {
  return { ok: true, count, total, offset, limit: 50, pages: Array.from({ length: count }, (_, i) => page(offset + i)) }
}

function document(id = 'a', content = 'Original', revision = 1): DocumentBody {
  return { id, uri: `wiki://${id}`, title: `Title ${id}`, content, layer: 'wiki', kind: 'wiki', revision, etag: `W/"${revision}"` }
}

function saved(id = 'a', revision = 2): WikiPutResult {
  return { ok: true, document_id: id, uri: `wiki://${id}`, slug: id, revision, etag: `W/"${revision}"` }
}

function editingStore() {
  const store = new WikiStore()
  store.current = document()
  store.startEdit()
  store.draftContent = 'Submitted'
  store.markDirty()
  return store
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.wikiList).mockResolvedValue(batch(0, 0, 0))
  vi.mocked(api.backlinks).mockResolvedValue({ ok: true, count: 0, backlinks: [] })
  vi.mocked(api.document).mockImplementation(async ({ id }) => document(id))
  vi.mocked(api.putWiki).mockResolvedValue(saved())
  vi.stubGlobal('localStorage', { getItem: () => null, setItem: vi.fn() })
  vi.spyOn(window, 'confirm').mockReturnValue(true)
})

describe('wiki catalog pagination', () => {
  it('loads beyond 50, retains earlier pages, and stops at the reported total', async () => {
    const store = new WikiStore()
    vi.mocked(api.wikiList).mockResolvedValueOnce(batch(0, 50, 75)).mockResolvedValueOnce(batch(50, 25, 75))
    await store.loadCatalog()
    expect(store.pages).toHaveLength(50)
    expect(store.catalogHasMore).toBe(true)
    await store.loadMore()
    expect(store.pages).toHaveLength(75)
    expect(store.pages[74].id).toBe('74')
    expect(store.catalogTotal).toBe(75)
    expect(store.catalogHasMore).toBe(false)
    expect(api.wikiList).toHaveBeenLastCalledWith({ limit: 50, offset: 50 })
  })

  it('preserves loaded pages after a next-page failure and retries the same offset', async () => {
    const store = new WikiStore()
    vi.mocked(api.wikiList).mockResolvedValueOnce(batch(0, 50, 51)).mockRejectedValueOnce(new Error('Offline')).mockResolvedValueOnce(batch(50, 1, 51))
    await store.loadCatalog()
    await store.loadMore()
    expect(store.pages).toHaveLength(50)
    expect(store.loadMoreError).toBe('Offline')
    await store.loadMore()
    expect(store.pages).toHaveLength(51)
    expect(store.loadMoreError).toBeNull()
  })

  it('ignores an old next-page response after changing catalog filters', async () => {
    const pending = deferred<WikiListResponse>()
    const store = new WikiStore()
    vi.mocked(api.wikiList).mockResolvedValueOnce(batch(0, 50, 100)).mockReturnValueOnce(pending.promise).mockResolvedValueOnce(batch(200, 1, 1))
    await store.loadCatalog()
    const more = store.loadMore()
    store.filter = 'specific'
    await store.loadCatalog()
    pending.resolve(batch(50, 50, 100))
    await more
    expect(store.pages.map((row) => row.id)).toEqual(['200'])
    expect(store.catalogQ).toBe('specific')
    expect(store.loadingMore).toBe(false)
  })

  it('refuses repeated pages instead of presenting an incomplete catalog as complete', async () => {
    const store = new WikiStore()
    vi.mocked(api.wikiList).mockResolvedValueOnce(batch(0, 50, 60)).mockResolvedValueOnce({ ...batch(0, 10, 60), offset: 50 })
    await store.loadCatalog()
    await store.loadMore()
    expect(store.pages).toHaveLength(50)
    expect(store.loadMoreError).toContain('Каталог изменился')
    expect(store.catalogHasMore).toBe(true)
  })
})

describe('wiki save lifecycle', () => {
  it('keeps text typed during a save dirty and advances CAS for the next save', async () => {
    const pending = deferred<WikiPutResult>()
    vi.mocked(api.putWiki).mockReturnValueOnce(pending.promise).mockResolvedValueOnce(saved('a', 3))
    const store = editingStore()
    const save = store.save()
    store.draftContent = 'Submitted plus new text'
    store.draftTitle = 'New title while saving'
    store.markDirty()
    await store.save()
    expect(api.putWiki).toHaveBeenCalledTimes(1)
    pending.resolve(saved())
    await save
    expect(store.current?.content).toBe('Submitted')
    expect(store.current?.revision).toBe(2)
    expect(store.draftContent).toBe('Submitted plus new text')
    expect(store.draftTitle).toBe('New title while saving')
    expect(store.editing).toBe(true)
    expect(store.dirty).toBe(true)
    await store.save()
    expect(api.putWiki).toHaveBeenLastCalledWith(expect.objectContaining({ content: 'Submitted plus new text', title: 'New title while saving', if_match_revision: 2 }))
    expect(store.dirty).toBe(false)
    expect(store.editing).toBe(false)
  })

  it('does not let a saved response replace a different page and restores later edits on return', async () => {
    const pending = deferred<WikiPutResult>()
    vi.mocked(api.putWiki).mockReturnValueOnce(pending.promise)
    const store = editingStore()
    const save = store.save()
    store.draftContent = 'Unsaved addition on A'
    store.markDirty()
    await store.openPage('b')
    pending.resolve(saved())
    await save
    expect(store.current?.id).toBe('b')
    expect(store.draftContent).toBe('Original')
    vi.mocked(api.document).mockResolvedValueOnce(document('a', 'Submitted', 2))
    await store.openPage('a')
    expect(store.draftContent).toBe('Unsaved addition on A')
    expect(store.dirty).toBe(true)
    expect(store.current?.revision).toBe(2)
  })

  it('retains input added while navigation is loading the next page', async () => {
    const pending = deferred<DocumentBody>()
    const store = editingStore()
    vi.mocked(api.document).mockReturnValueOnce(pending.promise)
    const navigation = store.openPage('b')
    store.draftContent = 'Typed while B was loading'
    store.markDirty()
    pending.resolve(document('b'))
    await navigation
    await store.openPage('a')
    expect(store.draftContent).toBe('Typed while B was loading')
  })

  it('keeps the draft editable after a failed save and guards cancel while pending', async () => {
    const pending = deferred<WikiPutResult>()
    vi.mocked(api.putWiki).mockReturnValueOnce(pending.promise)
    const store = editingStore()
    const save = store.save()
    store.cancelEdit()
    expect(store.editing).toBe(true)
    pending.reject(new Error('Offline'))
    await expect(save).rejects.toThrow('Offline')
    expect(store.draftContent).toBe('Submitted')
    expect(store.dirty).toBe(true)
    expect(store.saving).toBe(false)
  })

  it('does not show a stale conflict dialog after navigating to another page', async () => {
    const remote = deferred<DocumentBody>()
    vi.mocked(api.putWiki).mockRejectedValueOnce(new Error('HTTP 409: conflict'))
    vi.mocked(api.document).mockReturnValueOnce(remote.promise)
    const store = editingStore()
    const save = store.save()
    await Promise.resolve()
    await Promise.resolve()
    await store.openPage('b')
    remote.resolve(document('a', 'Remote update', 2))
    await save
    expect(window.confirm).not.toHaveBeenCalled()
    expect(store.current?.id).toBe('b')
  })

  it('preserves typing during an explicitly approved conflict retry', async () => {
    const retry = deferred<WikiPutResult>()
    vi.mocked(api.putWiki).mockRejectedValueOnce(new Error('HTTP 409: conflict')).mockReturnValueOnce(retry.promise)
    vi.mocked(api.document).mockResolvedValueOnce(document('a', 'Remote update', 2))
    const store = editingStore()
    const save = store.save()
    await vi.waitFor(() => expect(api.putWiki).toHaveBeenCalledTimes(2))
    store.draftContent = 'More typing during retry'
    store.markDirty()
    retry.resolve(saved('a', 3))
    await save
    expect(store.current?.revision).toBe(3)
    expect(store.draftContent).toBe('More typing during retry')
    expect(store.dirty).toBe(true)
  })
})

describe('wiki create-only behavior', () => {
  it('never overwrites a hidden page when a slug collides outside the loaded catalog', async () => {
    const store = new WikiStore()
    store.pages = [page(50)]
    vi.mocked(api.createWiki).mockRejectedValueOnce(new Error('HTTP 409: URI occupied'))
    expect(await store.createPage('Hidden Page')).toBeNull()
    expect(api.createWiki).toHaveBeenCalledWith({ slug: 'hidden-page', title: 'Hidden Page', content: '# Hidden Page\n\n', kind: 'wiki' })
    expect(api.putWiki).not.toHaveBeenCalled()
    expect(store.pages.map((row) => row.id)).toEqual(['50'])
    expect(store.pendingEditId).toBeNull()
    expect(ui.toast).toHaveBeenCalledWith(expect.stringContaining('Выберите другое название'), 'error')
  })
})
