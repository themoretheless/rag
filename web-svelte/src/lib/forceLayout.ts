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
import { drag, type D3DragEvent, type DragBehavior } from 'd3-drag'
import type { GraphEdge, GraphNode } from '@/api/types'

export interface SimNode extends SimulationNodeDatum {
  id: string
  label: string
  kind: string
  document_id?: string | null
  radius: number
}

export interface SimLink extends SimulationLinkDatum<SimNode> {
  source: SimNode
  target: SimNode
  rel_type: string
}

/** Layout mode for client-side placement. Radial is for local seed views. */
export type LayoutMode = 'force' | 'radial'


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

/** Coarse bucket of a node kind (matches the nodeColor branches); names the halo gradients too. */
export function kindBucket(kind?: string): string {
  const k = (kind || '').toLowerCase()
  if (k.includes('tag')) return 'tag'
  if (k.includes('stub')) return 'stub'
  if (k.includes('entity')) return 'entity'
  if (k.includes('wiki') || k.includes('page')) return 'wiki'
  return 'document'
}



/** d3-drag behavior for graph nodes; reheat the sim while dragging. */
export function createNodeDrag(
  sim: { alphaTarget(alpha: number): { restart(): unknown } },
): DragBehavior<SVGGElement, SimNode, SimNode> {
  type NodeDragEvent = D3DragEvent<SVGGElement, SimNode, SimNode>
  return drag<SVGGElement, SimNode, SimNode>()
    .on('start', (event: NodeDragEvent, d) => {
      if (!event.active) sim.alphaTarget(0.3).restart()
      d.fx = d.x
      d.fy = d.y
    })
    .on('drag', (event: NodeDragEvent, d) => {
      d.fx = event.x
      d.fy = event.y
    })
    .on('end', (event: NodeDragEvent, d) => {
      if (!event.active) sim.alphaTarget(0)
      d.fx = null
      d.fy = null
    })
}

function degreeMap(edges: readonly GraphEdge[]): Map<string, number> {
  const deg = new Map<string, number>()
  for (const e of edges) {
    deg.set(e.source_id, (deg.get(e.source_id) || 0) + 1)
    deg.set(e.target_id, (deg.get(e.target_id) || 0) + 1)
  }
  return deg
}

/**
 * Simulation nodes for a graph view. A node that was already on screen (`previous`)
 * keeps its position and velocity; a new one starts near the viewport centre.
 */
function makeSimNodes(
  nodes: readonly GraphNode[],
  edges: readonly GraphEdge[],
  width: number,
  height: number,
  previous?: ReadonlyMap<string, SimNode>,
): SimNode[] {
  const deg = degreeMap(edges)
  return nodes.map((n) => {
    const old = previous?.get(n.id)
    const node: SimNode = {
      id: n.id,
      label: n.label,
      kind: n.kind || 'document',
      ...(n.document_id !== undefined ? { document_id: n.document_id } : {}),
      radius: nodeRadius(n.kind, deg.get(n.id) || 0),
      x: old?.x ?? width / 2 + (Math.random() - 0.5) * 80,
      y: old?.y ?? height / 2 + (Math.random() - 0.5) * 80,
    }
    if (old?.vx !== undefined) node.vx = old.vx
    if (old?.vy !== undefined) node.vy = old.vy
    return node
  })
}

