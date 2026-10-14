// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/svelte'
import { flushSync } from 'svelte'
import { ui } from '@/lib/state/ui.svelte'
import { wiki } from '@/lib/state/wiki.svelte'
import { doc } from '@/test/fixtures'
import Outline from './Outline.svelte'

vi.mock('@/api/client', () => ({ api: {} }))

const content = '# Intro\n\ntext\n\n## Setup\n\nmore\n\n### Details\n'

beforeEach(() => {
  ui.locale = 'en'
  wiki.editing = false
  wiki.current = doc('a', content)
})

describe('Outline', () => {
  it('hides when there are no headings', () => {
    wiki.current = doc('a', 'plain text')
    render(Outline)
    expect(screen.queryByRole('navigation')).toBeNull()
  })

  it('hides while editing', () => {
    wiki.editing = true
    render(Outline)
    expect(screen.queryByRole('navigation')).toBeNull()
  })

  it('lists headings indented by level', () => {
    render(Outline)
    const nav = screen.getByRole('navigation', { name: ui.t('outline') })
    expect(nav).toBeInTheDocument()
    const items = nav.querySelectorAll('li')
    expect([...items].map((li) => li.textContent?.trim())).toEqual(['Intro', 'Setup', 'Details'])
    expect((items[2] as HTMLElement).style.paddingLeft).toBe('34px')
  })

  it('collapses and expands', async () => {
    render(Outline)
    const head = screen.getByRole('button', { name: new RegExp(ui.t('outline')) })
    expect(head).toHaveAttribute('aria-expanded', 'true')
    await fireEvent.click(head)
    expect(head).toHaveAttribute('aria-expanded', 'false')
    expect(screen.queryByText('Setup')).toBeNull()
  })

  it('scrolls to a heading and marks it active', async () => {
    const target = document.createElement('h2')
    target.id = 'setup'
    target.scrollIntoView = vi.fn()
    document.body.append(target)
    render(Outline)
    await fireEvent.click(screen.getByText('Setup'))
    expect(target.scrollIntoView).toHaveBeenCalled()
    expect(screen.getByText('Setup').closest('li')).toHaveClass('active')
    target.remove()
  })

  it('updates when page content changes', () => {
    render(Outline)
    wiki.current = doc('b', '## Only')
    flushSync()
    expect(screen.getByText('Only')).toBeInTheDocument()
    expect(screen.queryByText('Intro')).toBeNull()
  })
})
