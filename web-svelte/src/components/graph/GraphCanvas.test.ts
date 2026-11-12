// @vitest-environment jsdom
import { render } from '@testing-library/svelte'
import { flushSync } from 'svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { GraphEdge, GraphNode } from '@/api/types'
import { graph } from '@/lib/state/graph.svelte'
import GraphCanvas from './GraphCanvas.svelte'

vi.mock('@/api/client', () => ({
  api: {
    projects: vi.fn(),
    tunnels: vi.fn(),
    graph: vi.fn(),
    neighbors: vi.fn(),
    agents: vi.fn(),
    health: vi.fn(),
  },
}))

const node = (id: string, kind = 'document'): GraphNode => ({ id, label: `Node ${id}`, kind })
const edge = (source_id: string, target_id: string, rel_type = 'related'): GraphEdge => ({
  source_id,
  target_id,
  rel_type,
})

function nodeElement(container: HTMLElement, id: string): SVGGElement {
  const found = [...container.querySelectorAll<SVGGElement>('g.node')].find(
    (element) => element.querySelector('text')?.textContent === `Node ${id}`,
  )
  if (!found) throw new Error(`node ${id} is not rendered`)
  return found
}

const core = (element: SVGGElement) => element.querySelector('circle.core')
const label = (element: SVGGElement) => element.querySelector('text')

let observers: { callback: ResizeObserverCallback; disconnect: ReturnType<typeof vi.fn> }[] = []

beforeEach(() => {
  observers = []
  vi.stubGlobal(
    'ResizeObserver',
    class {
      disconnect = vi.fn()
      constructor(public callback: ResizeObserverCallback) {
        observers.push(this)
      }
      observe() {}
      unobserve() {}
    },
  )
  graph.nodes = [node('a'), node('b', 'wiki'), node('c', 'tag')]
  graph.edges = [edge('a', 'b', 'wikilink'), edge('b', 'c', 'tagged')]
  graph.selectedId = null
  graph.hoveredId = null
  graph.focusMode = false
  graph.labelMode = 'hubs'
  graph.layoutOverride = null
  graph.mode = 'full'
  graph.loading = false
  graph.error = null
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('GraphCanvas', () => {
  it('renders one group per node and one line per edge into a single svg', () => {
    const { container } = render(GraphCanvas)
    flushSync()
    expect(container.querySelectorAll('svg')).toHaveLength(1)
    expect(container.querySelectorAll('g.node')).toHaveLength(3)
    expect(container.querySelectorAll('g.links line')).toHaveLength(2)
    expect(container.querySelector('filter')).toBeNull()
    expect(container.querySelector('feGaussianBlur')).toBeNull()
    expect(nodeElement(container, 'b').querySelector('circle.halo')?.getAttribute('fill')).toBe(
      'url(#graph-halo-wiki)',
    )
  })

  it('selection updates attributes without recreating any node element', () => {
    const { container } = render(GraphCanvas)
    flushSync()
    const svg = container.querySelector('svg')
    const before = ['a', 'b', 'c'].map((id) => nodeElement(container, id))
    expect(core(before[0]!)?.getAttribute('stroke-width')).toBe('1')

    graph.select('a')
    flushSync()
    const after = ['a', 'b', 'c'].map((id) => nodeElement(container, id))
    expect(after[0]).toBe(before[0])
    expect(after[1]).toBe(before[1])
    expect(after[2]).toBe(before[2])
    expect(container.querySelector('svg')).toBe(svg)
    expect(core(after[0]!)?.getAttribute('stroke-width')).toBe('2.5')
    expect(core(after[0]!)?.getAttribute('stroke')).toBe('var(--graph-node-stroke-selected)')
    expect(core(after[1]!)?.getAttribute('stroke-width')).toBe('1')

    graph.setFocusMode(true)
    flushSync()
    expect(nodeElement(container, 'c')).toBe(before[2])
    expect(nodeElement(container, 'c').getAttribute('opacity')).toBe('0.25')
    expect(nodeElement(container, 'b').getAttribute('opacity')).toBe('1')
  })

  it('hover toggles only the hovered label and keeps node identity', () => {
    const { container } = render(GraphCanvas)
    flushSync()
    const b = nodeElement(container, 'b')
    const c = nodeElement(container, 'c')
    expect(label(b)?.getAttribute('display')).toBe('none')

    b.dispatchEvent(new MouseEvent('mouseenter'))
    flushSync()
    expect(graph.hoveredId).toBe('b')
    expect(nodeElement(container, 'b')).toBe(b)
    expect(label(b)?.getAttribute('display')).toBeNull()
    expect(label(c)?.getAttribute('display')).toBe('none')

    b.dispatchEvent(new MouseEvent('mouseleave'))
    flushSync()
    expect(graph.hoveredId).toBeNull()
    expect(nodeElement(container, 'b')).toBe(b)
    expect(label(b)?.getAttribute('display')).toBe('none')
  })

  it('changing the seed input does not repaint a radial layout', () => {
    graph.mode = 'local'
    graph.loadedSeed = 'a'
    graph.setLayout('radial')
    const { container } = render(GraphCanvas)
    flushSync()
    const a = nodeElement(container, 'a')
    const transforms = () => ['a', 'b', 'c'].map((id) => nodeElement(container, id).getAttribute('transform'))
    const placed = transforms()
    expect(placed[0]).toBe('translate(400,300)')

    graph.seed = 'c'
    graph.select('c')
    flushSync()
    expect(nodeElement(container, 'a')).toBe(a)
    expect(transforms()).toEqual(placed)
  })

  it('a new graph load keeps the elements of nodes that stay and drops the rest', () => {
    const { container } = render(GraphCanvas)
    flushSync()
    const a = nodeElement(container, 'a')
    const svg = container.querySelector('svg')

    graph.nodes = [node('a'), node('d', 'entity')]
    graph.edges = [edge('a', 'd')]
    flushSync()
    expect(container.querySelector('svg')).toBe(svg)
    expect(nodeElement(container, 'a')).toBe(a)
    expect(container.querySelectorAll('g.node')).toHaveLength(2)
    expect(container.querySelectorAll('g.links line')).toHaveLength(1)
    expect(() => nodeElement(container, 'b')).toThrow()
  })

  it('shows a placeholder instead of nodes while the graph is empty', () => {
    graph.nodes = []
    graph.edges = []
    graph.loading = true
    const { container } = render(GraphCanvas)
    flushSync()
    const empty = container.querySelector('text.empty')
    expect(empty?.getAttribute('display')).toBeNull()
    expect(empty?.textContent).not.toBe('')

    graph.loading = false
    graph.error = 'HTTP 500: boom'
    flushSync()
    expect(empty?.textContent).toBe('HTTP 500: boom')

    graph.nodes = [node('a')]
    flushSync()
    expect(empty?.getAttribute('display')).toBe('none')
  })

  it('cleans up the svg and the observer on destroy', () => {
    const { container, unmount } = render(GraphCanvas)
    flushSync()
    unmount()
    expect(container.querySelector('svg')).toBeNull()
    expect(observers[0]?.disconnect).toHaveBeenCalled()
  })
})
