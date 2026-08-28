import { api } from '@/api/client'
import type { GraphEdge, GraphNode, GraphView } from '@/api/types'
import type { LayoutMode } from '@/lib/forceLayout'

/** Graph explorer state: nodes/edges, seed expand, selection, layout. */
class GraphStore {
  nodes = $state<GraphNode[]>([])
  edges = $state<GraphEdge[]>([])
  loading = $state(false)
  error = $state<string | null>(null)
  seed = $state('')
  depth = $state(1)
  maxNodes = $state(300)
  includeTags = $state(false)
  selectedId = $state<string | null>(null)
  mode = $state<'full' | 'local'>('full')
  /** When on and a node is selected, canvas dims non-neighbors. */
  focusMode = $state(true)
  /** Manual layout override; null = auto (radial for local seed views, force for full). */
  layoutOverride = $state<LayoutMode | null>(null)

  /** Effective layout for the canvas. */
  layout = $derived<LayoutMode>(
    this.layoutOverride ?? (this.mode === 'local' ? 'radial' : 'force'),
  )

  /**
   * Ids in the focus set: selected node + 1-hop neighbors.
   * Null when focus mode is off or nothing is selected (no dimming).
   */
  focusNodeIds = $derived.by((): Set<string> | null => {
    const id = this.selectedId
    if (!this.focusMode || !id) return null
    const set = new Set<string>([id])
    for (const e of this.edges) {
      if (e.source_id === id) set.add(e.target_id)
      else if (e.target_id === id) set.add(e.source_id)
    }
    return set
  })

  private applyView(view: GraphView) {
    this.nodes = view.nodes ?? []
    this.edges = view.edges ?? []
  }

  async loadFull() {
    this.loading = true
    this.error = null
    this.mode = 'full'
    try {
      const view = await api.graph({
        max_nodes: this.maxNodes,
        include_tags: this.includeTags,
      })
      this.applyView(view)
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e)
    } finally {
      this.loading = false
    }
  }

  async loadNeighbors(seedKey?: string) {
    const s = (seedKey ?? this.seed).trim()
    if (!s) {
      this.error = 'seed required'
      return
    }
    this.loading = true
    this.error = null
    this.mode = 'local'
    this.seed = s
    try {
      const view = await api.neighbors(s, this.depth, this.maxNodes)
      this.applyView(view)
    } catch (e) {
      this.error = e instanceof Error ? e.message : String(e)
    } finally {
      this.loading = false
    }
  }

  select(id: string | null) {
    this.selectedId = id
  }

  setFocusMode(on: boolean) {
    this.focusMode = on
  }

  toggleFocusMode() {
    this.focusMode = !this.focusMode
  }

  setLayout(mode: LayoutMode | null) {
    this.layoutOverride = mode
  }

  /** True when an edge is incident to the selected node (kept bright in focus mode). */
  isFocusEdge(sourceId: string, targetId: string): boolean {
    const id = this.selectedId
    if (!this.focusMode || !id) return true
    return sourceId === id || targetId === id
  }
}

export const graph = new GraphStore()
