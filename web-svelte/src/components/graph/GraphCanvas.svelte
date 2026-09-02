<script lang="ts">
  import { onDestroy, onMount, untrack } from 'svelte'
  import { select, type Selection } from 'd3-selection'
  import { zoom, zoomIdentity, type ZoomBehavior, type ZoomTransform } from 'd3-zoom'
  import { graph } from '@/lib/state/graph.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import {
    buildSimulation,
    createNodeDrag,
    nodeColor,
    type SimLink,
    type SimNode,
  } from '@/lib/forceLayout'

  let host: HTMLDivElement | null = $state(null)
  let cleanup: (() => void) | null = null

  /** Live zoom + layout state for fitView/minimap (rebuilt on each paint). */
  let svgSel: Selection<SVGSVGElement, unknown, null, undefined> | null = null
  let zoomBehavior: ZoomBehavior<SVGSVGElement, unknown> | null = null
  let liveNodes: SimNode[] = []
  let viewW = $state(800)
  let viewH = $state(600)
  let nodeSel: Selection<SVGGElement, SimNode, SVGGElement, unknown> | null = null
  let linkSel: Selection<SVGLineElement, SimLink, SVGGElement, unknown> | null = null
  let currentTransform: ZoomTransform = zoomIdentity

  export function fitView() {
    if (!svgSel || !zoomBehavior) return
    const nodes = liveNodes
    if (!nodes.length) {
      svgSel.call(zoomBehavior.transform as never, zoomIdentity)
      return
    }

    let minX = Infinity
    let minY = Infinity
    let maxX = -Infinity
    let maxY = -Infinity
    for (const n of nodes) {
      const x = n.x ?? 0
      const y = n.y ?? 0
      const r = n.radius + 8
      if (x - r < minX) minX = x - r
      if (y - r < minY) minY = y - r
      if (x + r > maxX) maxX = x + r
      if (y + r > maxY) maxY = y + r
    }
    if (!Number.isFinite(minX) || !Number.isFinite(minY)) {
      svgSel.call(zoomBehavior.transform as never, zoomIdentity)
      return
    }

    const pad = 48
    const bw = Math.max(1, maxX - minX)
    const bh = Math.max(1, maxY - minY)
    const usableW = Math.max(360, viewW - (graph.selectedId ? 360 : 0))
    const scale = Math.max(
      0.35,
      Math.min(1.8, Math.min(usableW / (bw + pad * 2), viewH / (bh + pad * 2))),
    )
    const tx = usableW / 2 - (scale * (minX + maxX)) / 2
    const ty = viewH / 2 - (scale * (minY + maxY)) / 2
    svgSel.call(zoomBehavior.transform as never, zoomIdentity.translate(tx, ty).scale(scale))
  }

  function zoomBy(factor: number) {
    if (!svgSel || !zoomBehavior) return
    svgSel.call(zoomBehavior.scaleBy as never, factor)
  }

  export function zoomIn() { zoomBy(1.25) }
  export function zoomOut() { zoomBy(0.8) }

  function paint() {
    cleanup?.()
    cleanup = null
    svgSel = null
    zoomBehavior = null
    liveNodes = []
    nodeSel = null
    linkSel = null
    const el = host
    if (!el) return

    const width = el.clientWidth || 800
    const height = el.clientHeight || 600
    viewW = width
    viewH = height
    el.innerHTML = ''

    const svg = select(el)
      .append('svg')
      .attr('width', '100%')
      .attr('height', '100%')
      .attr('viewBox', `0 0 ${width} ${height}`)

    svgSel = svg as Selection<SVGSVGElement, unknown, null, undefined>

    // soft vignette background
    const defs = svg.append('defs')
    const glow = defs
      .append('filter')
      .attr('id', 'glow')
      .attr('x', '-50%')
      .attr('y', '-50%')
      .attr('width', '200%')
      .attr('height', '200%')
    glow.append('feGaussianBlur').attr('stdDeviation', '3.5').attr('result', 'coloredBlur')
    const merge = glow.append('feMerge')
    merge.append('feMergeNode').attr('in', 'coloredBlur')
    merge.append('feMergeNode').attr('in', 'SourceGraphic')

    const root = svg.append('g').attr('class', 'viewport')

    const z = zoom<SVGSVGElement, unknown>()
      .scaleExtent([0.3, 2.5])
      .on('zoom', (event) => {
        currentTransform = event.transform
        root.attr('transform', event.transform)
      })
    zoomBehavior = z
    currentTransform = zoomIdentity
    svg.call(z as never)
    svg.call(z.transform as never, zoomIdentity.translate(0, 0).scale(1))

    if (!graph.nodes.length) {
      root
        .append('text')
        .attr('x', width / 2)
        .attr('y', height / 2)
        .attr('text-anchor', 'middle')
        .attr('fill', 'var(--graph-panel-muted)')
        .attr('font-size', 14)
        .text(graph.loading ? ui.t('graphLoading') : graph.error || ui.t('graphNoNodes'))
      cleanup = () => {
        el.innerHTML = ''
        svgSel = null
        zoomBehavior = null
        liveNodes = []
      }
      return
    }

    const { sim, simNodes, simLinks } = buildSimulation(graph.nodes, graph.edges, width, height, {
      layout: graph.layout,
      seedKey: graph.mode === 'local' ? graph.seed : null,
    })
    liveNodes = simNodes

    const link = root
      .append('g')
      .attr('class', 'links')
      .selectAll<SVGLineElement, SimLink>('line')
      .data(simLinks)
      .join('line')
      .attr('stroke', (d) => d.rel_type === 'wikilink' ? 'var(--graph-edge-wikilink)' : d.rel_type === 'tagged' ? 'var(--graph-node-tag)' : d.rel_type === 'tunnel' ? 'var(--graph-node-entity)' : 'var(--graph-edge)')
      .attr('stroke-width', (d) => d.rel_type === 'tunnel' ? 2 : d.rel_type === 'wikilink' ? 1.6 : 1)
      .attr('stroke-dasharray', (d) => d.rel_type === 'tagged' ? '2 4' : d.rel_type === 'tunnel' ? '7 5' : null)
      .attr('stroke-opacity', 0.85)
    linkSel = link

    const nodeG = root
      .append('g')
      .attr('class', 'nodes')
      .selectAll<SVGGElement, SimNode>('g')
      .data(simNodes)
      .join('g')
      .style('cursor', 'grab')
      .call(createNodeDrag(sim) as never)
      .on('click', (event, d) => {
        // d3-drag marks the click as defaultPrevented after a real drag.
        if (event.defaultPrevented) return
        graph.select(d.id)
      })
      .on('mouseenter', (_event, d) => { graph.hoveredId = d.id; applySelection() })
      .on('mouseleave', () => { graph.hoveredId = null; applySelection() })
    nodeSel = nodeG

    nodeG
      .append('circle')
      .attr('r', (d) => d.radius + 6)
      .attr('fill', (d) => nodeColor(d.kind))
      .attr('opacity', 0.18)
      .attr('filter', 'url(#glow)')

    nodeG
      .append('circle')
      .attr('class', 'core')
      .attr('r', (d) => d.radius)
      .attr('fill', (d) => nodeColor(d.kind))
      .attr('stroke', 'var(--graph-node-stroke)')
      .attr('stroke-width', 1)

    const labels = nodeG
      .append('text')
      .text((d) => (d.label.length > 28 ? d.label.slice(0, 26) + '…' : d.label))
      .attr('x', (d) => d.radius + 6)
      .attr('y', 4)
      .attr('fill', 'var(--graph-label)')
      .attr('font-size', 11)
      .attr('font-family', 'Golos Text, system-ui, sans-serif')
      .attr('paint-order', 'stroke')
      .attr('stroke', 'var(--graph-label-stroke)')
      .attr('stroke-width', 3)

    labels.attr('display', (d) => graph.labelMode === 'all' || (graph.labelMode === 'hubs' && d.radius >= 11) ? null : 'none')

    let tickCount = 0
    function tick() {
      link
        .attr('x1', (d) => (d.source as SimNode).x ?? 0)
        .attr('y1', (d) => (d.source as SimNode).y ?? 0)
        .attr('x2', (d) => (d.target as SimNode).x ?? 0)
        .attr('y2', (d) => (d.target as SimNode).y ?? 0)
      nodeG.attr('transform', (d) => `translate(${d.x ?? 0},${d.y ?? 0})`)
      tickCount += 1
      if (tickCount === 100) fitView()
    }

    sim.on('tick', tick)
    // Radial layout's sim is stopped: paint one frame manually.
    tick()
    applySelection()

    cleanup = () => {
      sim.stop()
      el.innerHTML = ''
      svgSel = null
      zoomBehavior = null
      liveNodes = []
      nodeSel = null
      linkSel = null
    }
  }

  /** Highlight selected node + dim non-neighbors when focus mode is on. */
  function applySelection() {
    const id = graph.selectedId
    const focus = graph.focusNodeIds
    if (nodeSel) {
      nodeSel
        .select('circle.core')
        .attr('stroke-width', (d: SimNode) => (d.id === id ? 2.5 : 1))
        .attr('stroke', (d: SimNode) =>
          d.id === id ? 'var(--graph-node-stroke-selected)' : 'var(--graph-node-stroke)',
        )
      nodeSel.attr('opacity', (d: SimNode) => (focus && !focus.has(d.id) ? 0.25 : 1))
      nodeSel.select('text').attr('display', (d: SimNode) => graph.labelMode === 'all' || (graph.labelMode === 'hubs' && (d.radius >= 11 || d.id === graph.hoveredId || d.id === id)) ? null : 'none')
    }
    if (linkSel) {
      linkSel.attr('stroke-opacity', (d) =>
        focus && !graph.isFocusEdge((d.source as SimNode).id, (d.target as SimNode).id)
          ? 0.12
          : 0.85,
      )
    }
  }

  $effect(() => {
    // Keep the previous graph stable while a replacement request is loading.
    // Loading text only needs a repaint when there is no graph yet.
    void graph.nodes
    void graph.edges
    void graph.layout
    void graph.mode
    if (graph.layout === 'radial') void graph.seed
    if (!graph.nodes.length) void graph.loading
    untrack(paint)
  })

  $effect(() => {
    // Cheap attr updates only; no repaint.
    void graph.selectedId
    void graph.focusNodeIds
    void graph.hoveredId
    void graph.labelMode
    applySelection()
  })

  let ro: ResizeObserver | null = null

  onMount(() => {
    paint()
    ro = new ResizeObserver(() => paint())
    if (host) ro.observe(host)
  })

  onDestroy(() => {
    ro?.disconnect()
    cleanup?.()
  })
</script>

<div class="stage">
  <div bind:this={host} class="canvas" tabindex="-1"></div>
  <div class="hint">Тяните узел — соседи потянутся и спружинят обратно · фон — панорама · физика: rubber</div>
</div>

<style>
  .stage {
    position: absolute;
    inset: 0;
  }
  .canvas {
    position: absolute;
    inset: 0;
    overflow: hidden;
    background:
      radial-gradient(rgba(255,255,255,.055) 1px,transparent 1px),
      radial-gradient(1200px 600px at 50% 40%, var(--graph-vignette-a), transparent 60%),
      radial-gradient(800px 500px at 20% 80%, var(--graph-vignette-b), transparent 55%),
      var(--graph-bg);
    background-size:26px 26px,auto,auto,auto;
  }
  .hint{position:absolute;right:14px;bottom:14px;padding:8px 11px;border:1px solid var(--graph-panel-border);border-radius:9px;background:var(--graph-panel-bg);color:var(--graph-panel-muted);font-size:11.5px;pointer-events:none;backdrop-filter:blur(8px)}
</style>
