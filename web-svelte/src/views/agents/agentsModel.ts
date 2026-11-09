/** Pure logic of the agents / journal screen. */
import type { AgentInfo, CallRow, OpsLogItem } from '@/api/types'
import { time } from '@/lib/format'
import {
  bucketCounts,
  percentile,
  sortedFinite,
  sparkHeights,
  windowStart,
  type BucketWindow,
} from '@/lib/stats'

/** Case- and whitespace-insensitive identity of an agent name. */
export function agentKey(value?: string | null): string {
  return value?.trim().toLocaleLowerCase() ?? ''
}

/** Agent cards shown at once. */
export const VISIBLE_AGENTS = 4

/**
 * The first few agents, with the selected one pulled to the front when it would
 * otherwise be cut off.
 */
export function visibleAgents(agents: readonly AgentInfo[], selectedKey: string): AgentInfo[] {
  const first = agents.slice(0, VISIBLE_AGENTS)
  if (!selectedKey || first.some((agent) => agentKey(agent.agent) === selectedKey)) return first
  const selected = agents.find((agent) => agentKey(agent.agent) === selectedKey)
  if (!selected) return first
  return [
    selected,
    ...first.filter((agent) => agentKey(agent.agent) !== selectedKey).slice(0, VISIBLE_AGENTS - 1),
  ]
}

/** Prefix filter values of the mutation journal. */
export const OP_FILTERS = ['all', 'INGEST', 'WIKI', 'LINT'] as const

/** Journal category shown on the badge (`prefix`, falling back to `op`). */
export function opCategory(item: Pick<OpsLogItem, 'prefix' | 'op'>): string {
  return item.prefix || item.op || ''
}

export function filterOperations(
  operations: readonly OpsLogItem[],
  opFilter: string,
  selectedKey: string,
): OpsLogItem[] {
  return operations.filter(
    (item) =>
      (opFilter === 'all' || opCategory(item).toUpperCase() === opFilter) &&
      (!selectedKey || agentKey(item.agent_name) === selectedKey),
  )
}

/**
 * Pretty-printed journal payload: '' for an empty object, the raw text when it is
 * not JSON. Parse once per expanded row and reuse the result.
 */
export function payloadText(payloadJson?: string | null): string {
  try {
    const parsed: unknown = JSON.parse(payloadJson || '{}')
    // Object.keys(null) throws, which routes a literal `null` payload to the raw branch.
    return Object.keys(parsed as object).length ? JSON.stringify(parsed, null, 2) : ''
  } catch {
    return payloadJson || ''
  }
}

export function filterCalls(calls: readonly CallRow[], selectedKey: string): CallRow[] {
  return calls.filter((item) => !selectedKey || agentKey(item.agent) === selectedKey)
}

export interface CallStats {
  count: number
  p50: number
  p95: number
}

/** Latency percentiles of the calls with a usable duration; null when there are none. */
export function callStats(calls: readonly CallRow[]): CallStats | null {
  const values = sortedFinite(calls.map((item) => item.elapsed_ms))
  if (!values.length) return null
  return { count: values.length, p50: percentile(values, 0.5), p95: percentile(values, 0.95) }
}

/** Sparkline window: the last hour in 5-minute buckets. */
export const SPARK_BUCKET_COUNT = 12
export const SPARK_BUCKET_MS = 5 * 60_000

export interface SparkPoint {
  count: number
  /** Bar height in px. */
  height: number
  label: string
}

/** Per-agent activity sparkline built from the call stream, keyed by `agentKey`. */
export function sparkByAgent(
  agents: readonly AgentInfo[],
  calls: readonly CallRow[],
  now: number,
): Map<string, SparkPoint[]> {
  const window: BucketWindow = { now, bucketCount: SPARK_BUCKET_COUNT, bucketMs: SPARK_BUCKET_MS }
  const start = windowStart(window)
  const timestamps = new Map<string, number[]>()
  for (const agent of agents) timestamps.set(agentKey(agent.agent), [])
  for (const item of calls) timestamps.get(agentKey(item.agent))?.push(Date.parse(item.ts))

  const clock = (offset: number) => time(new Date(start + offset * SPARK_BUCKET_MS).toISOString())
  const output = new Map<string, SparkPoint[]>()
  for (const [key, stamps] of timestamps) {
    const counts = bucketCounts(stamps, window)
    const heights = sparkHeights(counts)
    output.set(
      key,
      counts.map((count, index) => ({
        count,
        height: heights[index] ?? 1,
        label: `${clock(index)}–${clock(index + 1)} · ${count} вызовов`,
      })),
    )
  }
  return output
}
