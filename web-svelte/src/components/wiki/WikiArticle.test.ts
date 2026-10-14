// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/svelte'
import { flushSync } from 'svelte'
import { goWiki } from '@/lib/router.svelte'
import { ui } from '@/lib/state/ui.svelte'
import { wiki } from '@/lib/state/wiki.svelte'
import { doc, meta } from '@/test/fixtures'
import WikiArticle from './WikiArticle.svelte'

vi.mock('@/api/client', () => ({ api: {} }))
vi.mock('@/lib/router.svelte', () => ({ goWiki: vi.fn(), goGraph: vi.fn(), route: { name: 'wiki', query: {} } }))
vi.mock('./WikiHome.svelte', async () => ({ default: (await import('../../test/Stub.svelte')).default }))

beforeEach(() => {
  vi.clearAllMocks()
  ui.locale = 'en'
  wiki.editing = false
  wiki.pages = [meta('b', 'Beta Page', { slug: 'beta' })]
  wiki.current = doc('a', '# Heading\n\nSee [[Beta Page]] and [[Missing]].', { title: 'Alpha', uri: 'wiki://alpha' })
})

describe('WikiArticle', () => {
  it('renders WikiHome when no page is open', () => {
    wiki.current = null
    render(WikiArticle)
    expect(screen.getByTestId('stub')).toBeInTheDocument()
  })

  it('renders title, meta and markdown body', () => {
    render(WikiArticle)
    expect(screen.getByRole('heading', { level: 1, name: 'Alpha' })).toBeInTheDocument()
    expect(screen.getByText('wiki://alpha')).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Heading' })).toBeInTheDocument()
    expect(screen.getByRole('navigation', { name: ui.t('outline') })).toBeInTheDocument()
  })

  it('marks missing wikilinks and routes known ones', async () => {
    const { container } = render(WikiArticle)
    const links = container.querySelectorAll<HTMLAnchorElement>('a[data-wikilink]')
    expect(links).toHaveLength(2)
    expect(links[1]).toHaveClass('missing')
    await fireEvent.click(links[0]!)
    expect(goWiki).toHaveBeenCalledWith('b')
    await fireEvent.click(links[1]!)
    expect(goWiki).toHaveBeenCalledTimes(1)
  })

  it('activates wikilinks from the keyboard', async () => {
    const { container } = render(WikiArticle)
    const link = container.querySelector<HTMLAnchorElement>('a[data-wikilink]')!
    await fireEvent.keyDown(link, { key: 'Enter' })
    expect(goWiki).toHaveBeenCalledOnce()
    expect(goWiki).toHaveBeenCalledWith('b')
  })

  it('activates wikilinks with the Space key exactly once', async () => {
    const { container } = render(WikiArticle)
    const link = container.querySelector<HTMLAnchorElement>('a[data-wikilink]')!
    await fireEvent.keyDown(link, { key: ' ' })
    expect(goWiki).toHaveBeenCalledOnce()
    expect(goWiki).toHaveBeenCalledWith('b')
  })

  it('ignores other keys on wikilinks', async () => {
    const { container } = render(WikiArticle)
    const link = container.querySelector<HTMLAnchorElement>('a[data-wikilink]')!
    await fireEvent.keyDown(link, { key: 'a' })
    expect(goWiki).not.toHaveBeenCalled()
  })

  it('re-renders when the open page changes', () => {
    render(WikiArticle)
    wiki.current = doc('c', 'Plain body', { title: 'Gamma' })
    flushSync()
    expect(screen.getByRole('heading', { level: 1, name: 'Gamma' })).toBeInTheDocument()
    expect(screen.getByText('Plain body')).toBeInTheDocument()
  })
})
