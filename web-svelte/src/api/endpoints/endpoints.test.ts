import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api, documentsQuery } from '../client'

function respondWith(body: unknown) {
  vi.mocked(fetch).mockResolvedValueOnce(
    new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } }),
  )
}

function lastCall() {
  const call = vi.mocked(fetch).mock.calls.at(-1)
  if (!call) throw new Error('fetch was not called')
  const [url, init] = call
  return { url: url instanceof Request ? url.url : url.toString(), init }
}

beforeEach(() => {
  vi.stubGlobal('sessionStorage', { getItem: () => null, setItem: vi.fn(), removeItem: vi.fn() })
  vi.stubGlobal('fetch', vi.fn())
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('documentsQuery', () => {
  it('serializes only the active filters in a stable order', () => {
    expect(documentsQuery({ limit: 200 })).toBe('limit=200')
    expect(
      documentsQuery({
        q: 'spec',
        status: 'archived',
        include_archived: true,
        layer: 'raw',
        room: 'src',
        wing: 'rag',
        limit: 200,
      }),
    ).toBe('limit=200&wing=rag&room=src&layer=raw&status=archived&include_archived=true&q=spec')
    expect(
      documentsQuery({
        limit: 200,
        wing: '',
        room: '',
        layer: '',
        status: '',
        include_archived: false,
        q: '',
      }),
    ).toBe('limit=200')
  })

  it('appends the cursor last so a page keeps the signature of its filters as a prefix', () => {
    const filters = { limit: 200, wing: 'rag' }
    expect(documentsQuery({ ...filters, cursor: 'c1' })).toBe(`${documentsQuery(filters)}&cursor=c1`)
  })
})

describe('typed endpoints', () => {
  it('unwraps list envelopes and tolerates a missing items field', async () => {
    respondWith({ items: [{ agent: 'claude', online: true }] })
    await expect(api.agents()).resolves.toEqual([{ agent: 'claude', online: true }])
    expect(lastCall().url).toBe('/v1/agents')

    respondWith({})
    await expect(api.opsLog(8)).resolves.toEqual([])
    expect(lastCall().url).toBe('/v1/ops-log?limit=8')
  })

  it('unwraps single-object envelopes with their documented fallbacks', async () => {
    respondWith({ runtime: { pid: 7 } })
    await expect(api.runtime()).resolves.toEqual({ pid: 7 })
    respondWith({})
    await expect(api.runtime()).resolves.toEqual({})
    respondWith({})
    await expect(api.taxonomy()).resolves.toBeUndefined()
    respondWith({})
    await expect(api.projectHome('my wing')).resolves.toBeNull()
    expect(lastCall().url).toBe('/v1/project-home?project=my%20wing')
  })

  it('normalizes both KG envelopes', async () => {
    respondWith({ items: [{ id: 'f1' }], count: 12 })
    await expect(api.kgFacts(80)).resolves.toEqual({ items: [{ id: 'f1' }], total: 12 })
    respondWith({ facts: [{ id: 'f1' }, { id: 'f2' }] })
    await expect(api.kgFacts(80)).resolves.toEqual({ items: [{ id: 'f1' }, { id: 'f2' }], total: 2 })
    expect(lastCall().url).toBe('/v1/kg?limit=80')
  })

  it('posts multi-get and sync bodies and resolves the job id from either shape', async () => {
    respondWith({ items: [] })
    await api.multiGet(['d1'], { include_chunks: true, chunk_limit: 80 })
    expect(lastCall().url).toBe('/v1/multi-get')
    expect(lastCall().init?.body).toBe('{"document_ids":["d1"],"include_chunks":true,"chunk_limit":80}')

    respondWith({ job: { id: 'j2' } })
    await expect(api.startSync({ path: '/data', wing: undefined, room: undefined })).resolves.toBe('j2')
    expect(lastCall().init?.body).toBe('{"path":"/data"}')
    respondWith({})
    await expect(api.startSync({ path: '/data' })).resolves.toBe('')
  })

  it('forwards an abort signal to fetch', async () => {
    const controller = new AbortController()
    respondWith({ items: [] })
    await api.documents({ limit: 20, layer: 'raw', q: 'x' }, { signal: controller.signal })
    expect(lastCall().url).toBe('/v1/documents?limit=20&layer=raw&q=x')
    expect(lastCall().init?.signal).toBe(controller.signal)
  })
})
