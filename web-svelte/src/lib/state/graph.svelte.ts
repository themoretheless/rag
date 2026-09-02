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
  depth = $state(2)
  maxNodes = $state(300)
  projects = $state<Array<{ project_id: string; document_count: number }>>([])
  project = $state('rag')
  includeTags = $state(true)
  selectedId = $state<string | null>(null)
  mode = $state<'full' | 'local'>('full')
  requestedMode = $state<'full' | 'local'>('full')
  loadedProject = $state<string | null>(null)
  loadedSeed = $state<string | null>(null)
  pendingProject = $state<string | null>(null)
  pendingSeed = $state<string | null>(null)
  /** When on and a node is selected, canvas dims non-neighbors. */
  focusMode = $state(false)
  labelMode = $state<'hubs' | 'all' | 'none'>('hubs')
  hoveredId = $state<string | null>(null)
  /** Manual layout override; null = auto (radial for local seed views, force for full). */
  layoutOverride = $state<LayoutMode | null>(null)
  private projectsLoaded = false
  private projectsPromise: Promise<void> | null = null
  private requestToken = 0

  /** Effective layout for the canvas. */
  layout = $derived<LayoutMode>(this.layoutOverride ?? 'force')

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
    if (!this.selectedId || !this.nodes.some((node) => node.id === this.selectedId)) {
      const degree = new Map<string, number>()
      const relationKinds = new Map<string, Set<string>>()
      for (const edge of this.edges) {
        degree.set(edge.source_id, (degree.get(edge.source_id) ?? 0) + 1)
        degree.set(edge.target_id, (degree.get(edge.target_id) ?? 0) + 1)
        if (!relationKinds.has(edge.source_id)) relationKinds.set(edge.source_id, new Set())
        if (!relationKinds.has(edge.target_id)) relationKinds.set(edge.target_id, new Set())
        relationKinds.get(edge.source_id)!.add(edge.rel_type)
        relationKinds.get(edge.target_id)!.add(edge.rel_type)
      }
      const candidates = this.nodes.filter((node) => !/tag|stub/i.test(node.kind ?? ''))
      this.selectedId = (candidates.length ? candidates : this.nodes).reduce<GraphNode | null>(
        (best, node) => {
          if (!best) return node
          const score = (relationKinds.get(node.id)?.size ?? 0) * 1_000 + (degree.get(node.id) ?? 0)
          const bestScore = (relationKinds.get(best.id)?.size ?? 0) * 1_000 + (degree.get(best.id) ?? 0)
          return score > bestScore ? node : best
        },
        null,
      )?.id ?? null
    }
  }

  private async ensureProjects() {
    if (this.projectsLoaded) return
    if (this.projectsPromise) return this.projectsPromise
    this.projectsPromise = (async () => {
      try {
        const response = await api.get<{ items?: Array<{ project_id: string; document_count: number }> }>('/v1/projects')
        this.projects = response.items ?? []
        this.projectsLoaded = true
      } catch {
        // Keep the requested scope intact. The graph endpoint must validate it;
        // silently falling back to all projects would make the URL misleading.
      } finally {
        this.projectsPromise = null
      }
    })()
    return this.projectsPromise
  }

  private async appendVisibleTunnels(requestToken: number) {
    try {
      const response = await api.get<{ items?: GraphEdge[] }>('/v1/tunnels')
      if (requestToken !== this.requestToken) return
      const visible = new Set(this.nodes.map((node) => node.id))
      const known = new Set(this.edges.map((edge) => edge.id || `${edge.source_id}:${edge.target_id}:${edge.rel_type}`))
      const additions: GraphEdge[] = []
      for (const edge of response.items ?? []) {
        const key = edge.id || `${edge.source_id}:${edge.target_id}:${edge.rel_type}`
        if (visible.has(edge.source_id) && visible.has(edge.target_id) && !known.has(key)) {
          additions.push(edge)
          known.add(key)
        }
      }
      if (additions.length) this.edges = [...this.edges, ...additions]
    } catch {
      // Tunnel overlay is optional; the base graph remains useful if unavailable.
    }
  }

  async loadFull() {
    const requestToken = ++this.requestToken
    const requestProject = this.project
    this.requestedMode = 'full'
    this.pendingProject = requestProject
    this.pendingSeed = null
    this.loading = true
    this.error = null
    try {
      await this.ensureProjects()
      if (requestToken !== this.requestToken) return
      const view = await api.graph({
        max_nodes: this.maxNodes,
        include_tags: this.includeTags,
        project: requestProject,
      })
      if (requestToken !== this.requestToken) return
      this.mode = 'full'
      this.loadedProject = requestProject
      this.loadedSeed = null
      this.applyView(view)
      this.loading = false
      void this.appendVisibleTunnels(requestToken)
    } catch (e) {
      if (requestToken === this.requestToken) this.error = e instanceof Error ? e.message : String(e)
    } finally {
      if (requestToken === this.requestToken) {
        this.loading = false
        this.pendingProject = null
        this.pendingSeed = null
      }
    }
  }

  async loadNeighbors(seedKey?: string) {
    const s = (seedKey ?? this.seed).trim()
    if (!s) {
      ++this.requestToken
      this.requestedMode = 'local'
      this.pendingProject = null
      this.pendingSeed = null
      this.loading = false
      this.error = 'seed required'
      return
    }
    const requestToken = ++this.requestToken
    const requestProject = this.project
    this.requestedMode = 'local'
    this.pendingProject = requestProject
    this.pendingSeed = s
    this.loading = true
    this.error = null
    this.seed = s
    try {
      await this.ensureProjects()
      if (requestToken !== this.requestToken) return
      const view = await api.neighbors(s, this.depth, this.maxNodes, this.includeTags, requestProject)
      if (requestToken !== this.requestToken) return
      this.mode = 'local'
      this.loadedProject = requestProject
      this.loadedSeed = s
      this.applyView(view)
      this.loading = false
      void this.appendVisibleTunnels(requestToken)
    } catch (e) {
      if (requestToken === this.requestToken) this.error = e instanceof Error ? e.message : String(e)
    } finally {
      if (requestToken === this.requestToken) {
        this.loading = false
        this.pendingProject = null
        this.pendingSeed = null
      }
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
