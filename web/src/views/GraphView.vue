<script setup lang="ts">
import { onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useGraphStore } from '@/stores/graph'
import GraphCanvas from '@/components/graph/GraphCanvas.vue'
import GraphToolbar from '@/components/graph/GraphToolbar.vue'
import GraphLegend from '@/components/graph/GraphLegend.vue'
import GraphInspector from '@/components/graph/GraphInspector.vue'

const graph = useGraphStore()
const route = useRoute()
const router = useRouter()

function seedFromQuery(): string | null {
  const s = route.query.seed
  if (typeof s === 'string' && s.trim()) return s.trim()
  return null
}

/** Write current expand seed into the URL (shareable / back-forward). */
function syncSeedQuery(seed: string | null) {
  const current = seedFromQuery()
  if (seed) {
    if (current === seed) return
    void router.replace({
      name: 'graph',
      query: { ...route.query, seed },
    })
    return
  }
  if (current == null) return
  const next = { ...route.query }
  delete next.seed
  void router.replace({ name: 'graph', query: next })
}

/** Expand neighborhood and keep `?seed=` in sync. */
async function expandSeed(seedKey?: string) {
  const s = (seedKey ?? graph.seed).trim()
  if (!s) {
    graph.error = 'seed required'
    return
  }
  graph.seed = s
  syncSeedQuery(s)
  await graph.loadNeighbors(s)
}

async function showFullGraph() {
  syncSeedQuery(null)
  await graph.loadFull()
}

/** Apply `?seed=` from the route (external links, back/forward, wiki bridge). */
async function applyRouteSeed(seed: string | null) {
  if (seed) {
    if (graph.mode === 'local' && graph.seed === seed && graph.nodes.length > 0) return
    graph.seed = seed
    await graph.loadNeighbors(seed)
    return
  }
  if (graph.mode === 'full' && graph.nodes.length > 0) return
  await graph.loadFull()
}

onMounted(() => {
  void applyRouteSeed(seedFromQuery())
})

watch(
  () => route.query.seed,
  (seed) => {
    const s = typeof seed === 'string' && seed.trim() ? seed.trim() : null
    void applyRouteSeed(s)
  },
)

// Select fills seed input so Expand uses the selection; does not rewrite URL alone.
watch(
  () => graph.selectedId,
  (id) => {
    if (!id) return
    const n = graph.nodes.find((x) => x.id === id)
    if (!n) return
    graph.seed = (n.document_id || n.label || n.id).trim()
  },
)
</script>

<template>
  <div class="graph-page">
    <GraphToolbar @expand="expandSeed()" @full="showFullGraph()" />
    <div class="stage">
      <GraphCanvas @expand-node="expandSeed" />
      <GraphLegend />
      <GraphInspector @expand="expandSeed()" />
    </div>
  </div>
</template>

<style scoped>
.graph-page {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--graph-bg);
  color: var(--graph-panel-text);
}
.stage {
  position: relative;
  flex: 1;
  min-height: 0;
}
</style>
