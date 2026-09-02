import {
  forceCenter,
  forceCollide,
  forceLink,
  forceManyBody,
  forceSimulation,
  type Simulation,
  type SimulationNodeDatum,
  type SimulationLinkDatum,
} from 'd3-force'
import { drag, type DragBehavior } from 'd3-drag'
import type { GraphEdge, GraphNode } from '@/api/types'

export interface SimNode extends SimulationNodeDatum {
  id: string
  label: string
  kind: string
  document_id?: string | null
  radius: number
}

export interface SimLink extends SimulationLinkDatum<SimNode> {
  rel_type: string
}

/** Layout mode for client-side placement. Radial is for local seed views. */
export type LayoutMode = 'force' | 'radial'

export interface BuildSimulationOptions {
  /** Default `force`. `radial` = deterministic RadialLocal (seed + BFS rings). */
  layout?: LayoutMode
  /** Seed id / label / document_id for radial center. */
  seedKey?: string | null
}

/** Ring gap in px (matches rag-mcp-ui RadialLocal scale). */
export const RING_GAP = 140

export function nodeColor(kind?: string): string {
  const k = (kind || '').toLowerCase()
  if (k.includes('tag')) return 'var(--graph-node-tag)'
  if (k.includes('stub')) return 'var(--graph-node-stub)'
  if (k.includes('entity')) return 'var(--graph-node-entity)'
  if (k.includes('wiki') || k.includes('page')) return 'var(--graph-node-wiki)'
  return 'var(--graph-node-doc)'
}

export function nodeRadius(kind?: string, degree = 1): number {
  const base = kind?.includes('tag') ? 5 : kind?.includes('stub') ? 6 : 8
  return Math.min(18, base + Math.sqrt(Math.max(0, degree)) * 1.6)
}

/** Coarse legend bucket for a node kind (matches nodeColor branches). */
function legendBucket(kind?: string): string {
  const k = (kind || '').toLowerCase()
  if (k.includes('tag')) return 'tag'
  if (k.includes('stub')) return 'stub'
  if (k.includes('entity')) return 'entity'
  if (k.includes('wiki') || k.includes('page')) return 'wiki'
  return 'document'
}

export interface LegendEntry {
  /** Bucket key (tag | stub | wiki | document). */
  source: string
  label: string
  color: string
}

/** Distinct kind buckets present in `nodes`, for the graph legend. */
export function legendEntries(nodes: GraphNode[]): LegendEntry[] {
  const seen = new Map<string, LegendEntry>()
  for (const n of nodes) {
    const source = legendBucket(n.kind)
    if (!seen.has(source)) {
      seen.set(source, { source, label: source, color: nodeColor(n.kind) })
    }
  }
  return [...seen.values()].sort((a, b) => a.label.localeCompare(b.label))
}

/** d3-drag behavior for graph nodes; reheat the sim while dragging. */
export function createNodeDrag(
  sim: Simulation<SimNode, undefined>,
): DragBehavior<SVGGElement, SimNode, SimNode> {
  return drag<SVGGElement, SimNode, SimNode>()
    .on('start', (event, d) => {
      if (!event.active) sim.alphaTarget(0.3).restart()
      d.fx = d.x
      d.fy = d.y
    })
    .on('drag', (event, d) => {
      d.fx = event.x
      d.fy = event.y
    })
    .on('end', (event, d) => {
      if (!event.active) sim.alphaTarget(0)
      d.fx = null
      d.fy = null
    })
}

function degreeMap(edges: GraphEdge[]): Map<string, number> {
  const deg = new Map<string, number>()
  for (const e of edges) {
    deg.set(e.source_id, (deg.get(e.source_id) || 0) + 1)
    deg.set(e.target_id, (deg.get(e.target_id) || 0) + 1)
  }
  return deg
}

function makeSimNodes(
  nodes: GraphNode[],
  edges: GraphEdge[],
  width: number,
  height: number,
): SimNode[] {
  const deg = degreeMap(edges)
  return nodes.map((n) => ({
    id: n.id,
    label: n.label,
    kind: n.kind || 'document',
    document_id: n.document_id,
    radius: nodeRadius(n.kind, deg.get(n.id) || 0),
    x: width / 2 + (Math.random() - 0.5) * 80,
    y: height / 2 + (Math.random() - 0.5) * 80,
  }))
}

function makeSimLinks(simNodes: SimNode[], edges: GraphEdge[]): SimLink[] {
  const byId = new Map(simNodes.map((n) => [n.id, n]))
  const simLinks: SimLink[] = []
  for (const e of edges) {
    const s = byId.get(e.source_id)
    const t = byId.get(e.target_id)
    if (!s || !t) continue
    simLinks.push({ source: s, target: t, rel_type: e.rel_type })
  }
  return simLinks
}

/**
 * Resolve seed among sim nodes: exact id, then document_id, then label,
 * then lexicographically first id (stable fallback).
 */
export function resolveSeedId(simNodes: SimNode[], seedKey?: string | null): string | null {
  if (!simNodes.length) return null
  const key = (seedKey ?? '').trim()
  if (key) {
    if (simNodes.some((n) => n.id === key)) return key
    const byDoc = simNodes.find((n) => n.document_id === key)
    if (byDoc) return byDoc.id
    const byLabel = simNodes.find((n) => n.label === key)
    if (byLabel) return byLabel.id
    const keyLower = key.toLowerCase()
    const byLabelCi = simNodes.find((n) => n.label.toLowerCase() === keyLower)
    if (byLabelCi) return byLabelCi.id
  }
  const ids = simNodes.map((n) => n.id).sort()
  return ids[0] ?? null
}

