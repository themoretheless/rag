// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/svelte'
import { flushSync } from 'svelte'
import { api } from '@/api/client'
import { go, goGraph, goWiki, navigate } from '@/lib/router.svelte'
import { ui } from '@/lib/state/ui.svelte'
import { wiki } from '@/lib/state/wiki.svelte'
import { graph } from '@/lib/state/graph.svelte'
import { meta } from '@/test/fixtures'
import CommandPalette from './CommandPalette.svelte'

vi.mock('@/api/client', () => ({ api: { get: vi.fn(), wikiList: vi.fn() } }))
vi.mock('@/lib/router.svelte', () => ({
  route: { name: 'wiki', pageId: null, query: {} },
  goWiki: vi.fn(),
  goGraph: vi.fn(),
  goSearch: vi.fn(),
  go: vi.fn(),
  navigate: vi.fn(),
}))

beforeEach(() => {
  vi.clearAllMocks()
  vi.mocked(api.get).mockResolvedValue({ items: [] })
  ui.locale = 'en'
  ui.commandOpen = false
  wiki.pages = [meta('p1', 'Architecture'), meta('p2', 'Runbook')]
  graph.nodes = []
})

function open() {
  render(CommandPalette)
  ui.openCommand()
  flushSync()
}

describe('CommandPalette', () => {
  it('renders nothing while closed', () => {
    render(CommandPalette)
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('lists actions and catalog pages when opened', () => {
    open()
    expect(screen.getByRole('dialog')).toBeInTheDocument()
    expect(screen.getByText(ui.t('paletteActionGraph'))).toBeInTheDocument()
    expect(screen.getByText('Architecture')).toBeInTheDocument()
    expect(screen.getByText('Runbook')).toBeInTheDocument()
  })

  it('filters by query and shows empty state', async () => {
    open()
    const input = screen.getByRole('searchbox')
    await fireEvent.input(input, { target: { value: 'runb' } })
    expect(screen.queryByText('Architecture')).toBeNull()
    expect(screen.getByText('Runbook')).toBeInTheDocument()
    await fireEvent.input(input, { target: { value: 'zzzzqq' } })
    expect(screen.getByText(ui.t('paletteNoMatches'))).toBeInTheDocument()
  })

  it('opens a page on click and closes', async () => {
    open()
    await fireEvent.click(screen.getByText('Runbook'))
    expect(goWiki).toHaveBeenCalledWith('p2')
    expect(ui.commandOpen).toBe(false)
  })

  it('navigates with arrows and Enter, closes on Escape', async () => {
    open()
    const dialog = screen.getByRole('dialog')
    await fireEvent.keyDown(dialog, { key: 'ArrowDown' })
    await fireEvent.keyDown(dialog, { key: 'Enter' })
    expect(goGraph).toHaveBeenCalled()
    expect(ui.commandOpen).toBe(false)

    ui.openCommand()
    flushSync()
    await fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' })
    expect(ui.commandOpen).toBe(false)
  })

  it('includes matching graph nodes and raw documents', async () => {
    vi.useFakeTimers()
    try {
      graph.nodes = [{ id: 'n1', label: 'Retriever node', kind: 'doc', document_id: 'd1' } as never]
      vi.mocked(api.get).mockResolvedValue({ items: [{ id: 'd9', title: 'Retriever spec', uri: 'file:///x', layer: 'raw', wing: 'rag' }] })
      open()
      await fireEvent.input(screen.getByRole('searchbox'), { target: { value: 'retriever' } })
      await vi.advanceTimersByTimeAsync(200)
      flushSync()
      expect(api.get).toHaveBeenCalledWith(expect.stringContaining('/v1/documents?'))
      await fireEvent.click(screen.getByText('Retriever spec'))
      expect(navigate).toHaveBeenCalledWith(expect.stringContaining('document_id=d9'))
      ui.openCommand()
      flushSync()
      await fireEvent.input(screen.getByRole('searchbox'), { target: { value: 'retriever' } })
      await fireEvent.click(screen.getByText('Retriever node'))
      expect(goGraph).toHaveBeenCalledWith('d1', { project: graph.project })
    } finally {
      vi.useRealTimers()
    }
  })

  it('runs route actions', async () => {
    open()
    await fireEvent.input(screen.getByRole('searchbox'), { target: { value: 'корпус' } })
    await fireEvent.click(screen.getByText('Корпус'))
    expect(go).toHaveBeenCalledWith('corpus')
  })
})
