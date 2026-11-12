// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'

vi.mock('@/api/client', () => ({ api: { agents: vi.fn(), health: vi.fn() } }))

const mocked = vi.mocked(api)

/** Fresh module graph per test: the agents resource caches its last snapshot. */
async function freshHealth() {
  vi.resetModules()
  const { ShellHealth, HEALTH_POLL_MS } = await import('./health.svelte')
  return { health: new ShellHealth(), HEALTH_POLL_MS }
}

beforeEach(() => {
  vi.clearAllMocks()
  vi.useFakeTimers()
  mocked.health.mockResolvedValue({ ok: true })
  mocked.agents.mockResolvedValue([{ agent: 'claude', online: true }])
})

afterEach(() => {
  vi.useRealTimers()
})

describe('ShellHealth polling', () => {
  it('checks immediately, then on every interval, until stopped', async () => {
    const { health, HEALTH_POLL_MS } = await freshHealth()
    const stop = health.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(mocked.health).toHaveBeenCalledTimes(1)
    expect(health.health).toEqual({ ok: true })
    expect(health.agents).toHaveLength(1)

    await vi.advanceTimersByTimeAsync(HEALTH_POLL_MS)
    expect(mocked.health).toHaveBeenCalledTimes(2)

    stop()
    await vi.advanceTimersByTimeAsync(HEALTH_POLL_MS * 3)
    expect(mocked.health).toHaveBeenCalledTimes(2)
  })

  it('background ticks reuse a recent agents snapshot, explicit checks do not', async () => {
    const { health, HEALTH_POLL_MS } = await freshHealth()
    const stop = health.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(mocked.agents).toHaveBeenCalledTimes(1)

    // A screen refreshes agents shortly before the next shell tick.
    await vi.advanceTimersByTimeAsync(HEALTH_POLL_MS - 2_000)
    await health.check()
    expect(mocked.agents).toHaveBeenCalledTimes(2)

    await vi.advanceTimersByTimeAsync(2_000)
    expect(mocked.health).toHaveBeenCalledTimes(3)
    expect(mocked.agents).toHaveBeenCalledTimes(2)
    stop()
  })

  it('does not poll while the tab is hidden', async () => {
    const { health, HEALTH_POLL_MS } = await freshHealth()
    const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden')
    const stop = health.start()
    await vi.advanceTimersByTimeAsync(HEALTH_POLL_MS * 2)
    expect(mocked.health).toHaveBeenCalledTimes(1)
    stop()
    visibility.mockRestore()
  })
})
