import { beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { DocumentDetail, DocumentPage, DocumentRow } from '@/api/types'
import { ui } from '@/lib/state/ui.svelte'
import { contentSize, documentFilters, mergeRows, prependRow } from './corpusModel'
import { createCorpusStore } from './corpusStore.svelte'

vi.mock('@/api/client', async () => {
  const { documentsQuery } =
    await vi.importActual<typeof import('@/api/endpoints/corpus')>('@/api/endpoints/corpus')
  const { loadPanels } = await vi.importActual<typeof import('@/lib/async')>('@/lib/async')
  return {
    documentsQuery,
    loadPanels,
    api: {
      documents: vi.fn(),
      multiGet: vi.fn(),
      backlinks: vi.fn(),
      projects: vi.fn(),
      taxonomy: vi.fn(),
      embeddingManifest: vi.fn(),
      projectHome: vi.fn(),
      startSync: vi.fn(),
    },
  }
})
vi.mock('@/lib/state/ui.svelte', () => ({ ui: { toast: vi.fn() } }))

const mocked = vi.mocked(api)

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

function row(id: string, title = `Doc ${id}`): DocumentRow {
  return { id, uri: `file:///${id}`, title, layer: 'raw', kind: 'document', status: 'active', updated_at: '' }
}

function page(ids: string[], next: string | null = null, total = ids.length): DocumentPage {
  return { items: ids.map((id) => row(id)), page: { total, next_cursor: next } }
}

function detail(id: string): DocumentDetail {
  return { document: { ...row(id), revision: 1, metadata_json: '{}', content: `body ${id}` }, chunks: [] }
}

/** Let the fire-and-forget detail load started by a catalog reload settle. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))

beforeEach(() => {
  vi.clearAllMocks()
  mocked.multiGet.mockImplementation((ids) => Promise.resolve(ids.map(detail)))
  mocked.backlinks.mockResolvedValue({ ok: true, count: 0, backlinks: [] })
  mocked.projects.mockResolvedValue([])
  mocked.taxonomy.mockResolvedValue(undefined)
  mocked.embeddingManifest.mockResolvedValue({})
})

describe('corpus model', () => {
  it('builds the documents query from the filters', () => {
    const filters = { project: 'rag', room: 'src', layer: 'raw', status: 'archived', query: '  spec ' }
    expect(documentFilters(filters)).toEqual({
      limit: 200,
      wing: 'rag',
      room: 'src',
      layer: 'raw',
      status: 'archived',
      include_archived: true,
      q: 'spec',
    })
    expect(documentFilters({ ...filters, status: 'draft' }).include_archived).toBe(false)
  })

  it('merges pages without duplicates, keeping position and the newer row', () => {
    const merged = mergeRows([row('a'), row('b')], [row('b', 'B updated'), row('c')])
    expect(merged.map((item) => item.id)).toEqual(['a', 'b', 'c'])
    expect(merged[1]?.title).toBe('B updated')
  })

  it('prepends a deep-linked row once', () => {
    expect(prependRow([row('a'), row('b')], row('b', 'B')).map((item) => item.id)).toEqual(['b', 'a'])
    expect(prependRow([row('a')], row('z')).map((item) => item.id)).toEqual(['z', 'a'])
  })

  it('measures content in UTF-8 bytes', () => {
    expect(contentSize('')).toBe('—')
    expect(contentSize('привет')).toMatch(/^12 байт$/)
  })
})

describe('corpus store catalog', () => {
  it('loads the first page, remembers the cursor and opens the first row', async () => {
    const store = createCorpusStore()
    mocked.documents.mockResolvedValueOnce(page(['a', 'b'], 'c1', 5))
    await store.loadDocuments()
    await settle()
    expect(mocked.documents).toHaveBeenCalledWith(expect.objectContaining({ limit: 200 }), expect.anything())
    expect(store.rows.map((item) => item.id)).toEqual(['a', 'b'])
    expect(store.total).toBe(5)
    expect(store.nextCursor).toBe('c1')
    expect(store.busy).toBe(false)
    expect(store.selected?.id).toBe('a')
    expect(store.detailDocument?.content).toBe('body a')
  })

  it('appends cursor pages, de-duplicates overlapping rows and stops without a cursor', async () => {
    const store = createCorpusStore()
    mocked.documents
      .mockResolvedValueOnce(page(['a', 'b'], 'c1', 3))
      .mockResolvedValueOnce(page(['b', 'c'], null, 3))
    await store.loadDocuments()
    await store.loadMore()
    expect(mocked.documents).toHaveBeenLastCalledWith(
      expect.objectContaining({ cursor: 'c1' }),
      expect.anything(),
    )
    expect(store.rows.map((item) => item.id)).toEqual(['a', 'b', 'c'])
    expect(store.nextCursor).toBeNull()
    expect(store.loadingMore).toBe(false)

    await store.loadMore()
    expect(mocked.documents).toHaveBeenCalledTimes(2)
  })

  it('refuses to append once the filters differ from the loaded query', async () => {
    const store = createCorpusStore()
    mocked.documents.mockResolvedValueOnce(page(['a'], 'c1', 9))
    await store.loadDocuments()
    store.query = 'changed'
    expect(store.filtersDirty).toBe(true)
    await store.loadMore()
    expect(mocked.documents).toHaveBeenCalledTimes(1)
  })

  it('drops an appended page when the filters changed while it was loading', async () => {
    const store = createCorpusStore()
    const slow = deferred<DocumentPage>()
    mocked.documents.mockResolvedValueOnce(page(['a'], 'c1', 9)).mockReturnValueOnce(slow.promise)
    await store.loadDocuments()
    const more = store.loadMore()
    store.layer = 'wiki'
    slow.resolve(page(['b'], 'c2', 9))
    await more
    expect(store.rows.map((item) => item.id)).toEqual(['a'])
    expect(store.nextCursor).toBe('c1')
  })

  it('keeps the page error separate from the first-page error', async () => {
    const store = createCorpusStore()
    mocked.documents.mockRejectedValueOnce(new Error('HTTP 500: first'))
    await store.loadDocuments()
    expect(store.documentError).toBe('HTTP 500: first')
    expect(store.appendError).toBe('')

    mocked.documents.mockResolvedValueOnce(page(['a'], 'c1', 9)).mockRejectedValueOnce(new Error('Offline'))
    await store.loadDocuments()
    expect(store.documentError).toBe('')
    await store.loadMore()
    expect(store.appendError).toBe('Offline')
    expect(store.rows).toHaveLength(1)
  })

  it('ignores a stale catalog response when a newer load has started, and aborts it', async () => {
    const store = createCorpusStore()
    const slow = deferred<DocumentPage>()
    mocked.documents.mockReturnValueOnce(slow.promise).mockResolvedValueOnce(page(['fresh']))
    const first = store.loadDocuments()
    store.query = 'fresh'
    await store.loadDocuments()
    slow.resolve(page(['stale']))
    await first
    expect(mocked.documents.mock.calls[0]?.[1]?.signal?.aborted).toBe(true)
    expect(store.rows.map((item) => item.id)).toEqual(['fresh'])
    expect(store.busy).toBe(false)
  })
})

describe('corpus store selection', () => {
  it('selects a deep-linked row from the page, or fetches it when it is not listed', async () => {
    const listed = createCorpusStore()
    listed.applyRoute({ project: 'rag', room: '', query: 'doc', documentId: 'b' })
    mocked.documents.mockResolvedValueOnce(page(['a', 'b']))
    await listed.loadDocuments()
    await settle()
    expect(mocked.documents).toHaveBeenCalledWith(
      expect.objectContaining({ wing: 'rag', q: 'doc' }),
      expect.anything(),
    )
    expect(listed.selected?.id).toBe('b')

    const unlisted = createCorpusStore()
    unlisted.applyRoute({ project: '', room: '', query: '', documentId: 'z' })
    mocked.documents.mockResolvedValueOnce(page(['a']))
    await unlisted.loadDocuments()
    await settle()
    expect(mocked.multiGet).toHaveBeenCalledWith(['z'], { include_chunks: false })
    expect(unlisted.rows.map((item) => item.id)).toEqual(['z', 'a'])
    expect(unlisted.selected?.id).toBe('z')
  })

  it('warns when the deep-linked document does not exist', async () => {
    const store = createCorpusStore()
    store.applyRoute({ project: '', room: '', query: '', documentId: 'gone' })
    mocked.documents.mockResolvedValueOnce(page(['a']))
    mocked.multiGet.mockResolvedValueOnce([])
    await store.loadDocuments()
    await settle()
    expect(store.panelWarning).toBe('Документ gone не найден')
  })

  it('shows only the detail of the latest chosen row', async () => {
    const store = createCorpusStore()
    const slow = deferred<DocumentDetail[]>()
    mocked.multiGet.mockReturnValueOnce(slow.promise)
    const first = store.choose(row('a'))
    await store.choose(row('b'))
    slow.resolve([detail('a')])
    await first
    expect(store.selected?.id).toBe('b')
    expect(store.detailDocument?.id).toBe('b')
    expect(store.detailBusy).toBe(false)
  })

  it('reports a failed detail load and tolerates failing backlinks', async () => {
    const store = createCorpusStore()
    mocked.backlinks.mockRejectedValueOnce(new Error('no backlinks'))
    await store.choose(row('a'))
    expect(store.detailDocument?.id).toBe('a')
    expect(store.backlinks).toEqual([])

    mocked.multiGet.mockRejectedValueOnce(new Error('HTTP 500: detail'))
    await store.choose(row('b'))
    expect(store.detailError).toBe('HTTP 500: detail')
    expect(ui.toast).toHaveBeenCalledWith('HTTP 500: detail', 'error')
  })

  it('closing the inspector discards a detail that is still loading', async () => {
    const store = createCorpusStore()
    const slow = deferred<DocumentDetail[]>()
    mocked.multiGet.mockReturnValueOnce(slow.promise)
    const choosing = store.choose(row('a'))
    store.closeInspector()
    slow.resolve([detail('a')])
    await choosing
    expect(store.selected).toBeNull()
    expect(store.detail).toBeNull()
    expect(store.detailBusy).toBe(false)
  })
})

describe('corpus store panels', () => {
  it('refreshAll reports failed panels without hiding the others', async () => {
    const store = createCorpusStore()
    mocked.documents.mockResolvedValue(page(['a']))
    mocked.projects.mockResolvedValueOnce([{ project_id: 'rag', document_count: 3 }])
    mocked.taxonomy.mockRejectedValueOnce(new Error('HTTP 500: taxonomy'))
    await store.refreshAll()
    expect(store.projects).toHaveLength(1)
    expect(store.rows).toHaveLength(1)
    expect(store.allDocumentCount).toBe(3)
    expect(store.panelWarning).toBe('Не обновлены панели: taxonomy: HTTP 500: taxonomy')
  })

  it('chooseScope reloads the catalog and loads the project home for that scope only', async () => {
    const store = createCorpusStore()
    const home = deferred<{ source_roots: { canonical_root: string; file_count: number }[] } | null>()
    mocked.documents.mockResolvedValue(page([]))
    mocked.projectHome.mockReturnValueOnce(home.promise).mockResolvedValueOnce(null)
    store.chooseScope('rag', 'src')
    store.chooseScope('other')
    home.resolve({ source_roots: [{ canonical_root: '/rag', file_count: 1 }] })
    await settle()
    expect(store.project).toBe('other')
    expect(store.projectHome).toBeNull()
    expect(mocked.documents).toHaveBeenLastCalledWith(
      expect.objectContaining({ wing: 'other' }),
      expect.anything(),
    )
  })

  it('starts a sync job for the current scope', async () => {
    const store = createCorpusStore()
    store.project = 'rag'
    mocked.startSync.mockResolvedValueOnce('job-1')
    await expect(store.startSync('/data')).resolves.toBe('job-1')
    expect(mocked.startSync).toHaveBeenCalledWith({ path: '/data', wing: 'rag', room: undefined })
  })
})
