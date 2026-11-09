import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { createRequestGate, debounce, errorMessage, isAbortError, withTimeout } from './async'

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('errorMessage', () => {
  it('unwraps Error instances and stringifies everything else', () => {
    expect(errorMessage(new Error('boom'))).toBe('boom')
    expect(errorMessage('plain')).toBe('plain')
    expect(errorMessage(42)).toBe('42')
    expect(errorMessage(null)).toBe('null')
  })

  it('recognises fetch aborts', () => {
    expect(isAbortError(new DOMException('gone', 'AbortError'))).toBe(true)
    expect(isAbortError(new Error('AbortError'))).toBe(false)
  })
})

describe('debounce', () => {
  it('runs only the last call after the delay', () => {
    const fn = vi.fn()
    const debounced = debounce(fn, 100)
    debounced('a')
    vi.advanceTimersByTime(99)
    debounced('b')
    expect(debounced.pending).toBe(true)
    vi.advanceTimersByTime(99)
    expect(fn).not.toHaveBeenCalled()
    vi.advanceTimersByTime(1)
    expect(fn).toHaveBeenCalledExactlyOnceWith('b')
    expect(debounced.pending).toBe(false)
  })

  it('cancel drops the pending call', () => {
    const fn = vi.fn()
    const debounced = debounce(fn, 50)
    debounced()
    debounced.cancel()
    vi.advanceTimersByTime(500)
    expect(fn).not.toHaveBeenCalled()
  })
})

describe('createRequestGate', () => {
  it('keeps only the newest ticket current and aborts the superseded one', () => {
    const gate = createRequestGate()
    const first = gate.next()
    expect(first.current).toBe(true)
    expect(gate.latest).toBe(first)
    const second = gate.next()
    expect(first.current).toBe(false)
    expect(first.signal.aborted).toBe(true)
    expect(second.current).toBe(true)
    expect(second.signal.aborted).toBe(false)
  })

  it('cancel invalidates the in-flight ticket without a replacement request', () => {
    const gate = createRequestGate()
    const ticket = gate.next()
    gate.cancel()
    expect(ticket.current).toBe(false)
    expect(ticket.signal.aborted).toBe(true)
  })

  it('exposes a usable ticket before the first request', () => {
    const gate = createRequestGate()
    const idle = gate.latest
    expect(idle.current).toBe(true)
    gate.next()
    expect(idle.current).toBe(false)
  })

  it('drops a slow stale response that resolves after a newer one', async () => {
    const gate = createRequestGate()
    const applied: string[] = []
    const load = async (value: string, delay: number) => {
      const ticket = gate.next()
      await new Promise((resolve) => setTimeout(resolve, delay))
      if (ticket.current) applied.push(value)
    }
    const slow = load('stale', 100)
    const fast = load('fresh', 10)
    await vi.advanceTimersByTimeAsync(100)
    await Promise.all([slow, fast])
    expect(applied).toEqual(['fresh'])
  })
})

describe('withTimeout', () => {
  it('passes through a result that arrives in time', async () => {
    await expect(withTimeout(Promise.resolve('ok'), 100)).resolves.toBe('ok')
    expect(vi.getTimerCount()).toBe(0)
  })

  it('rejects with the timeout once the budget is spent', async () => {
    const pending = withTimeout(new Promise<never>(() => undefined), 8000)
    const settled = expect(pending).rejects.toThrow('Timeout after 8000 ms')
    await vi.advanceTimersByTimeAsync(8000)
    await settled
  })

  it('propagates the original rejection', async () => {
    await expect(withTimeout(Promise.reject(new Error('down')), 100)).rejects.toThrow('down')
  })
})
