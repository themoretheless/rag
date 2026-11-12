import { beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { PackContextResponse, SearchHit, SearchResponse } from '@/api/types'
import { createSearchSession } from './searchSession.svelte'

vi.mock('@/api/client', () => ({ api: { search: vi.fn(), packContext: vi.fn(), createWiki: vi.fn() } }))

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

function hit(id: string): SearchHit {
  return {
    chunk_id: id,
    document_id: `doc-${id}`,
    document_title: id,
    document_uri: `file:///${id}`,
    chunk_index: 0,
    content: id,
    score: 1,
  }
}

function results(...ids: string[]): SearchResponse {
  return { ok: true, mode: 'hybrid', count: ids.length, items: ids.map(hit) }
}

function packedFrom(hits: SearchHit[]): PackContextResponse {
  return { ok: true, total_tokens: 10, max_tokens: 2000, omitted_count: 0, context_text: 'ctx', hits }
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe('search session', () => {
  it('does nothing for a blank query', async () => {
    const session = createSearchSession()
    session.form.query = '   '
    await session.run()
    expect(mocked.search).not.toHaveBeenCalled()
    expect(session.busy).toBe(false)
  })

  it('runs the request built from the form and remembers its snapshot', async () => {
    const session = createSearchSession()
    session.form.query = ' rrf '
    session.form.wing = 'rag'
    mocked.search.mockResolvedValue(results('a', 'b'))
    await session.run()
    expect(mocked.search).toHaveBeenCalledWith(
      expect.objectContaining({ query: 'rrf', wing: 'rag', mode: 'hybrid', top_k: 8 }),
      expect.objectContaining({ signal: expect.any(AbortSignal) as AbortSignal }),
    )
    expect(session.hits.map((item) => item.chunk_id)).toEqual(['a', 'b'])
    expect(session.resultSnapshot?.query).toBe('rrf')
    expect(session.executedRequest).toMatchObject({ query: 'rrf', wing: 'rag' })
    expect(session.resultsDirty).toBe(false)
    expect(session.busy).toBe(false)
  })

  it('marks results dirty once the form moves on, and clean again when it returns', async () => {
    const session = createSearchSession()
    session.form.query = 'q'
    mocked.search.mockResolvedValue(results('a'))
    await session.run()
    session.form.topK = 20
    expect(session.resultsDirty).toBe(true)
    session.form.topK = 8
    expect(session.resultsDirty).toBe(false)
  })

  it('drops a stale response when a newer search has started, and aborts it', async () => {
    const session = createSearchSession()
    const slow = deferred<SearchResponse>()
    mocked.search.mockReturnValueOnce(slow.promise).mockResolvedValueOnce(results('fresh'))
    session.form.query = 'old'
    const first = session.run()
    const firstSignal = mocked.search.mock.calls[0]?.[1]?.signal
    session.form.query = 'new'
    await session.run()
    slow.resolve(results('stale'))
    await first
    expect(firstSignal?.aborted).toBe(true)
    expect(session.hits.map((item) => item.chunk_id)).toEqual(['fresh'])
    expect(session.resultSnapshot?.query).toBe('new')
    expect(session.busy).toBe(false)
  })

  it('ignores the failure of a superseded search but reports the current one', async () => {
    const session = createSearchSession()
    const slow = deferred<SearchResponse>()
    mocked.search.mockReturnValueOnce(slow.promise).mockResolvedValueOnce(results('fresh'))
    session.form.query = 'old'
    const first = session.run()
    session.form.query = 'new'
    await session.run()
    slow.reject(new DOMException('aborted', 'AbortError'))
    await first
    expect(session.error).toBe('')

    mocked.search.mockRejectedValueOnce(new Error('HTTP 500: boom'))
    await session.run()
    expect(session.error).toBe('HTTP 500: boom')
    expect(session.hits).toEqual([])
    expect(session.busy).toBe(false)
  })

  it('packs the shown hits and refuses to pack while the form is ahead of the results', async () => {
    const session = createSearchSession()
    session.form.query = 'q'
    session.form.maxTokens = 500
    mocked.search.mockResolvedValue(results('a', 'b'))
    await session.run()

    session.form.topK = 3
    await session.pack()
    expect(mocked.packContext).not.toHaveBeenCalled()

    session.form.topK = 8
    mocked.packContext.mockResolvedValue(packedFrom([hit('a')]))
    await session.pack()
    expect(mocked.packContext).toHaveBeenCalledWith(
      { hits: session.hits, max_tokens: 500, context_expansion: 'neighbors', neighbor_chunks: 1 },
      expect.anything(),
    )
    expect(session.packed?.context_text).toBe('ctx')
    expect(session.packedSourceCount).toBe(2)
    expect(session.packedDirty).toBe(false)
    session.form.query = 'other'
    expect(session.packedDirty).toBe(true)
  })

  it('a new search invalidates an in-flight pack', async () => {
    const session = createSearchSession()
    session.form.query = 'q'
    mocked.search.mockResolvedValue(results('a'))
    await session.run()
    const slowPack = deferred<PackContextResponse>()
    mocked.packContext.mockReturnValueOnce(slowPack.promise)
    const packing = session.pack()
    expect(session.packing).toBe(true)
    await session.run()
    expect(session.packing).toBe(false)
    slowPack.resolve(packedFrom([hit('a')]))
    await packing
    expect(session.packed).toBeNull()
  })

  it('creates a wiki draft only from a current packed context', async () => {
    const session = createSearchSession()
    await expect(session.createWikiDraft()).resolves.toBeUndefined()

    session.form.query = 'q'
    mocked.search.mockResolvedValue(results('a'))
    mocked.packContext.mockResolvedValue(packedFrom([hit('a')]))
    await session.run()
    await session.pack()

    mocked.createWiki.mockResolvedValueOnce({
      ok: true,
      document_id: 'w1',
      uri: 'wiki://x',
      slug: 'x',
      revision: 1,
      etag: 'W/"1"',
    })
    await expect(session.createWikiDraft()).resolves.toBe('w1')
    expect(mocked.createWiki).toHaveBeenCalledWith(
      expect.objectContaining({ category: 'search-context', kind: 'wiki', title: 'Контекст поиска: q' }),
    )

    mocked.createWiki.mockRejectedValueOnce(new Error('HTTP 409: URI occupied'))
    await expect(session.createWikiDraft()).resolves.toBeNull()
    expect(session.error).toBe('HTTP 409: URI occupied')
    expect(session.creatingWiki).toBe(false)

    session.form.query = 'changed'
    await expect(session.createWikiDraft()).resolves.toBeUndefined()
    expect(mocked.createWiki).toHaveBeenCalledTimes(2)
  })
})
