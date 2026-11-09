import { describe, expect, it } from 'vitest'
import type { AgentInfo, CallRow, OpsLogItem } from '@/api/types'
import {
  SPARK_BUCKET_MS,
  agentKey,
  callStats,
  filterCalls,
  filterOperations,
  payloadText,
  sparkByAgent,
  visibleAgents,
} from './agentsModel'

const agent = (name: string): AgentInfo => ({ agent: name })

function call(agentName: string, elapsed: number, ts = '2026-01-01T00:00:00Z', seq = 0): CallRow {
  return { seq, ts, agent: agentName, tool: 'search', args: '{}', elapsed_ms: elapsed, ok: true }
}

describe('agentKey', () => {
  it('ignores case and surrounding whitespace', () => {
    expect(agentKey('  Claude-Code ')).toBe('claude-code')
    expect(agentKey(null)).toBe('')
    expect(agentKey()).toBe('')
  })
})

describe('visibleAgents', () => {
  const agents = ['a', 'b', 'c', 'd', 'e', 'f'].map(agent)

  it('shows the first four when nothing or a visible agent is selected', () => {
    expect(visibleAgents(agents, '').map((item) => item.agent)).toEqual(['a', 'b', 'c', 'd'])
    expect(visibleAgents(agents, 'c').map((item) => item.agent)).toEqual(['a', 'b', 'c', 'd'])
  })

  it('pulls a selected agent from beyond the cut to the front', () => {
    expect(visibleAgents(agents, 'f').map((item) => item.agent)).toEqual(['f', 'a', 'b', 'c'])
    expect(visibleAgents(agents, 'missing').map((item) => item.agent)).toEqual(['a', 'b', 'c', 'd'])
  })
})

describe('journal filters', () => {
  const operations: OpsLogItem[] = [
    { id: '1', prefix: 'INGEST', agent_name: 'Claude' },
    { id: '2', op: 'wiki', agent_name: 'codex' },
    { id: '3', prefix: 'LINT', agent_name: null },
  ]

  it('filters by category (prefix, then op) and by agent', () => {
    expect(filterOperations(operations, 'all', '').map((item) => item.id)).toEqual(['1', '2', '3'])
    expect(filterOperations(operations, 'WIKI', '').map((item) => item.id)).toEqual(['2'])
    expect(filterOperations(operations, 'all', 'claude').map((item) => item.id)).toEqual(['1'])
    expect(filterOperations(operations, 'LINT', 'claude')).toEqual([])
  })

  it('formats a payload once: pretty JSON, empty for {}, raw text when it is not JSON', () => {
    expect(payloadText('{"path":"/a","count":2}')).toBe('{\n  "path": "/a",\n  "count": 2\n}')
    expect(payloadText('{}')).toBe('')
    expect(payloadText('')).toBe('')
    expect(payloadText(null)).toBe('')
    expect(payloadText('not json')).toBe('not json')
    expect(payloadText('null')).toBe('null')
  })
})

describe('call stream views', () => {
  const calls = [call('Claude', 10), call('codex', 400), call('claude', 30), call('claude', Number.NaN)]

  it('filters calls by agent key', () => {
    expect(filterCalls(calls, '')).toHaveLength(4)
    expect(filterCalls(calls, 'claude')).toHaveLength(3)
  })

  it('summarises latency over the usable durations only', () => {
    expect(callStats(calls)).toEqual({ count: 3, p50: 30, p95: 400 })
    expect(callStats([call('x', Number.NaN), call('x', -1)])).toBeNull()
    expect(callStats([])).toBeNull()
  })
})

describe('sparkByAgent', () => {
  const now = Date.parse('2026-01-01T01:00:00Z')
  const at = (minutesAgo: number) => new Date(now - minutesAgo * 60_000).toISOString()

  it('buckets each known agent over the last hour and ignores unknown agents and old calls', () => {
    const sparks = sparkByAgent(
      [agent('Claude'), agent('idle')],
      [
        call('claude', 1, at(1)),
        call('CLAUDE', 1, at(2)),
        call('claude', 1, at(58)),
        call('claude', 1, at(90)),
        call('stranger', 1, at(1)),
        call('claude', 1, 'not a date'),
      ],
      now,
    )
    expect([...sparks.keys()]).toEqual(['claude', 'idle'])
    const claude = sparks.get('claude') ?? []
    expect(claude).toHaveLength(12)
    expect(claude.map((point) => point.count)).toEqual([1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2])
    expect(claude.map((point) => point.height)).toEqual([7, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 14])
    expect(claude.at(-1)?.label).toMatch(/^\d{2}:\d{2}–\d{2}:\d{2} · 2 вызовов$/)
    expect((sparks.get('idle') ?? []).every((point) => point.count === 0 && point.height === 1)).toBe(true)
  })

  it('uses five-minute buckets', () => {
    expect(SPARK_BUCKET_MS).toBe(300_000)
  })
})
