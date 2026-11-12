import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SharedResource } from './resources.svelte'

vi.mock('./client', () => ({ api: { status: vi.fn(), agents: vi.fn() } }))

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (cause: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('SharedResource', () => {
  it('shares one request between concurrent callers', async () => {
    const pending = deferred<string[]>()
    const load = vi.fn(() => pending.promise)
    const resource = new SharedResource(load)
    const first = resource.refresh()
    const second = resource.refresh()
    pending.resolve(['claude'])
    await expect(first).resolves.toEqual(['claude'])
    await expect(second).resolves.toEqual(['claude'])
    expect(load).toHaveBeenCalledTimes(1)
    expect(resource.value).toEqual(['claude'])
  })

  it('fetches again once the previous request has settled', async () => {
    const load = vi.fn().mockResolvedValueOnce(1).mockResolvedValueOnce(2)
    const resource = new SharedResource<number>(load)
    await resource.refresh()
    await expect(resource.refresh()).resolves.toBe(2)
    expect(load).toHaveBeenCalledTimes(2)
  })

  it('keeps the last good snapshot and reports the error when a refresh fails', async () => {
    const load = vi.fn().mockResolvedValueOnce('good').mockRejectedValueOnce(new Error('down'))
    const resource = new SharedResource<string>(load)
    await resource.refresh()
    await expect(resource.refresh()).rejects.toThrow('down')
    expect(resource.value).toBe('good')
    expect(resource.error).toBe('down')
    load.mockResolvedValueOnce('back')
    await resource.refresh()
    expect(resource.error).toBeNull()
    expect(resource.value).toBe('back')
  })

  it('fresh() reuses a recent snapshot and refetches a stale or missing one', async () => {
    const load = vi.fn().mockResolvedValueOnce('a').mockResolvedValueOnce('b')
    const resource = new SharedResource<string>(load)
    await expect(resource.fresh(10_000)).resolves.toBe('a')
    vi.advanceTimersByTime(9_000)
    await expect(resource.fresh(10_000)).resolves.toBe('a')
    expect(load).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(2_000)
    await expect(resource.fresh(10_000)).resolves.toBe('b')
    expect(load).toHaveBeenCalledTimes(2)
  })
})
