import { afterEach, describe, expect, it, vi } from 'vitest'
import { lazyPromise } from './async'
import { copyText } from './clipboard'

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('copyText', () => {
  it('writes to the clipboard and reports success', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    await expect(copyText('hello')).resolves.toBe(true)
    expect(writeText).toHaveBeenCalledWith('hello')
  })

  it('reports failure instead of rejecting when the write is denied', async () => {
    vi.stubGlobal('navigator', {
      clipboard: { writeText: vi.fn().mockRejectedValue(new DOMException('denied', 'NotAllowedError')) },
    })
    await expect(copyText('hello')).resolves.toBe(false)
  })

  it('reports failure when the clipboard API is missing (insecure origin)', async () => {
    vi.stubGlobal('navigator', {})
    await expect(copyText('hello')).resolves.toBe(false)
  })
})

describe('lazyPromise', () => {
  it('loads once and shares the result', async () => {
    const load = vi.fn().mockResolvedValue('module')
    const get = lazyPromise(load)
    const [first, second] = await Promise.all([get(), get()])
    expect(first).toBe('module')
    expect(second).toBe('module')
    await get()
    expect(load).toHaveBeenCalledTimes(1)
  })

  it('does not cache a rejection: the next call retries', async () => {
    const load = vi.fn().mockRejectedValueOnce(new Error('chunk failed')).mockResolvedValueOnce('module')
    const get = lazyPromise(load)
    await expect(get()).rejects.toThrow('chunk failed')
    await expect(get()).resolves.toBe('module')
    await expect(get()).resolves.toBe('module')
    expect(load).toHaveBeenCalledTimes(2)
  })

  it('shares one failure between concurrent callers', async () => {
    const load = vi.fn().mockRejectedValue(new Error('offline'))
    const get = lazyPromise(load)
    const results = await Promise.allSettled([get(), get()])
    expect(results.map((result) => result.status)).toEqual(['rejected', 'rejected'])
    expect(load).toHaveBeenCalledTimes(1)
  })
})
