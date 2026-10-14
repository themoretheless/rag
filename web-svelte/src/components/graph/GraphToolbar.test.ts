// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/svelte'
import { ui } from '@/lib/state/ui.svelte'
import { graph } from '@/lib/state/graph.svelte'
import GraphToolbar from './GraphToolbar.svelte'

vi.mock('@/api/client', () => ({ api: { get: vi.fn(), graph: vi.fn(), neighbors: vi.fn() } }))

function props() {
  return {
    onexpand: vi.fn(),
    onfull: vi.fn(),
    onfit: vi.fn(),
    onzoomin: vi.fn(),
    onzoomout: vi.fn(),
    onprojectchange: vi.fn(),
    onreload: vi.fn(async () => {}),
  }
}

beforeEach(() => {
  ui.locale = 'en'
  graph.nodes = [{ id: 'a', label: 'A' } as never, { id: 'b', label: 'B' } as never]
  graph.edges = []
  graph.projects = [{ project_id: 'rag', document_count: 12 }]
  graph.project = 'rag'
  graph.includeTags = true
  graph.focusMode = false
  graph.loading = false
  graph.error = null
  graph.setLayout(null)
})

describe('GraphToolbar', () => {
  it('shows counts and project options', () => {
    render(GraphToolbar, props())
    expect(screen.getByText(ui.t('graphNodes', { count: 2 }))).toBeInTheDocument()
    expect(screen.getByText(ui.t('graphEdges', { count: 0 }))).toBeInTheDocument()
    expect(screen.getByRole('option', { name: 'rag · 12' })).toBeInTheDocument()
  })

  it('wires callback buttons', async () => {
    const p = props()
    render(GraphToolbar, p)
    await fireEvent.click(screen.getByText(ui.t('graphFull')))
    await fireEvent.click(screen.getByText(ui.t('graphExpand')))
    await fireEvent.click(screen.getByRole('button', { name: ui.t('graphZoomIn') }))
    await fireEvent.click(screen.getByRole('button', { name: ui.t('graphZoomOut') }))
    await fireEvent.click(screen.getByRole('button', { name: ui.t('graphFit') }))
    expect(p.onfull).toHaveBeenCalledOnce()
    expect(p.onexpand).toHaveBeenCalledOnce()
    expect(p.onzoomin).toHaveBeenCalledOnce()
    expect(p.onzoomout).toHaveBeenCalledOnce()
    expect(p.onfit).toHaveBeenCalledOnce()
  })

  it('changes project and notifies', async () => {
    const p = props()
    render(GraphToolbar, p)
    await fireEvent.change(screen.getByRole('combobox', { name: ui.t('graphProject') }), { target: { value: '' } })
    expect(graph.project).toBe('')
    expect(p.onprojectchange).toHaveBeenCalledOnce()
  })

  it('toggles layout override and focus mode', async () => {
    render(GraphToolbar, props())
    const radial = screen.getByText(ui.t('layoutRadial'))
    await fireEvent.click(radial)
    expect(graph.layout).toBe('radial')
    expect(radial).toHaveAttribute('aria-pressed', 'true')
    await fireEvent.click(radial)
    expect(graph.layoutOverride).toBeNull()
    await fireEvent.click(screen.getByRole('button', { name: ui.t('graphFocus') }))
    expect(graph.focusMode).toBe(true)
  })

  it('toggles tags and reverts when reload fails', async () => {
    const p = props()
    render(GraphToolbar, p)
    const tags = screen.getByText(ui.t('graphTagsToggle'))
    await fireEvent.click(tags)
    expect(p.onreload).toHaveBeenCalledOnce()
    expect(graph.includeTags).toBe(false)

    let duringReload: boolean | undefined
    p.onreload.mockImplementationOnce(async () => {
      duringReload = graph.includeTags
      graph.error = 'nope'
    })
    await fireEvent.click(tags)
    await vi.waitFor(() => expect(p.onreload).toHaveBeenCalledTimes(2))
    expect(duringReload).toBe(true)
    expect(graph.includeTags).toBe(false)
  })

  it('disables tags toggle while loading', () => {
    graph.loading = true
    render(GraphToolbar, props())
    expect(screen.getByText(ui.t('graphTagsToggle'))).toBeDisabled()
  })
})
