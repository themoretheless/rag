import { describe, expect, it } from 'vitest'
import {
  bucketCounts,
  bucketIndex,
  percentile,
  sortedFinite,
  sparkHeights,
  sparkPolyline,
  windowStart,
} from './stats'

describe('percentile', () => {
  it('uses nearest rank on a sorted list', () => {
    const values = [10, 20, 30, 40, 50]
    expect(percentile(values, 0.5)).toBe(30)
    expect(percentile(values, 0.95)).toBe(50)
    expect(percentile(values, 0)).toBe(10)
    expect(percentile([7], 0.95)).toBe(7)
    expect(percentile([], 0.5)).toBe(0)
  })

  it('sortedFinite drops NaN, infinities and negatives', () => {
    expect(sortedFinite([5, Number.NaN, -1, 2, Number.POSITIVE_INFINITY, 0])).toEqual([0, 2, 5])
  })
})

describe('spark bucketing', () => {
  const window = { now: 60 * 60_000, bucketCount: 12, bucketMs: 5 * 60_000 }

  it('maps timestamps to buckets with now in the last one', () => {
    expect(windowStart(window)).toBe(0)
    expect(bucketIndex(0, window)).toBe(0)
    expect(bucketIndex(5 * 60_000 - 1, window)).toBe(0)
    expect(bucketIndex(5 * 60_000, window)).toBe(1)
    expect(bucketIndex(window.now, window)).toBe(11)
    expect(bucketIndex(-1, window)).toBe(-1)
    expect(bucketIndex(window.now + 1, window)).toBe(-1)
    expect(bucketIndex(Number.NaN, window)).toBe(-1)
  })

  it('counts only timestamps inside the window', () => {
    const counts = bucketCounts([0, 1, 5 * 60_000, window.now, window.now + 1, Number.NaN], window)
    expect(counts).toHaveLength(12)
    expect(counts[0]).toBe(2)
    expect(counts[1]).toBe(1)
    expect(counts[11]).toBe(1)
    expect(counts.reduce((sum, count) => sum + count, 0)).toBe(4)
  })

  it('scales heights to the tallest bucket with 1px / 3px floors', () => {
    expect(sparkHeights([0, 0])).toEqual([1, 1])
    expect(sparkHeights([0, 1, 10, 5])).toEqual([1, 3, 14, 7])
  })

  it('builds polyline points with 1px vertical padding', () => {
    expect(sparkPolyline([], 100, 20, 10)).toBe('')
    expect(sparkPolyline([5], 100, 20, 10)).toBe('0.0,10.0')
    expect(sparkPolyline([0, 10, 20], 100, 20, 10)).toBe('0.0,19.0 50.0,1.0 100.0,1.0')
    expect(sparkPolyline([1], 100, 20, 0)).toBe('0.0,1.0')
  })
})
