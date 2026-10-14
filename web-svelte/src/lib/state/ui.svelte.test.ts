// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { HealthResponse } from '@/api/types'

vi.mock('@/api/client', () => ({ api: { get: vi.fn(), health: vi.fn() } }))

const mocked = vi.mocked(api)

async function freshUi() {
  vi.resetModules()
  return (await import('./ui.svelte')).ui
}

beforeEach(() => {
  vi.clearAllMocks()
  localStorage.clear()
  delete document.documentElement.dataset.theme
  window.matchMedia = vi.fn().mockReturnValue({ matches: false }) as unknown as typeof window.matchMedia
})

afterEach(() => {
  vi.useRealTimers()
})

describe('ui store', () => {
  it('hydrates theme and locale from localStorage', async () => {
    localStorage.setItem('rag-theme', 'light')
    localStorage.setItem('rag-locale', 'en')
    const ui = await freshUi()
    ui.hydrateTheme()
    expect(ui.theme).toBe('light')
    expect(document.documentElement.dataset.theme).toBe('light')
    expect(ui.locale).toBe('en')
    expect(ui.localeLabel).toBe('EN')
  })

  it('ignores unknown persisted locales', async () => {
    localStorage.setItem('rag-locale', 'de')
    const ui = await freshUi()
    ui.hydrateLocale()
    expect(ui.locale).toBe('ru')
  })

  it('setTheme persists and resolves system preference', async () => {
    const ui = await freshUi()
    ui.setTheme('system')
    expect(localStorage.getItem('rag-theme')).toBe('system')
    expect(document.documentElement.dataset.theme).toBe('light')
  })

  it('toggleLocale flips and persists the locale and translations', async () => {
    const ui = await freshUi()
    const ru = ui.t('wiki')
    ui.toggleLocale()
    expect(ui.locale).toBe('en')
    expect(localStorage.getItem('rag-locale')).toBe('en')
    expect(typeof ui.t('wiki')).toBe('string')
    ui.toggleLocale()
    expect(ui.t('wiki')).toBe(ru)
  })

  it('toggles sidebar and command palette', async () => {
    const ui = await freshUi()
    ui.toggleSidebar()
    expect(ui.sidebarCollapsed).toBe(true)
    ui.openCommand()
    expect(ui.commandOpen).toBe(true)
    ui.closeCommand()
    expect(ui.commandOpen).toBe(false)
  })

  it('toasts expire after their timeout', async () => {
    vi.useFakeTimers()
    const ui = await freshUi()
    ui.toast('hello', 'ok')
    ui.toast('second')
    expect(ui.toasts.map((t) => t.text)).toEqual(['hello', 'second'])
    expect(ui.toasts[1]!.kind).toBe('info')
    vi.advanceTimersByTime(4200)
    expect(ui.toasts).toEqual([])
  })

  it('checkHealth keeps health and agents failures independent', async () => {
    const ui = await freshUi()
    mocked.health.mockResolvedValue({ ok: true } as unknown as HealthResponse)
    mocked.get.mockResolvedValue({ items: [{ agent: 'claude', online: true }] })
    await ui.checkHealth()
    expect(ui.health).toEqual({ ok: true })
    expect(ui.shellAgents).toHaveLength(1)

    mocked.health.mockRejectedValue(new Error('down'))
    mocked.get.mockRejectedValue(new Error('agents down'))
    await ui.checkHealth()
    expect(ui.health).toBeNull()
    expect(ui.healthError).toBe('down')
    expect(ui.shellAgentsError).toBe('agents down')
    expect(ui.shellAgents).toHaveLength(1)
  })

  it('checkHealth drops stale responses', async () => {
    const ui = await freshUi()
    let resolveOld!: (v: HealthResponse) => void
    mocked.health.mockReturnValueOnce(new Promise((r) => (resolveOld = r)))
    mocked.health.mockResolvedValueOnce({ ok: 'new' } as unknown as HealthResponse)
    mocked.get.mockResolvedValue({ items: [] })
    const old = ui.checkHealth()
    await ui.checkHealth()
    resolveOld({ ok: 'old' } as unknown as HealthResponse)
    await old
    expect(ui.health).toEqual({ ok: 'new' })
  })
})