function makeSimLinks(simNodes: SimNode[], edges: readonly GraphEdge[]): SimLink[] {
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
  edges: readonly GraphEdge[],
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

  // Undirected adjacency; a Set per node drops parallel edges.
  const neighbourSets = new Map<string, Set<string>>()
  const link = (from: string, to: string) => {
    const set = neighbourSets.get(from)
    if (set) set.add(to)
    else neighbourSets.set(from, new Set([to]))
  }
  for (const e of edges) {
    const s = e.source_id
    const t = e.target_id
    if (!nodeIds.has(s) || !nodeIds.has(t) || s === t) continue
    link(s, t)
    link(t, s)
  }
  const adj = new Map<string, string[]>()
  for (const [id, set] of neighbourSets) adj.set(id, [...set].sort())

  const depthOf = new Map<string, number>([[seed, 0]])
  const rings = new Map<number, string[]>([[0, [seed]]])
  // BFS frontier of [id, depth]; `head` avoids O(n) Array.shift().
  const queue: [id: string, depth: number][] = [[seed, 0]]

  for (let head = 0; head < queue.length; head++) {
    const entry = queue[head]
    if (!entry) break
    const [id, d] = entry
    for (const n of adj.get(id) ?? []) {
      if (depthOf.has(n)) continue
      depthOf.set(n, d + 1)
      const ring = rings.get(d + 1)
      if (ring) ring.push(n)
      else rings.set(d + 1, [n])
      queue.push([n, d + 1])
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
      ring.forEach((id, i) => {
        const node = byId.get(id)
        if (!node) return
        const x = id === seed ? cx : cx + 20 * i
        const y = cy
        node.x = x
        node.y = y
        node.fx = x
        node.fy = y
        node.vx = 0
        node.vy = 0
      })
      continue
    }

    const count = Math.max(1, ring.length)
    const r = depth * RING_GAP
    ring.forEach((id, i) => {
      const node = byId.get(id)
      if (!node) return
      const angle = (TAU * i) / count
      const x = cx + r * Math.cos(angle)
      const y = cy + r * Math.sin(angle)
      node.x = x
      node.y = y
      node.fx = x
      node.fy = y
      node.vx = 0
      node.vy = 0
    })
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

export interface GraphLayoutOptions {
  width: number
  height: number
  /** `radial` = deterministic RadialLocal (seed + BFS rings), frozen in place. */
  layout: LayoutMode
  /** Seed id / label / document_id for the radial centre. */
  seedKey?: string | null
}

/**
 * One long-lived d3-force simulation for the graph canvas.
 *
 * `setData` swaps the node / link sets in place (nodes that stay keep their
 * positions) and reheats; `relayout` re-applies the layout to the current data, e.g.
 * after a resize or a layout switch. Nothing is rebuilt between graph loads.
 */
export class GraphSimulation {
  readonly sim: Simulation<SimNode, SimLink>
  nodes: SimNode[] = []
  links: SimLink[] = []

  private edges: readonly GraphEdge[] = []
  private readonly linkForce = forceLink<SimNode, SimLink>()
    .id((d) => d.id)
    .distance((l) => (l.rel_type === 'wikilink' ? 70 : 90))
    .strength(0.45)
  private readonly chargeForce = forceManyBody<SimNode>().strength(-180).distanceMax(420)
  private readonly collideForce = forceCollide<SimNode>().radius((d) => d.radius + 4)

  constructor() {
    this.sim = forceSimulation<SimNode, SimLink>().force('collide', this.collideForce).stop()
  }

  /** Replace the graph and lay it out. */
  setData(nodes: readonly GraphNode[], edges: readonly GraphEdge[], options: GraphLayoutOptions): void {
    const previous = new Map(this.nodes.map((node) => [node.id, node]))
    this.nodes = makeSimNodes(nodes, edges, options.width, options.height, previous)
    this.links = makeSimLinks(this.nodes, edges)
    this.edges = edges
    this.sim.nodes(this.nodes)
    this.relayout(options)
  }

  /** Apply the layout to the current data and (for `force`) reheat the simulation. */
  relayout(options: GraphLayoutOptions): void {
    const { width, height, layout } = options
    if (layout === 'radial') {
      placeRadialLocal(this.nodes, this.edges, width, height, options.seedKey)
      // Frozen positions: no attracting forces, and a simulation that settles at once.
      this.sim.force('link', null).force('charge', null).force('center', null)
      this.collideForce.iterations(1).strength(0)
      this.sim.alpha(0).alphaDecay(1).stop()
      // One tick so the frozen positions are applied without drift.
      this.sim.tick()
      return
    }
    // Leaving a radial layout: release the pinned positions.
    for (const node of this.nodes) {
      node.fx = null
      node.fy = null
    }
    this.linkForce.links(this.links)
    this.collideForce.iterations(2).strength(1)
    this.sim
      .force('link', this.linkForce)
      .force('charge', this.chargeForce)
      .force('center', forceCenter(width / 2, height / 2))
      .alphaDecay(0.04)
      .alpha(1)
      .restart()
  }

  stop(): void {
    this.sim.stop()
  }
}