/**
 * Deterministic RadialLocal placement (aligned with crates/rag-mcp-ui layout.rs):
 * 1. Seed at viewport center
 * 2. Undirected BFS depth rings
 * 3. r = depth * RING_GAP
 * 4. Equal angles on each ring, sorted by node id
 * 5. Unreachable nodes on one outer ring
 * Positions are frozen via fx/fy.
 */
export function placeRadialLocal(
  simNodes: SimNode[],
  edges: GraphEdge[],
  width: number,
  height: number,
  seedKey?: string | null,
): void {
  if (!simNodes.length) return

  const cx = width / 2
  const cy = height / 2
  const seed = resolveSeedId(simNodes, seedKey)
  if (!seed) return

  const nodeIds = new Set(simNodes.map((n) => n.id))
  const byId = new Map(simNodes.map((n) => [n.id, n]))

  const adj = new Map<string, string[]>()
  for (const e of edges) {
    const s = e.source_id
    const t = e.target_id
    if (!nodeIds.has(s) || !nodeIds.has(t) || s === t) continue
    if (!adj.has(s)) adj.set(s, [])
    if (!adj.has(t)) adj.set(t, [])
    adj.get(s)!.push(t)
    adj.get(t)!.push(s)
  }
  for (const [, neis] of adj) {
    neis.sort()
    // dedupe
    let w = 0
    for (let r = 0; r < neis.length; r++) {
      if (r === 0 || neis[r] !== neis[r - 1]) neis[w++] = neis[r]!
    }
    neis.length = w
  }

  const depthOf = new Map<string, number>()
  const rings = new Map<number, string[]>()
  const q: string[] = []

  depthOf.set(seed, 0)
  rings.set(0, [seed])
  q.push(seed)

  while (q.length) {
    const id = q.shift()!
    const d = depthOf.get(id)!
    const neis = adj.get(id)
    if (!neis) continue
    for (const n of neis) {
      if (depthOf.has(n)) continue
      depthOf.set(n, d + 1)
      if (!rings.has(d + 1)) rings.set(d + 1, [])
      rings.get(d + 1)!.push(n)
      q.push(n)
    }
  }

  const maxDepth = rings.size ? Math.max(...rings.keys()) : 0
  const unreachable = simNodes
    .map((n) => n.id)
    .filter((id) => !depthOf.has(id))
    .sort()
  if (unreachable.length) {
    rings.set(maxDepth + 1, unreachable)
  }

  const depths = [...rings.keys()].sort((a, b) => a - b)
  const TAU = Math.PI * 2

  for (const depth of depths) {
    const ring = (rings.get(depth) ?? []).slice().sort()
    if (depth === 0) {
      for (let i = 0; i < ring.length; i++) {
        const id = ring[i]!
        const node = byId.get(id)
        if (!node) continue
        const x = id === seed ? cx : cx + 20 * i
        const y = cy
        node.x = x
        node.y = y
        node.fx = x
        node.fy = y
        node.vx = 0
        node.vy = 0
      }
      continue
    }

    const count = Math.max(1, ring.length)
    const r = depth * RING_GAP
    for (let i = 0; i < ring.length; i++) {
      const id = ring[i]!
      const node = byId.get(id)
      if (!node) continue
      const angle = (TAU * i) / count
      const x = cx + r * Math.cos(angle)
      const y = cy + r * Math.sin(angle)
      node.x = x
      node.y = y
      node.fx = x
      node.fy = y
      node.vx = 0
      node.vy = 0
    }
  }

  // Guarantee every node has a frozen position.
  for (const n of simNodes) {
    if (n.fx == null || n.fy == null) {
      n.x = cx
      n.y = cy
      n.fx = cx
      n.fy = cy
      n.vx = 0
      n.vy = 0
    }
  }
}

export function buildSimulation(
  nodes: GraphNode[],
  edges: GraphEdge[],
  width: number,
  height: number,
  options?: BuildSimulationOptions,
) {
  const layout: LayoutMode = options?.layout ?? 'force'
  const simNodes = makeSimNodes(nodes, edges, width, height)
  const simLinks = makeSimLinks(simNodes, edges)

  if (layout === 'radial') {
    placeRadialLocal(simNodes, edges, width, height, options?.seedKey)
    // Frozen positions: minimal sim so callers can still listen for tick once.
    const sim = forceSimulation(simNodes)
      .force(
        'collide',
        forceCollide<SimNode>()
          .radius((d) => d.radius + 4)
          .iterations(1)
          .strength(0),
      )
      .alpha(0)
      .alphaDecay(1)
      .stop()
    // One tick so initial paint positions are applied without drift.
    sim.tick()
    return { sim, simNodes, simLinks, layout: 'radial' as const }
  }

  const sim = forceSimulation(simNodes)
    .force(
      'link',
      forceLink<SimNode, SimLink>(simLinks)
        .id((d) => d.id)
        .distance((l) => (l.rel_type === 'wikilink' ? 70 : 90))
        .strength(0.45),
    )
    .force('charge', forceManyBody().strength(-180).distanceMax(420))
    .force('center', forceCenter(width / 2, height / 2))
    .force(
      'collide',
      forceCollide<SimNode>().radius((d) => d.radius + 4).iterations(2),
    )
    .alphaDecay(0.04)

  return { sim, simNodes, simLinks, layout: 'force' as const }
}
