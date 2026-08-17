<script setup lang="ts">
/**
 * Bottom-right graph overview.
 * Prefer `points` + `transform` + view size from GraphCanvas for true sync.
 * Without points, runs a lightweight local force layout from the graph store
 * so the component still shows a rough nodes bbox overview.
 */
import { onMounted, onUnmounted, ref, shallowRef, watch } from 'vue'
import { select, type Selection } from 'd3-selection'
import { useGraphStore } from '@/stores/graph'
import { buildSimulation, nodeColor } from '@/lib/forceLayout'

export interface MinimapPoint {
  id: string
  x: number
  y: number
  kind?: string
}

export interface MinimapTransform {
  x: number
  y: number
  k: number
}

const props = withDefaults(
  defineProps<{
    /** World-space node positions from the main canvas (preferred). */
    points?: MinimapPoint[]
    /** d3-zoom transform of the main viewport. */
    transform?: MinimapTransform
    /** Main canvas size in CSS/SVG pixels (for viewport rect). */
    viewWidth?: number
    viewHeight?: number
    /** Outer minimap box size (square) in CSS px. */
    size?: number
  }>(),
  {
    points: undefined,
    transform: () => ({ x: 0, y: 0, k: 1 }),
    viewWidth: 800,
    viewHeight: 600,
    size: 148,
  },
)

const emit = defineEmits<{
  /** Request main canvas to center on world (x, y). */
  navigate: [payload: { x: number; y: number }]
}>()

const graph = useGraphStore()
const host = ref<HTMLDivElement | null>(null)

const localPoints = shallowRef<MinimapPoint[]>([])
let localCleanup: (() => void) | null = null

/** World→minimap map derived on each paint. */
let mapState: {
  minX: number
  minY: number
  scale: number
  pad: number
  inner: number
} | null = null

const PAD = 10
const LOCAL_W = 800
const LOCAL_H = 600

function activePoints(): MinimapPoint[] {
  if (props.points && props.points.length) return props.points
  return localPoints.value
}

function bboxOf(pts: MinimapPoint[]) {
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const p of pts) {
    const x = p.x
    const y = p.y
    if (!Number.isFinite(x) || !Number.isFinite(y)) continue
    if (x < minX) minX = x
    if (y < minY) minY = y
    if (x > maxX) maxX = x
    if (y > maxY) maxY = y
  }
  if (!Number.isFinite(minX)) {
    return { minX: 0, minY: 0, maxX: 1, maxY: 1 }
  }
  // Degenerate single-point / zero-size bbox: expand so scale stays finite.
  if (maxX - minX < 1) {
    minX -= 40
    maxX += 40
  }
  if (maxY - minY < 1) {
    minY -= 40
    maxY += 40
  }
  return { minX, minY, maxX, maxY }
}

function worldToMini(x: number, y: number): [number, number] {
  const m = mapState
  if (!m) return [0, 0]
  return [m.pad + (x - m.minX) * m.scale, m.pad + (y - m.minY) * m.scale]
}

function miniToWorld(mx: number, my: number): [number, number] {
  const m = mapState
  if (!m || m.scale === 0) return [0, 0]
  return [(mx - m.pad) / m.scale + m.minX, (my - m.pad) / m.scale + m.minY]
}

