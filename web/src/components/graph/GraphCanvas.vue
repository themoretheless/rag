<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import { select, type Selection } from 'd3-selection'
import { zoom, zoomIdentity, type ZoomBehavior } from 'd3-zoom'
import { useGraphStore } from '@/stores/graph'
import { buildSimulation, nodeColor, type SimLink, type SimNode } from '@/lib/forceLayout'

const graph = useGraphStore()
const host = ref<HTMLDivElement | null>(null)
let cleanup: (() => void) | null = null

/** Live zoom + layout state for fitView (rebuilt on each paint). */
let svgSel: Selection<SVGSVGElement, unknown, null, undefined> | null = null
let zoomBehavior: ZoomBehavior<SVGSVGElement, unknown> | null = null
let liveNodes: SimNode[] = []
let viewW = 800
let viewH = 600

function fitView() {
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
  const scale = Math.max(
    0.15,
    Math.min(4, Math.min(viewW / (bw + pad * 2), viewH / (bh + pad * 2))),
  )
  const tx = viewW / 2 - (scale * (minX + maxX)) / 2
  const ty = viewH / 2 - (scale * (minY + maxY)) / 2
  svgSel.call(zoomBehavior.transform as never, zoomIdentity.translate(tx, ty).scale(scale))
}

defineExpose({ fitView })

function paint() {
  cleanup?.()
  cleanup = null
  svgSel = null
  zoomBehavior = null
  liveNodes = []
  const el = host.value
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
    .scaleExtent([0.15, 4])
    .on('zoom', (event) => {
      root.attr('transform', event.transform)
    })
  zoomBehavior = z
  svg.call(z as never)
  svg.call(z.transform as never, zoomIdentity.translate(0, 0).scale(1))

  if (!graph.nodes.length) {
    root
      .append('text')
      .attr('x', width / 2)
      .attr('y', height / 2)
      .attr('text-anchor', 'middle')
      .attr('fill', '#64748b')
      .attr('font-size', 14)
      .text(graph.loading ? 'Loading graph…' : graph.error || 'No nodes')
    cleanup = () => {
      el.innerHTML = ''
      svgSel = null
      zoomBehavior = null
      liveNodes = []
    }
    return
  }

  const { sim, simNodes, simLinks } = buildSimulation(graph.nodes, graph.edges, width, height)
  liveNodes = simNodes

  const link = root
    .append('g')
    .attr('class', 'links')
    .selectAll('line')
    .data(simLinks)
    .join('line')
    .attr('stroke', (d) =>
      d.rel_type === 'wikilink' ? 'var(--graph-edge-wikilink)' : 'var(--graph-edge)',
    )
    .attr('stroke-width', (d) => (d.rel_type === 'wikilink' ? 1.6 : 1))
    .attr('stroke-opacity', 0.85)

  const nodeG = root
    .append('g')
    .attr('class', 'nodes')
    .selectAll('g')
    .data(simNodes)
    .join('g')
    .style('cursor', 'grab')
    .call(createNodeDrag(sim))
    .on('click', (event, d) => {
      // d3-drag marks the click as defaultPrevented after a real drag.
      if (event.defaultPrevented) return
      graph.select(d.id)
    })

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
    .attr('stroke', 'rgba(255,255,255,0.35)')
    .attr('stroke-width', 1)

  nodeG
    .append('text')
    .text((d) => (d.label.length > 28 ? d.label.slice(0, 26) + '…' : d.label))
    .attr('x', (d) => d.radius + 6)
    .attr('y', 4)
    .attr('fill', '#e2e8f0')
    .attr('font-size', 11)
    .attr('font-family', 'Inter, system-ui, sans-serif')
    .attr('paint-order', 'stroke')
    .attr('stroke', 'rgba(10,14,24,0.85)')
    .attr('stroke-width', 3)

  function tick() {
    link
      .attr('x1', (d) => (d.source as SimNode).x ?? 0)
      .attr('y1', (d) => (d.source as SimNode).y ?? 0)
      .attr('x2', (d) => (d.target as SimNode).x ?? 0)
      .attr('y2', (d) => (d.target as SimNode).y ?? 0)
    nodeG.attr('transform', (d) => `translate(${d.x ?? 0},${d.y ?? 0})`)
  }

  sim.on('tick', tick)

  // highlight selected
  const stopWatch = watch(
    () => graph.selectedId,
    (id) => {
      nodeG.select('circle.core').attr('stroke-width', (d) => (d.id === id ? 2.5 : 1))
      nodeG
        .select('circle.core')
        .attr('stroke', (d) => (d.id === id ? '#fff' : 'rgba(255,255,255,0.35)'))
    },
    { immediate: true },
  )

  cleanup = () => {
    sim.stop()
    stopWatch()
    el.innerHTML = ''
    svgSel = null
    zoomBehavior = null
    liveNodes = []
  }
}

let ro: ResizeObserver | null = null

onMounted(() => {
  paint()
  ro = new ResizeObserver(() => paint())
  if (host.value) ro.observe(host.value)
})

onUnmounted(() => {
  ro?.disconnect()
  cleanup?.()
})

watch(
  () => [graph.nodes, graph.edges, graph.loading] as const,
  () => paint(),
  { deep: true },
)
</script>

<template>
  <div ref="host" class="canvas" tabindex="-1" />
</template>

<style scoped>
.canvas {
  position: absolute;
  inset: 0;
  overflow: hidden;
  background:
    radial-gradient(1200px 600px at 50% 40%, var(--graph-vignette-a), transparent 60%),
    radial-gradient(800px 500px at 20% 80%, var(--graph-vignette-b), transparent 55%),
    var(--graph-bg);
}
</style>
