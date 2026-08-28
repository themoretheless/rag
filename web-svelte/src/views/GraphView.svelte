<script lang="ts">
  import { graph } from '@/lib/state/graph.svelte'
  import { route, graphSeedParam, setGraphSeedQuery } from '@/lib/router.svelte'
  import GraphCanvas from '@/components/graph/GraphCanvas.svelte'
  import GraphToolbar from '@/components/graph/GraphToolbar.svelte'
  import GraphLegend from '@/components/graph/GraphLegend.svelte'
  import GraphInspector from '@/components/graph/GraphInspector.svelte'

  let canvas: GraphCanvas | null = $state(null)

  /** Expand neighborhood and keep `?seed=` in sync. */
  async function expandSeed(seedKey?: string) {
    const s = (seedKey ?? graph.seed).trim()
    if (!s) {
      graph.error = 'seed required'
      return
    }
    graph.seed = s
    setGraphSeedQuery(s)
    await graph.loadNeighbors(s)
  }

  async function showFullGraph() {
    setGraphSeedQuery(null)
    await graph.loadFull()
  }

  /** Apply `?seed=` from the route (external links, back/forward, wiki bridge). */
  async function applyRouteSeed(seed: string | null) {
    if (seed) {
      if (
        graph.mode === 'local' &&
        graph.seed === seed &&
        (graph.nodes.length > 0 || graph.loading)
      )
        return
      graph.seed = seed
      await graph.loadNeighbors(seed)
      return
    }
    if (graph.mode === 'full' && (graph.nodes.length > 0 || graph.loading)) return
    await graph.loadFull()
  }

  $effect(() => {
    if (route.name !== 'graph') return
    applyRouteSeed(graphSeedParam())
  })

  // Select fills seed input so Expand uses the selection; does not rewrite URL alone.
  let prevSelected: string | null = null
  $effect(() => {
    const id = graph.selectedId
    if (!id || id === prevSelected) {
      prevSelected = id
      return
    }
    prevSelected = id
    const n = graph.nodes.find((x) => x.id === id)
    if (!n) return
    graph.seed = (n.document_id || n.label || n.id).trim()
  })
</script>

<div class="graph-page">
  <GraphToolbar
    onexpand={() => expandSeed()}
    onfull={() => showFullGraph()}
    onfit={() => canvas?.fitView()}
  />
  <div class="stage">
    <GraphCanvas bind:this={canvas} />
    <GraphLegend />
    <GraphInspector onexpand={(seed) => expandSeed(seed)} />
  </div>
</div>

<style>
  .graph-page {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    flex: 1;
    background: var(--graph-bg);
    color: var(--graph-panel-text);
  }
  .stage {
    position: relative;
    flex: 1;
    min-height: 0;
  }
</style>
