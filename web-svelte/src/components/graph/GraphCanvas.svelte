<script lang="ts">
  import { onDestroy, untrack } from 'svelte'
  import { select, type Selection } from 'd3-selection'
  import { zoom, zoomIdentity, type D3ZoomEvent, type ZoomBehavior } from 'd3-zoom'
  import type { GraphEdge, GraphNode } from '@/api/types'
  import { debounce } from '@/lib/async'
  import {
    GraphSimulation,
    createNodeDrag,
    kindBucket,
    nodeColor,
    type LayoutMode,
    type SimLink,
    type SimNode,
  } from '@/lib/forceLayout'
  import { graph } from '@/lib/state/graph.svelte'
  import { ui } from '@/lib/state/ui.svelte'

  type SvgSelection = Selection<SVGSVGElement, unknown, null, undefined>
  type LayerSelection = Selection<SVGGElement, unknown, null, undefined>
  type NodeSelection = Selection<SVGGElement, SimNode, SVGGElement, unknown>
  type LinkSelection = Selection<SVGLineElement, SimLink, SVGGElement, unknown>

  /** The persistent SVG skeleton. Built once; graph loads only join data into it. */
  interface Scene {
    svg: SvgSelection
    viewport: LayerSelection
    links: LayerSelection
    nodes: LayerSelection
    empty: Selection<SVGTextElement, unknown, null, undefined>
    zoom: ZoomBehavior<SVGSVGElement, unknown>
  }

  /** Fallback viewport before the host element has been measured. */
  const DEFAULT_VIEW_WIDTH = 800
  const DEFAULT_VIEW_HEIGHT = 600
  /** Width reserved for the inspector panel; also the minimum usable canvas width. */
  const INSPECTOR_WIDTH = 360
  const MIN_USABLE_WIDTH = 360
  /** Ticks after a graph load before the view is fitted to the settled layout. */
  const FIT_AFTER_TICKS = 100
  const RESIZE_DEBOUNCE_MS = 120
  /** Node kinds that get their own halo gradient (see `kindBucket`). */
  const HALO_BUCKETS = ['document', 'wiki', 'tag', 'entity', 'stub'] as const
  /** How far the halo reaches beyond the node's radius. */
  const HALO_SPREAD = 13
  /** Hubs (by radius) keep their label in `hubs` label mode. */
  const HUB_RADIUS = 11

  let host: HTMLDivElement | null = $state(null)
  let viewW = DEFAULT_VIEW_WIDTH
  let viewH = DEFAULT_VIEW_HEIGHT

  let scene: Scene | null = null
  let nodeSel: NodeSelection | null = null
  let linkSel: LinkSelection | null = null
  /** Node elements by id, for updates that touch one or two nodes (hover). */
  const nodeElements = new Map<string, SVGGElement>()
  let renderedNodes: readonly GraphNode[] | null = null
  let tickCount = 0

  const simulation = new GraphSimulation()
  const nodeDrag = createNodeDrag(simulation.sim)
  simulation.sim.on('tick', () => {
    renderPositions()
    tickCount += 1
    if (tickCount === FIT_AFTER_TICKS) fitView()
  })

  export function fitView() {
    if (!scene) return
    const nodes = simulation.nodes
    if (!nodes.length) {
      scene.zoom.transform(scene.svg, zoomIdentity)
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
      scene.zoom.transform(scene.svg, zoomIdentity)
      return
    }

    const pad = 48
    const bw = Math.max(1, maxX - minX)
    const bh = Math.max(1, maxY - minY)
    const usableW = Math.max(MIN_USABLE_WIDTH, viewW - (graph.selectedId ? INSPECTOR_WIDTH : 0))
    const scale = Math.max(0.35, Math.min(1.8, Math.min(usableW / (bw + pad * 2), viewH / (bh + pad * 2))))
    const tx = usableW / 2 - (scale * (minX + maxX)) / 2
    const ty = viewH / 2 - (scale * (minY + maxY)) / 2
    scene.zoom.transform(scene.svg, zoomIdentity.translate(tx, ty).scale(scale))
  }

  function zoomBy(factor: number) {
    if (scene) scene.zoom.scaleBy(scene.svg, factor)
  }

  export function zoomIn() { zoomBy(1.25) }
  export function zoomOut() { zoomBy(0.8) }

  function createScene(el: HTMLDivElement): Scene {
    const svg: SvgSelection = select(el).append('svg').attr('width', '100%').attr('height', '100%')

    // Soft node halo as a radial gradient per kind: the look of the former per-node
    // feGaussianBlur filter without a filter pass for every node on every frame.
    const defs = svg.append('defs')
    for (const bucket of HALO_BUCKETS) {
      const gradient = defs.append('radialGradient').attr('id', `graph-halo-${bucket}`)
      for (const [offset, opacity] of [[0, 0.18], [0.45, 0.18], [0.72, 0.09], [1, 0]] as const) {
        gradient
          .append('stop')
          .attr('offset', offset)
          .style('stop-color', nodeColor(bucket))
          .style('stop-opacity', opacity)
      }
    }

    const viewport = svg.append('g').attr('class', 'viewport')
    const empty = viewport
      .append('text')
      .attr('class', 'empty')
      .attr('text-anchor', 'middle')
      .attr('fill', 'var(--graph-panel-muted)')
      .attr('font-size', 14)
    const links = viewport.append('g').attr('class', 'links')
    const nodes = viewport.append('g').attr('class', 'nodes')

    const zoomBehavior = zoom<SVGSVGElement, unknown>()
      .scaleExtent([0.3, 2.5])
      // Explicit extent: the default one reads SVG `viewBox.baseVal`, which is layout-dependent.
      .extent((): [[number, number], [number, number]] => [[0, 0], [viewW, viewH]])
      .on('zoom', (event: D3ZoomEvent<SVGSVGElement, unknown>) => {
        viewport.attr('transform', event.transform.toString())
      })
    svg.call(zoomBehavior)

    return { svg, viewport, links, nodes, empty, zoom: zoomBehavior }
  }

  /** Read the host size; returns whether it changed. */
  function measure(el: HTMLDivElement): boolean {
    const width = el.clientWidth || DEFAULT_VIEW_WIDTH
    const height = el.clientHeight || DEFAULT_VIEW_HEIGHT
    if (width === viewW && height === viewH) return false
    viewW = width
    viewH = height
    return true
  }

  const linkKey = (d: SimLink) => `${d.source.id}\u0000${d.target.id}\u0000${d.rel_type}`
  const linkStroke = (d: SimLink) =>
    d.rel_type === 'wikilink' ? 'var(--graph-edge-wikilink)' : d.rel_type === 'tagged' ? 'var(--graph-node-tag)' : d.rel_type === 'tunnel' ? 'var(--graph-node-entity)' : 'var(--graph-edge)'
  const linkWidth = (d: SimLink) => (d.rel_type === 'tunnel' ? 2 : d.rel_type === 'wikilink' ? 1.6 : 1)
  const linkDash = (d: SimLink) => (d.rel_type === 'tagged' ? '2 4' : d.rel_type === 'tunnel' ? '7 5' : null)
  const nodeLabel = (d: SimNode) => (d.label.length > 28 ? d.label.slice(0, 26) + '…' : d.label)

  /** Keyed data join: elements of nodes / links that stay on screen are reused. */
  function joinData(target: Scene) {
    linkSel = target.links
      .selectAll<SVGLineElement, SimLink>('line')
      .data(simulation.links, linkKey)
      .join('line')
      .attr('stroke', linkStroke)
      .attr('stroke-width', linkWidth)
      .attr('stroke-dasharray', linkDash)

    nodeSel = target.nodes
      .selectAll<SVGGElement, SimNode>('g.node')
      .data(simulation.nodes, (d) => d.id)
      .join(
        (enter) => {
          const group = enter
            .append('g')
            .attr('class', 'node')
            .style('cursor', 'grab')
            .call(nodeDrag)
            .on('click', (event: MouseEvent, d) => {
              // d3-drag marks the click as defaultPrevented after a real drag.
              if (event.defaultPrevented) return
              graph.select(d.id)
            })
            .on('mouseenter', (_event, d) => { graph.hoveredId = d.id })
            .on('mouseleave', () => { graph.hoveredId = null })
          group.append('circle').attr('class', 'halo')
          group
            .append('circle')
            .attr('class', 'core')
            .attr('stroke', 'var(--graph-node-stroke)')
            .attr('stroke-width', 1)
          group
            .append('text')
            .attr('y', 4)
            .attr('fill', 'var(--graph-label)')
            .attr('font-size', 11)
            .attr('font-family', 'Golos Text, system-ui, sans-serif')
            .attr('paint-order', 'stroke')
            .attr('stroke', 'var(--graph-label-stroke)')
            .attr('stroke-width', 3)
          return group
        },
        (update) => update,
        (exit) => exit.remove(),
      )

    // Kind, label and degree-based radius can change for a node that stays on screen.
    nodeSel
      .select<SVGCircleElement>('circle.halo')
      .attr('r', (d) => d.radius + HALO_SPREAD)
      .attr('fill', (d) => `url(#graph-halo-${kindBucket(d.kind)})`)
    nodeSel
      .select<SVGCircleElement>('circle.core')
      .attr('r', (d) => d.radius)
      .attr('fill', (d) => nodeColor(d.kind))
    nodeSel
      .select<SVGTextElement>('text')
      .text(nodeLabel)
      .attr('x', (d) => d.radius + 6)

    nodeElements.clear()
    nodeSel.each(function (d) {
      nodeElements.set(d.id, this)
    })
  }

  function renderPositions() {
    linkSel
      ?.attr('x1', (d) => d.source.x ?? 0)
      .attr('y1', (d) => d.source.y ?? 0)
      .attr('x2', (d) => d.target.x ?? 0)
      .attr('y2', (d) => d.target.y ?? 0)
    nodeSel?.attr('transform', (d) => `translate(${d.x ?? 0},${d.y ?? 0})`)
  }

  function labelDisplay(d: SimNode): string | null {
    const visible =
      graph.labelMode === 'all' ||
      (graph.labelMode === 'hubs' && (d.radius >= HUB_RADIUS || d.id === graph.hoveredId || d.id === graph.selectedId))
    return visible ? null : 'none'
  }

  /** Highlight the selected node, dim non-neighbours in focus mode, refresh label visibility. */
  function applySelection() {
    const id = graph.selectedId
    const focus = graph.focusNodeIds
    if (nodeSel) {
      nodeSel
        .select<SVGCircleElement>('circle.core')
        .attr('stroke-width', (d) => (d.id === id ? 2.5 : 1))
        .attr('stroke', (d) => (d.id === id ? 'var(--graph-node-stroke-selected)' : 'var(--graph-node-stroke)'))
      nodeSel.attr('opacity', (d) => (focus && !focus.has(d.id) ? 0.25 : 1))
      nodeSel.select<SVGTextElement>('text').attr('display', labelDisplay)
    }
    linkSel?.attr('stroke-opacity', (d) => (focus && !graph.isFocusEdge(d.source.id, d.target.id) ? 0.12 : 0.85))
  }

  /** Hover only decides whether one node shows its label: touch that node, not the graph. */
  function applyHoverLabel(id: string | null) {
    const element = id === null ? undefined : nodeElements.get(id)
    if (element) select<SVGGElement, SimNode>(element).select<SVGTextElement>('text').attr('display', labelDisplay)
  }

  function layoutOptions(layout: LayoutMode, seedKey: string | null) {
    return { width: viewW, height: viewH, layout, seedKey }
  }

  function applyViewBox(target: Scene) {
    target.svg.attr('viewBox', `0 0 ${viewW} ${viewH}`)
    target.empty.attr('x', viewW / 2).attr('y', viewH / 2)
  }

  function renderGraph(el: HTMLDivElement, nodes: readonly GraphNode[], edges: readonly GraphEdge[], layout: LayoutMode, seedKey: string | null) {
    scene ??= createScene(el)
    measure(el)
    applyViewBox(scene)
    simulation.setData(nodes, edges, layoutOptions(layout, seedKey))
    joinData(scene)
    tickCount = 0
    // A different node set is a new view: start from the unzoomed frame, fit once settled.
    if (nodes !== renderedNodes) scene.zoom.transform(scene.svg, zoomIdentity)
    renderedNodes = nodes
    renderPositions()
    applySelection()
  }

  // Graph data and layout. Only these re-run the join; selection and hover never do.
  $effect(() => {
    const el = host
    if (!el) return
    const nodes = graph.nodes
    const edges = graph.edges
    const layout = graph.layout
    // The radial centre is the seed of the *loaded* neighbourhood. The editable seed
    // input follows the selection and must not re-lay-out the canvas.
    const seedKey = layout === 'radial' && graph.mode === 'local' ? graph.loadedSeed : null
    untrack(() => { renderGraph(el, nodes, edges, layout, seedKey) })
  })

  // Placeholder while there is nothing to draw.
  $effect(() => {
    if (!host) return
    const message = graph.nodes.length ? null : graph.loading ? ui.t('graphLoading') : graph.error || ui.t('graphNoNodes')
    scene?.empty.attr('display', message === null ? 'none' : null).text(message ?? '')
  })

  // Selection, focus and label mode: attribute updates only.
  $effect(() => {
    void graph.selectedId
    void graph.focusNodeIds
    void graph.labelMode
    untrack(applySelection)
  })

  let hoveredBefore: string | null = null
  $effect(() => {
    const hovered = graph.hoveredId
    untrack(() => {
      applyHoverLabel(hoveredBefore)
      applyHoverLabel(hovered)
      hoveredBefore = hovered
    })
  })

  // A resize re-lays-out the current data; bursts of observer callbacks collapse into one.
  const onResize = debounce(() => {
    if (!host || !scene || !measure(host)) return
    applyViewBox(scene)
    const layout = graph.layout
    simulation.relayout(layoutOptions(layout, layout === 'radial' && graph.mode === 'local' ? graph.loadedSeed : null))
    tickCount = 0
    renderPositions()
  }, RESIZE_DEBOUNCE_MS)

  $effect(() => {
    const el = host
    if (!el) return
    const observer = new ResizeObserver(() => { onResize() })
    observer.observe(el)
    return () => { observer.disconnect() }
  })

  onDestroy(() => {
    onResize.cancel()
    simulation.stop()
    simulation.sim.on('tick', null)
    scene?.svg.remove()
    scene = null
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
