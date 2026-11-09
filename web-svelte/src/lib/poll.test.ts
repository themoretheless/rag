// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { startPoll } from './poll'

let visibility: DocumentVisibilityState = 'visible'

function setVisibility(next: DocumentVisibilityState) {
  visibility = next
  document.dispatchEvent(new Event('visibilitychange'))
}

beforeEach(() => {
  vi.useFakeTimers()
  visibility = 'visible'
  vi.spyOn(document, 'visibilityState', 'get').mockImplementation(() => visibility)
})

afterEach(() => {
  vi.restoreAllMocks()
  vi.useRealTimers()
})

describe('startPoll', () => {
  it('ticks on the interval and never immediately', () => {
    const fn = vi.fn()
    const stop = startPoll(fn, 1000)
    expect(fn).not.toHaveBeenCalled()
    vi.advanceTimersByTime(3000)
    expect(fn).toHaveBeenCalledTimes(3)
    stop()
  })

  it('pauses while the tab is hidden and catches up once when it is shown again', () => {
    const fn = vi.fn()
    const stop = startPoll(fn, 1000)
    setVisibility('hidden')
    vi.advanceTimersByTime(5000)
    expect(fn).not.toHaveBeenCalled()
    setVisibility('visible')
    expect(fn).toHaveBeenCalledTimes(1)
    vi.advanceTimersByTime(1000)
    expect(fn).toHaveBeenCalledTimes(2)
    stop()
  })

  it('does not fire on a visibility flip when no tick was missed', () => {
    const fn = vi.fn()
    const stop = startPoll(fn, 1000)
    setVisibility('hidden')
    setVisibility('visible')
    expect(fn).not.toHaveBeenCalled()
    stop()
  })

  it('stops ticking and listening after stop()', () => {
    const fn = vi.fn()
    const stop = startPoll(fn, 1000)
    setVisibility('hidden')
    vi.advanceTimersByTime(1000)
    stop()
    setVisibility('visible')
    vi.advanceTimersByTime(5000)
    expect(fn).not.toHaveBeenCalled()
  })
})
