import { describe, expect, it } from 'vitest'
import type { GraphEdge } from '@/api/types'
import { RING_GAP, legendEntries, nodeColor, nodeRadius, placeRadialLocal, resolveSeedId, type SimNode } from './forceLayout'

const node = (id: string, label = id, document_id: string | null = null): SimNode => ({ id, label, kind: 'doc', document_id, radius: 8 })
const edge = (source_id: string, target_id: string): GraphEdge => ({ source_id, target_id, rel_type: 'links' })

describe('nodeColor / nodeRadius', () => {
  it('maps kinds to theme colours with a document fallback', () => {
    expect(nodeColor('Tag')).toBe('var(--graph-node-tag)')
    expect(nodeColor('stub')).toBe('var(--graph-node-stub)')
    expect(nodeColor('entity')).toBe('var(--graph-node-entity)')
    expect(nodeColor('wiki_page')).toBe('var(--graph-node-wiki)')
    expect(nodeColor(undefined)).toBe('var(--graph-node-doc)')
  })

  it('grows radius with degree and caps it at 18', () => {
    expect(nodeRadius('tag', 0)).toBe(5)
    expect(nodeRadius('doc', 4)).toBeCloseTo(8 + 2 * 1.6)
    expect(nodeRadius('doc', 10_000)).toBe(18)
    expect(nodeRadius('doc', -5)).toBe(8)
  })
})

describe('legendEntries', () => {
  it('returns one sorted entry per bucket', () => {
    const entries = legendEntries([
      { id: 'a', label: 'a', kind: 'wiki' },
      { id: 'b', label: 'b', kind: 'tag' },
      { id: 'c', label: 'c', kind: 'page' },
      { id: 'd', label: 'd' },
    ])
    expect(entries.map((entry) => entry.source)).toEqual(['document', 'tag', 'wiki'])
  })
})

describe('resolveSeedId', () => {
  const nodes = [node('n2', 'Beta', 'doc-2'), node('n1', 'Alpha', 'doc-1')]

  it('prefers id, then document_id, then label (case-insensitive)', () => {
    expect(resolveSeedId(nodes, 'n2')).toBe('n2')
    expect(resolveSeedId(nodes, 'doc-1')).toBe('n1')
    expect(resolveSeedId(nodes, 'Beta')).toBe('n2')
    expect(resolveSeedId(nodes, ' alpha ')).toBe('n1')
  })

  it('falls back to the lexicographically first id and handles empty input', () => {
    expect(resolveSeedId(nodes, 'missing')).toBe('n1')
    expect(resolveSeedId(nodes, null)).toBe('n1')
    expect(resolveSeedId([], 'n1')).toBeNull()
  })
})

describe('placeRadialLocal', () => {
  it('centres the seed, rings neighbours by BFS depth and freezes positions', () => {
    const nodes = [node('s'), node('a'), node('b'), node('lonely')]
    placeRadialLocal(nodes, [edge('s', 'a'), edge('a', 'b')], 800, 600, 's')
    const [s, a, b, lonely] = nodes
    expect([s.x, s.y]).toEqual([400, 300])
    expect(s.fx).toBe(s.x)
    const dist = (n: SimNode) => Math.hypot((n.x ?? 0) - 400, (n.y ?? 0) - 300)
    expect(dist(a)).toBeCloseTo(RING_GAP)
    expect(dist(b)).toBeCloseTo(2 * RING_GAP)
    expect(dist(lonely)).toBeCloseTo(3 * RING_GAP)
  })

  it('is a no-op for an empty node list', () => {
    expect(() => placeRadialLocal([], [], 100, 100)).not.toThrow()
  })
})
