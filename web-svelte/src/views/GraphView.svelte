<script lang="ts">
  import { untrack } from 'svelte'
  import { graph } from '@/lib/state/graph.svelte'
  import { route, graphProjectParam, graphSeedParam, setGraphSeedQuery } from '@/lib/router.svelte'
  import GraphCanvas from '@/components/graph/GraphCanvas.svelte'
  import GraphToolbar from '@/components/graph/GraphToolbar.svelte'
  import GraphLegend from '@/components/graph/GraphLegend.svelte'
  import GraphInspector from '@/components/graph/GraphInspector.svelte'

  let canvas: GraphCanvas | null = $state(null)
  let navigationToken = 0

  /** Expand neighborhood and keep `?seed=` in sync. */
  async function expandSeed(seedKey?: string) {
    const token = ++navigationToken
    const s = (seedKey ?? graph.seed).trim()
    const targetProject = graph.project
    if (!s) {
      await graph.loadNeighbors('')
      return
    }
    graph.seed = s
    await graph.loadNeighbors(s)
    if (token === navigationToken && !graph.error && graph.mode === 'local' && graph.loadedSeed === s && graph.loadedProject === targetProject) {
      setGraphSeedQuery(s, targetProject)
    }
  }

  async function showFullGraph() {
    const token = ++navigationToken
    const targetProject = graph.project
    await graph.loadFull()
    if (token === navigationToken && !graph.error && graph.mode === 'full' && graph.loadedProject === targetProject) {
      setGraphSeedQuery(null, targetProject)
    }
  }

  async function changeProject() {
    const requestedProject = graph.project
    await reloadCurrentView()
    if (graph.error && graph.project === requestedProject && graph.loadedProject !== null) {
      graph.project = graph.loadedProject
    }
  }

  async function reloadCurrentView() {
    const token = ++navigationToken
    const targetMode = graph.requestedMode
    const targetSeed = targetMode === 'local'
      ? graph.loading
        ? graph.pendingSeed
        : graph.mode === 'local'
          ? graph.loadedSeed
          : graph.seed.trim() || null
      : null
    if (targetMode === 'local') await graph.loadNeighbors(targetSeed ?? '')
    else await graph.loadFull()
    if (token !== navigationToken || graph.error) return
    setGraphSeedQuery(graph.mode === 'local' ? graph.loadedSeed : null, graph.loadedProject ?? graph.project)
  }

  function retryCurrentView() {
    if (graph.requestedMode === 'local') void expandSeed()
    else void showFullGraph()
  }

  /** Apply `?seed=` from the route (external links, back/forward, wiki bridge). */
  async function applyRouteState(seed: string | null, projectParam: string | null, routeToken: number) {
    // Legacy seed links search all projects; an unscoped full-graph URL means
    // the main `rag` project. Successful loads below canonicalize both forms.
    const targetProject = projectParam !== null ? projectParam : seed ? '' : 'rag'
    graph.project = targetProject
    if (seed) {
      const loaded = graph.mode === 'local' && graph.loadedSeed === seed && graph.loadedProject === targetProject && graph.nodes.length > 0
      const pending = graph.loading && graph.requestedMode === 'local' && graph.pendingSeed === seed && graph.pendingProject === targetProject
      if (pending || (loaded && !graph.loading)) {
        if (loaded && !graph.loading) {
          graph.error = null
          graph.requestedMode = 'local'
          graph.seed = seed
        }
        if (routeToken === navigationToken && projectParam === null) setGraphSeedQuery(seed, targetProject)
        return
      }
      graph.seed = seed
      await graph.loadNeighbors(seed)
      if (routeToken === navigationToken && !graph.error && graph.mode === 'local' && graph.loadedSeed === seed && graph.loadedProject === targetProject && projectParam === null) {
        setGraphSeedQuery(seed, targetProject)
      }
      return
    }
    const loaded = graph.mode === 'full' && graph.loadedProject === targetProject && graph.nodes.length > 0
    const pending = graph.loading && graph.requestedMode === 'full' && graph.pendingProject === targetProject
    if (pending || (loaded && !graph.loading)) {
      if (loaded && !graph.loading) {
        graph.error = null
        graph.requestedMode = 'full'
      }
      if (routeToken === navigationToken && projectParam === null) setGraphSeedQuery(null, targetProject)
      return
    }
    await graph.loadFull()
    if (routeToken === navigationToken && !graph.error && graph.mode === 'full' && graph.loadedProject === targetProject && projectParam === null) {
      setGraphSeedQuery(null, targetProject)
    }
  }

  $effect(() => {
    if (route.name !== 'graph') return
    const seed = graphSeedParam()
    const project = graphProjectParam()
    // Only route changes should trigger this bridge. Tracking graph.mode/nodes
    // inside applyRouteSeed creates a competing full-graph request while an
    // explicit neighborhood expansion is still in flight.
    const routeToken = ++navigationToken
    untrack(() => void applyRouteState(seed, project, routeToken))
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
    onzoomin={() => canvas?.zoomIn()}
    onzoomout={() => canvas?.zoomOut()}
    onprojectchange={changeProject}
    onreload={reloadCurrentView}
  />
  <div class="stage">
    <GraphCanvas bind:this={canvas} />
    {#if graph.loading}
      <div class="graph-loading" role="status">
        <b>Загружаю {graph.pendingProject || 'все проекты'}…</b>
        {#if graph.loadedProject !== null}
          <span>На фоне предыдущий срез: {graph.loadedProject || 'все проекты'} · {graph.nodes.length} узлов</span>
        {/if}
      </div>
    {/if}
    {#if graph.error}<div class="graph-error" role="alert"><span>{graph.error}</span><button onclick={retryCurrentView}>Повторить</button></div>{/if}
    <GraphLegend />
    <GraphInspector onexpand={(seed) => expandSeed(seed)} />
  </div>
</div>

<style>
  .graph-page {
    position: relative;
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
  .graph-error {
    position: absolute;
    left: 14px;
    top: 66px;
    z-index: 9;
    max-width: min(560px, calc(100% - 370px));
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 10px;
    border: 1px solid color-mix(in srgb, var(--danger) 45%, var(--graph-panel-border));
    border-radius: 8px;
    background: var(--graph-panel-bg);
    color: var(--danger);
    font: 9px/1.35 var(--mono);
    box-shadow: var(--graph-shadow);
  }
  .graph-loading {
    position: absolute;
    left: 50%;
    top: 66px;
    z-index: 7;
    min-width: 210px;
    max-width: min(420px, calc(100% - 420px));
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 8px 11px;
    border: 1px solid color-mix(in srgb, var(--l1) 35%, var(--graph-panel-border));
    border-radius: 8px;
    background: color-mix(in srgb, var(--graph-panel-bg) 94%, transparent);
    box-shadow: var(--graph-shadow);
    color: var(--graph-panel-text);
    font: 9px/1.35 var(--mono);
    pointer-events: none;
    transform: translateX(-50%);
  }
  .graph-loading b { color: var(--l1); font-weight: 600; }
  .graph-loading span { overflow: hidden; color: var(--graph-panel-muted); text-overflow: ellipsis; white-space: nowrap; }
  .graph-error span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .graph-error button { margin-left: auto; border: 0; background: transparent; color: var(--graph-panel-text); font-size: 9px; cursor: pointer; }
  :global(.graph-page > .tb) { position:absolute;left:14px;right:14px;top:12px;z-index:8; }
</style>
