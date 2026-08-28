<script lang="ts" module>
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
</script>

<script lang="ts">
  /**
   * Bottom-right graph overview, synced to the main canvas via
   * `points` (world positions) + `transform` (d3-zoom) props.
   */
  import { select } from 'd3-selection'
  import { graph } from '@/lib/state/graph.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { nodeColor } from '@/lib/forceLayout'

  let {
    points = [],
    transform = { x: 0, y: 0, k: 1 },
    viewWidth = 800,
    viewHeight = 600,
    size = 148,
    onnavigate,
  }: {
    /** World-space node positions from the main canvas. */
    points?: MinimapPoint[]
    /** d3-zoom transform of the main viewport. */
    transform?: MinimapTransform
    /** Main canvas size in CSS/SVG pixels (for viewport rect). */
    viewWidth?: number
    viewHeight?: number
    /** Outer minimap box size (square) in CSS px. */
    size?: number
    /** Request main canvas to center on world (x, y). */
    onnavigate?: (payload: { x: number; y: number }) => void
  } = $props()

  let host: HTMLDivElement | null = $state(null)

  const PAD = 10

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

  function paint() {
    const el = host
    if (!el) return
    el.innerHTML = ''

    const svg = select(el)
      .append('svg')
      .attr('width', size)
      .attr('height', size)
      .attr('viewBox', `0 0 ${size} ${size}`)
      .attr('role', 'img')
      .attr('aria-label', 'Graph minimap')

    if (!points.length) {
      svg
        .append('text')
        .attr('x', size / 2)
        .attr('y', size / 2)
        .attr('text-anchor', 'middle')
        .attr('dominant-baseline', 'middle')
        .attr('fill', '#64748b')
        .attr('font-size', 10)
        .text(graph.loading ? '…' : ui.t('graphEmptyMinimap'))
      return
    }

    const { minX, minY, maxX, maxY } = bboxOf(points)
    const bw = maxX - minX
    const bh = maxY - minY
    const inner = size - PAD * 2
    const scale = Math.min(inner / bw, inner / bh)
    // Center content inside the square.
    const usedW = bw * scale
    const usedH = bh * scale
    const offX = PAD + (inner - usedW) / 2
    const offY = PAD + (inner - usedH) / 2

    const toMini = (x: number, y: number): [number, number] => [
      offX + (x - minX) * scale,
      offY + (y - minY) * scale,
    ]
    const toWorld = (mx: number, my: number): [number, number] => [
      (mx - offX) / scale + minX,
      (my - offY) / scale + minY,
    ]

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

    // Edges for orientation.
    const byId = new Map(points.map((p) => [p.id, p]))
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
    for (const p of points) {
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
    const k = transform.k || 1
    const wx0 = (0 - transform.x) / k
    const wy0 = (0 - transform.y) / k
    const wx1 = (viewWidth - transform.x) / k
    const wy1 = (viewHeight - transform.y) / k
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
      onnavigate?.({ x: wx, y: wy })
    })
  }

  $effect(() => {
    void points
    void transform
    void viewWidth
    void viewHeight
    void size
    void graph.edges
    paint()
  })

  let ro: ResizeObserver | null = null

  $effect(() => {
    if (!host) return
    ro = new ResizeObserver(() => paint())
    ro.observe(host)
    return () => {
      ro?.disconnect()
      ro = null
    }
  })
</script>

<div
  bind:this={host}
  class="minimap"
  style="width: {size}px; height: {size}px"
  title={ui.t('graphMinimapHint')}
></div>

<style>
  .minimap {
    position: absolute;
    right: 12px;
    bottom: 12px;
    z-index: 5;
    border-radius: 12px;
    background: var(--graph-panel-bg);
    border: 1px solid var(--graph-panel-border);
    box-shadow: var(--graph-shadow);
    backdrop-filter: blur(8px);
    overflow: hidden;
    cursor: crosshair;
    pointer-events: auto;
  }
  .minimap :global(svg) {
    display: block;
    width: 100%;
    height: 100%;
  }
</style>