function paint() {
  const el = host.value
  if (!el) return

  const size = props.size
  const pts = activePoints()
  el.innerHTML = ''

  const svg = select(el)
    .append('svg')
    .attr('width', size)
    .attr('height', size)
    .attr('viewBox', `0 0 ${size} ${size}`)
    .attr('role', 'img')
    .attr('aria-label', 'Graph minimap')

  if (!pts.length) {
    mapState = null
    svg
      .append('text')
      .attr('x', size / 2)
      .attr('y', size / 2)
      .attr('text-anchor', 'middle')
      .attr('dominant-baseline', 'middle')
      .attr('fill', '#64748b')
      .attr('font-size', 10)
      .text(graph.loading ? '…' : 'empty')
    return
  }

  const { minX, minY, maxX, maxY } = bboxOf(pts)
  const bw = maxX - minX
  const bh = maxY - minY
  const inner = size - PAD * 2
  const scale = Math.min(inner / bw, inner / bh)
  // Center content inside the square.
  const usedW = bw * scale
  const usedH = bh * scale
  const offX = PAD + (inner - usedW) / 2
  const offY = PAD + (inner - usedH) / 2

  mapState = { minX, minY, scale, pad: 0, inner: size }
  // Override pad so worldToMini uses offsets that center the bbox.
  mapState = {
    minX,
    minY,
    scale,
    pad: 0,
    inner: size,
  }

  // Custom mapping with centering offsets (avoid mutating pad misuse).
  const toMini = (x: number, y: number): [number, number] => [
    offX + (x - minX) * scale,
    offY + (y - minY) * scale,
  ]
  const toWorld = (mx: number, my: number): [number, number] => [
    (mx - offX) / scale + minX,
    (my - offY) / scale + minY,
  ]
  // Keep miniToWorld/worldToMini used by click handler via mapState shim:
  mapState = {
    minX: minX - offX / scale,
    minY: minY - offY / scale,
    scale,
    pad: 0,
    inner: size,
  }

  // Soft frame background is CSS; draw faint edges for orientation.
  svg
    .append('rect')
    .attr('class', 'frame')
    .attr('x', 0.5)
    .attr('y', 0.5)
    .attr('width', size - 1)
    .attr('height', size - 1)
    .attr('rx', 8)
    .attr('fill', 'transparent')
    .attr('stroke', 'rgba(255,255,255,0.06)')

  const layer = svg.append('g').attr('class', 'content')

  // Edges (optional, only when local points share ids with store edges).
  const byId = new Map(pts.map((p) => [p.id, p]))
  const edgeSel = layer.append('g').attr('class', 'edges')
  for (const e of graph.edges) {
    const s = byId.get(e.source_id)
    const t = byId.get(e.target_id)
    if (!s || !t) continue
    const [x1, y1] = toMini(s.x, s.y)
    const [x2, y2] = toMini(t.x, t.y)
    edgeSel
      .append('line')
      .attr('x1', x1)
      .attr('y1', y1)
      .attr('x2', x2)
      .attr('y2', y2)
      .attr(
        'stroke',
        e.rel_type === 'wikilink' ? 'var(--graph-edge-wikilink)' : 'var(--graph-edge)',
      )
      .attr('stroke-width', 0.6)
      .attr('stroke-opacity', 0.55)
  }

  const nodesG = layer.append('g').attr('class', 'nodes')
  for (const p of pts) {
    const [cx, cy] = toMini(p.x, p.y)
    nodesG
      .append('circle')
      .attr('cx', cx)
      .attr('cy', cy)
      .attr('r', 2.2)
      .attr('fill', nodeColor(p.kind))
      .attr('opacity', 0.92)
  }

  // Viewport rectangle in world space via inverse zoom transform.
  const t = props.transform ?? { x: 0, y: 0, k: 1 }
  const k = t.k || 1
  const vw = props.viewWidth
  const vh = props.viewHeight
  const wx0 = (0 - t.x) / k
  const wy0 = (0 - t.y) / k
  const wx1 = (vw - t.x) / k
  const wy1 = (vh - t.y) / k
  const [vx0, vy0] = toMini(wx0, wy0)
  const [vx1, vy1] = toMini(wx1, wy1)
  const rx = Math.min(vx0, vx1)
  const ry = Math.min(vy0, vy1)
  const rw = Math.abs(vx1 - vx0)
  const rh = Math.abs(vy1 - vy0)

  svg
    .append('rect')
    .attr('class', 'viewport')
    .attr('x', rx)
    .attr('y', ry)
    .attr('width', Math.max(2, rw))
    .attr('height', Math.max(2, rh))
    .attr('fill', 'rgba(124, 92, 252, 0.12)')
    .attr('stroke', 'rgba(167, 139, 250, 0.85)')
    .attr('stroke-width', 1.25)
    .attr('rx', 2)
    .style('pointer-events', 'none')

  // Click: navigate main canvas so that world point sits under click.
  svg.on('click', (event: MouseEvent) => {
    const target = event.currentTarget as SVGSVGElement | null
    if (!target) return
    const rect = target.getBoundingClientRect()
    const mx = ((event.clientX - rect.left) / rect.width) * size
    const my = ((event.clientY - rect.top) / rect.height) * size
    const [wx, wy] = toWorld(mx, my)
    emit('navigate', { x: wx, y: wy })
  })

  // Silence unused if TS keeps map helpers for future brush; keep helpers live.
  void worldToMini
  void miniToWorld
  void mapState
}

function stopLocalSim() {
  localCleanup?.()
  localCleanup = null
}

function startLocalSim() {
  stopLocalSim()
  // External points own the layout.
  if (props.points && props.points.length) {
    localPoints.value = []
    return
  }
  if (!graph.nodes.length) {
    localPoints.value = []
    return
  }

  const { sim, simNodes } = buildSimulation(graph.nodes, graph.edges, LOCAL_W, LOCAL_H)

  const push = () => {
    localPoints.value = simNodes.map((n) => ({
      id: n.id,
      x: n.x ?? 0,
      y: n.y ?? 0,
      kind: n.kind,
    }))
    paint()
  }

  // Coarse ticks only: minimap is overview, not the source of truth.
  sim.on('tick', () => {
    // Throttle paints: every ~3 ticks is enough for a small map.
    if ((sim.alpha() * 1000) % 3 < 1 || sim.alpha() < 0.05) push()
  })
  // Always paint first frame.
  push()

  localCleanup = () => {
    sim.stop()
  }
}

let ro: ResizeObserver | null = null

onMounted(() => {
  startLocalSim()
  paint()
  ro = new ResizeObserver(() => paint())
  if (host.value) ro.observe(host.value)
})

onUnmounted(() => {
  ro?.disconnect()
  stopLocalSim()
})

watch(
  () => [props.points, props.transform, props.viewWidth, props.viewHeight, props.size] as const,
  () => {
    if (props.points && props.points.length) {
      stopLocalSim()
      paint()
    } else {
      paint()
    }
  },
  { deep: true },
)

watch(
  () => [graph.nodes, graph.edges, graph.loading] as const,
  () => {
    if (props.points && props.points.length) {
      paint()
      return
    }
    startLocalSim()
  },
  { deep: true },
)

// Expose for tests / parent probing.
defineExpose({
  paint,
})
</script>

<template>
  <div
    ref="host"
    class="minimap"
    :style="{ width: size + 'px', height: size + 'px' }"
    title="Overview (click to navigate)"
  />
</template>

<style scoped>
.minimap {
  position: absolute;
  right: 12px;
  bottom: 12px;
  z-index: 5;
  border-radius: 12px;
  background: rgba(8, 12, 22, 0.78);
  border: 1px solid rgba(255, 255, 255, 0.08);
  box-shadow: 0 12px 36px rgba(0, 0, 0, 0.4);
  backdrop-filter: blur(8px);
  overflow: hidden;
  cursor: crosshair;
  /* Keep above canvas; below inspector (top-right) is fine. */
  pointer-events: auto;
}
.minimap :deep(svg) {
  display: block;
  width: 100%;
  height: 100%;
}
</style>
