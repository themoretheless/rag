import { beforeEach, describe, expect, it, vi } from 'vitest'
import { api } from '@/api/client'
import type { GraphEdge, GraphNode, GraphView } from '@/api/types'

vi.mock('@/api/client', () => ({ api: { get: vi.fn(), graph: vi.fn(), neighbors: vi.fn() } }))

const mocked = vi.mocked(api)

function node(id: string, kind = 'doc'): GraphNode {
  return { id, label: id.toUpperCase(), kind } as GraphNode
}
function edge(source_id: string, target_id: string, rel_type = 'links'): GraphEdge {
  return { id: `${source_id}-${target_id}`, source_id, target_id, rel_type } as GraphEdge
}
const view: GraphView = {
  nodes: [node('a'), node('b'), node('c'), node('t', 'tag')],
  edges: [edge('a', 'b'), edge('b', 'c', 'mentions'), edge('t', 'b', 'tagged')],
} as GraphView

async function freshStore() {
  vi.resetModules()
  const mod = await import('./graph.svelte')
  return mod.graph
}

beforeEach(() => {
  vi.clearAllMocks()
  mocked.get.mockImplementation(async (path: string) => {
    if (path === '/v1/projects') return { items: [{ project_id: 'rag', document_count: 3 }] }
    return { items: [] }
  })
})

describe('graph store', () => {
  it('loadFull applies the view and auto-selects the most connected non-tag node', async () => {
    const graph = await freshStore()
    mocked.graph.mockResolvedValue(view)
    await graph.loadFull()
    expect(mocked.graph).toHaveBeenCalledWith({ max_nodes: 300, include_tags: true, project: 'rag' })
    expect(graph.nodes).toHaveLength(4)
    expect(graph.mode).toBe('full')
    expect(graph.loadedProject).toBe('rag')
    expect(graph.loading).toBe(false)
    expect(graph.selectedId).toBe('b')
    expect(graph.projects).toEqual([{ project_id: 'rag', document_count: 3 }])
  })

  it('loadFull records errors and clears loading', async () => {
    const graph = await freshStore()
    mocked.graph.mockRejectedValue(new Error('boom'))
    await graph.loadFull()
    expect(graph.error).toBe('boom')
    expect(graph.loading).toBe(false)
    expect(graph.pendingProject).toBeNull()
  })

  it('loadNeighbors requires a seed', async () => {
    const graph = await freshStore()
    await graph.loadNeighbors('  ')
    expect(graph.error).toBe('seed required')
    expect(graph.requestedMode).toBe('local')
    expect(mocked.neighbors).not.toHaveBeenCalled()
  })

  it('loadNeighbors loads a local view for the trimmed seed', async () => {
    const graph = await freshStore()
    mocked.neighbors.mockResolvedValue(view)
    await graph.loadNeighbors(' a ')
    expect(mocked.neighbors).toHaveBeenCalledWith('a', 2, 300, true, 'rag')
    expect(graph.mode).toBe('local')
    expect(graph.loadedSeed).toBe('a')
    expect(graph.seed).toBe('a')
  })

  it('ignores stale responses when a newer request wins', async () => {
    const graph = await freshStore()
    let resolveFirst!: (v: GraphView) => void
    mocked.graph.mockReturnValueOnce(new Promise((r) => (resolveFirst = r)))
    mocked.neighbors.mockResolvedValue({ nodes: [node('z')], edges: [] } as unknown as GraphView)
    const first = graph.loadFull()
    await graph.loadNeighbors('z')
    resolveFirst(view)
    await first
    expect(graph.mode).toBe('local')
    expect(graph.nodes.map((n) => n.id)).toEqual(['z'])
  })

  it('appends visible tunnel edges without duplicates', async () => {
    const graph = await freshStore()
    mocked.graph.mockResolvedValue(view)
    mocked.get.mockImplementation(async (path: string) => {
      if (path === '/v1/tunnels') return { items: [edge('a', 'c', 'tunnel'), edge('a', 'b'), edge('a', 'zz', 'tunnel')] }
      return { items: [] }
    })
    await graph.loadFull()
    await vi.waitFor(() => expect(graph.edges).toHaveLength(4))
    expect(graph.edges.at(-1)?.rel_type).toBe('tunnel')
  })

  it('focus mode derives the 1-hop neighbourhood and focus edges', async () => {
    const graph = await freshStore()
    graph.nodes = view.nodes
    graph.edges = view.edges
    graph.select('a')
    expect(graph.focusNodeIds).toBeNull()
    expect(graph.isFocusEdge('b', 'c')).toBe(true)
    graph.toggleFocusMode()
    expect([...graph.focusNodeIds!].sort()).toEqual(['a', 'b'])
    expect(graph.isFocusEdge('b', 'c')).toBe(false)
    expect(graph.isFocusEdge('a', 'b')).toBe(true)
    graph.setFocusMode(false)
    expect(graph.focusMode).toBe(false)
  })

  it('layout defaults to force and honours overrides', async () => {
    const graph = await freshStore()
    expect(graph.layout).toBe('force')
    graph.setLayout('radial')
    expect(graph.layout).toBe('radial')
    graph.setLayout(null)
    expect(graph.layout).toBe('force')
  })
})
