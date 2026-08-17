import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '@/api/client'
import type { GraphEdge, GraphNode, GraphView } from '@/api/types'

export const useGraphStore = defineStore('graph', () => {
  const nodes = ref<GraphNode[]>([])
  const edges = ref<GraphEdge[]>([])
  const loading = ref(false)
  const error = ref<string | null>(null)
  const seed = ref('')
  const depth = ref(1)
  const maxNodes = ref(300)
  const includeTags = ref(false)
  const selectedId = ref<string | null>(null)
  const mode = ref<'full' | 'local'>('full')
  /** When on and a node is selected, canvas dims non-neighbors. */
  const focusMode = ref(true)

  function applyView(view: GraphView) {
    nodes.value = view.nodes ?? []
    edges.value = view.edges ?? []
  }

  async function loadFull() {
    loading.value = true
    error.value = null
    mode.value = 'full'
    try {
      const view = await api.graph({
        max_nodes: maxNodes.value,
        include_tags: includeTags.value,
      })
      applyView(view)
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      loading.value = false
    }
  }

  async function loadNeighbors(seedKey?: string) {
    const s = (seedKey ?? seed.value).trim()
    if (!s) {
      error.value = 'seed required'
      return
    }
    loading.value = true
    error.value = null
    mode.value = 'local'
    seed.value = s
    try {
      const view = await api.neighbors(s, depth.value, maxNodes.value)
      applyView(view)
    } catch (e) {
      error.value = e instanceof Error ? e.message : String(e)
    } finally {
      loading.value = false
    }
  }

  function select(id: string | null) {
    selectedId.value = id
  }

  function setFocusMode(on: boolean) {
    focusMode.value = on
  }

  function toggleFocusMode() {
    focusMode.value = !focusMode.value
  }

  /**
   * Ids in the focus set: selected node + 1-hop neighbors.
   * Null when focus mode is off or nothing is selected (no dimming).
   */
  const focusNodeIds = computed(() => {
    const id = selectedId.value
    if (!focusMode.value || !id) return null
    const set = new Set<string>([id])
    for (const e of edges.value) {
      if (e.source_id === id) set.add(e.target_id)
      else if (e.target_id === id) set.add(e.source_id)
    }
    return set
  })

  /** True when an edge is incident to the selected node (kept bright in focus mode). */
  function isFocusEdge(sourceId: string, targetId: string): boolean {
    const id = selectedId.value
    if (!focusMode.value || !id) return true
    return sourceId === id || targetId === id
  }

  return {
    nodes,
    edges,
    loading,
    error,
    seed,
    depth,
    maxNodes,
    includeTags,
    selectedId,
    mode,
    focusMode,
    focusNodeIds,
    loadFull,
    loadNeighbors,
    select,
    setFocusMode,
    toggleFocusMode,
    isFocusEdge,
  }
})
