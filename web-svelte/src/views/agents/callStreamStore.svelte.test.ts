import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { CallRow } from '@/api/types'
import { CALL_TIMEOUT_MS, CallStream } from './callStreamStore.svelte'

vi.mock('@/api/client', () => ({ api: { calls: vi.fn() } }))

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

const row = (seq: number): CallRow => ({
  seq,
  ts: '2026-01-01T00:00:00Z',
  agent: 'claude',
  tool: 'search',
  args: '{}',
  elapsed_ms: 5,
  ok: true,
})

beforeEach(() => {
  vi.clearAllMocks()
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('CallStream', () => {
  it('goes loading → live and records when the snapshot was taken', async () => {
    const stream = new CallStream()
    expect(stream.state).toBe('loading')
    mocked.calls.mockResolvedValue([row(1), row(2)])
    await stream.fetch()
    expect(mocked.calls).toHaveBeenCalledWith(100, expect.anything())
    expect(stream.state).toBe('live')
    expect(stream.calls).toHaveLength(2)
    expect(stream.updatedAt).toBe(Date.now())
    expect(stream.error).toBe('')
  })

  it('reports an empty but healthy stream as empty', async () => {
    const stream = new CallStream()
    mocked.calls.mockResolvedValue([])
    await stream.fetch()
    expect(stream.state).toBe('empty')
  })

  it('is error before the first snapshot and stale (keeping the snapshot) after it', async () => {
    const stream = new CallStream()
    mocked.calls.mockRejectedValueOnce(new Error('down'))
    await expect(stream.fetch()).rejects.toThrow('down')
    expect(stream.state).toBe('error')
    expect(stream.error).toBe('down')

    mocked.calls.mockResolvedValueOnce([row(1)])
    await stream.fetch()
    expect(stream.state).toBe('live')

    mocked.calls.mockRejectedValueOnce(new Error('flaky'))
    await expect(stream.fetch()).rejects.toThrow('flaky')
    expect(stream.state).toBe('stale')
    expect(stream.calls).toHaveLength(1)
  })

  it('times out a hanging request', async () => {
    const stream = new CallStream()
    mocked.calls.mockReturnValue(new Promise(() => undefined))
    const pending = expect(stream.fetch()).rejects.toThrow(`Timeout after ${CALL_TIMEOUT_MS} ms`)
    await vi.advanceTimersByTimeAsync(CALL_TIMEOUT_MS)
    await pending
    expect(stream.state).toBe('error')
  })

  it('lets a newer refresh win over a slower older one', async () => {
    const stream = new CallStream()
    const slow = deferred<CallRow[]>()
    mocked.calls.mockReturnValueOnce(slow.promise).mockResolvedValueOnce([row(2)])
    const first = stream.fetch()
    await stream.fetch()
    slow.resolve([row(1)])
    await first
    expect(stream.calls.map((item) => item.seq)).toEqual([2])
    expect(mocked.calls.mock.calls[0]?.[1]?.signal?.aborted).toBe(true)
  })

  it('poll() never rejects and skips a tick while the previous one is running', async () => {
    const stream = new CallStream()
    const slow = deferred<CallRow[]>()
    mocked.calls.mockReturnValueOnce(slow.promise)
    const first = stream.poll()
    await stream.poll()
    expect(mocked.calls).toHaveBeenCalledTimes(1)
    slow.reject(new Error('down'))
    await expect(first).resolves.toBeUndefined()
    expect(stream.state).toBe('error')

    mocked.calls.mockResolvedValueOnce([row(3)])
    await stream.poll()
    expect(stream.state).toBe('live')
  })
})
