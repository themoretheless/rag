/** Small numeric helpers behind the dashboard sparklines and latency footers. */

/** Nearest-rank percentile of an ascending list; 0 for an empty one. */
export function percentile(sorted: readonly number[], quantile: number): number {
  const index = Math.round((sorted.length - 1) * quantile)
  return sorted[Math.min(index, sorted.length - 1)] ?? 0
}

/** Ascending copy of the finite, non-negative values. */
export function sortedFinite(values: readonly number[]): number[] {
  return values.filter((value) => Number.isFinite(value) && value >= 0).sort((a, b) => a - b)
}

export interface BucketWindow {
  /** Right edge of the window (ms since epoch). */
  now: number
  bucketCount: number
  bucketMs: number
}

/** Left edge of the window (ms since epoch). */
export function windowStart({ now, bucketCount, bucketMs }: BucketWindow): number {
  return now - bucketCount * bucketMs
}

/**
 * Index of the bucket a timestamp falls into, or -1 when it is outside the window
 * (or not a finite number). `now` itself belongs to the last bucket.
 */
export function bucketIndex(timestamp: number, window: BucketWindow): number {
  const start = windowStart(window)
  if (!Number.isFinite(timestamp) || timestamp < start || timestamp > window.now) return -1
  return Math.min(window.bucketCount - 1, Math.floor((timestamp - start) / window.bucketMs))
}

/** Histogram of timestamps over the window. */
export function bucketCounts(timestamps: readonly number[], window: BucketWindow): number[] {
  const counts = Array.from({ length: window.bucketCount }, () => 0)
  for (const timestamp of timestamps) {
    const index = bucketIndex(timestamp, window)
    if (index >= 0) counts[index] = (counts[index] ?? 0) + 1
  }
  return counts
}

/**
 * Bar heights (px) for a count histogram: empty buckets are 1px, non-empty ones scale
 * to `maxHeight` relative to the tallest bucket but never drop below 3px.
 */
export function sparkHeights(counts: readonly number[], maxHeight = 14): number[] {
  const maximum = Math.max(0, ...counts)
  return counts.map((count) =>
    maximum && count ? Math.max(3, Math.round((count / maximum) * maxHeight)) : 1,
  )
}

/** SVG polyline `points` for a sparkline of values in `0..max`, with 1px vertical padding. */
export function sparkPolyline(values: readonly number[], width: number, height: number, max: number): string {
  if (!values.length) return ''
  const cap = max > 0 ? max : 1
  const step = values.length > 1 ? width / (values.length - 1) : width
  return values
    .map(
      (v, i) =>
        `${(i * step).toFixed(1)},${(height - (Math.min(v, cap) / cap) * (height - 2) - 1).toFixed(1)}`,
    )
    .join(' ')
}
