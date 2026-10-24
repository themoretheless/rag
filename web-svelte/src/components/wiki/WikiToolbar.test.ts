// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/svelte'
import { goGraph, goWiki } from '@/lib/router.svelte'
import { ui } from '@/lib/state/ui.svelte'
import { wiki } from '@/lib/state/wiki.svelte'
import { doc, meta } from '@/test/fixtures'
import WikiToolbar from './WikiToolbar.svelte'

vi.mock('@/api/client', () => ({ api: {} }))
vi.mock('@/lib/router.svelte', () => ({ goWiki: vi.fn(), goGraph: vi.fn() }))

beforeEach(() => {
  vi.clearAllMocks()
  vi.restoreAllMocks()
  localStorage.clear()
  ui.locale = 'en'
  wiki.cancelEdit()
  wiki.saving = false
  wiki.favorites = []
  wiki.history = []
  wiki.pages = [meta('a', 'Alpha', { category: 'Guides' }), meta('b', 'Beta')]
  wiki.current = doc('a', 'body', { title: 'Alpha' })
})

describe('WikiToolbar', () => {
  it('renders breadcrumbs with category, title and revision', () => {
    render(WikiToolbar)
    const crumbs = screen.getByRole('navigation', { name: ui.t('breadcrumb') })
    expect(crumbs).toHaveTextContent(new RegExp(`${ui.t('wiki')}\\s*/\\s*Guides\\s*/\\s*Alpha`))
    expect(screen.getByText('r3')).toBeInTheDocument()
  })

  it('wiki crumb returns to root', async () => {
    render(WikiToolbar)
    await fireEvent.click(screen.getByRole('button', { name: ui.t('wiki') }))
    expect(goWiki).toHaveBeenCalledWith()
  })

  it('disables back without history', () => {
    render(WikiToolbar)
    expect(screen.getByRole('button', { name: ui.t('back') })).toBeDisabled()
  })

  it('goes back and lists history newest first', async () => {
    const goBack = vi.spyOn(wiki, 'goBack').mockImplementation(() => {
      wiki.history.pop()
    })
    wiki.history = ['b', 'a']
    render(WikiToolbar)
    await fireEvent.click(screen.getByTitle(ui.t('history')))
    const items = screen.getAllByRole('menuitem')
    expect(items.map((i) => i.textContent?.replace(/\s+/g, ' ').trim())).toEqual(['Alpha Guides', 'Beta'])
    await fireEvent.click(items[1]!)
    expect(goBack).toHaveBeenCalledOnce()
    expect(goWiki).toHaveBeenCalledWith('b', { replace: true })
    expect(screen.queryByRole('menu')).toBeNull()
  })

  it('toggles favorite and opens graph', async () => {
    render(WikiToolbar)
    const pin = screen.getByRole('button', { name: ui.t('pinFavorite') })
    expect(pin).toHaveAttribute('aria-pressed', 'false')
    await fireEvent.click(pin)
    expect(wiki.isFavorite('a')).toBe(true)
    expect(pin).toHaveTextContent('★')
    expect(pin).toHaveAttribute('aria-label', ui.t('unpinFavorite'))
    expect(pin).toHaveAttribute('title', ui.t('unpinFavorite'))
    expect(screen.getByRole('button', { name: ui.t('unpinFavorite') })).toBe(pin)
    await fireEvent.click(pin)
    expect(pin).toHaveAttribute('aria-label', ui.t('pinFavorite'))
    await fireEvent.click(pin)
    await fireEvent.click(screen.getByText(ui.t('showInGraph')))
    expect(goGraph).toHaveBeenCalledWith('a')
  })

  it('switches between edit and view controls', async () => {
    render(WikiToolbar)
    await fireEvent.click(screen.getByRole('button', { name: new RegExp(ui.t('edit')) }))
    expect(wiki.editing).toBe(true)
    const save = screen.getByRole('button', { name: ui.t('save') })
    expect(save).toBeDisabled()
    await fireEvent.click(screen.getByRole('button', { name: new RegExp(ui.t('cancel')) }))
    expect(wiki.editing).toBe(false)
  })

  it('toasts when save fails', async () => {
    vi.spyOn(wiki, 'save').mockRejectedValue(new Error('x'))
    const toast = vi.spyOn(ui, 'toast').mockImplementation(() => {})
    render(WikiToolbar)
    wiki.startEdit()
    wiki.dirty = true
    await vi.waitFor(() => expect(screen.getByRole('button', { name: ui.t('save') })).toBeEnabled())
    await fireEvent.click(screen.getByRole('button', { name: ui.t('save') }))
    await vi.waitFor(() => expect(toast).toHaveBeenCalledWith(ui.t('saveFailed'), 'error'))
  })
})
