<script lang="ts">
  import { onMount } from 'svelte'
  import { api, loadPanels } from '@/api/client'

  type AgentRow = {
    agent: string
    transport?: string
    online?: boolean
    last_tool?: string
    calls_today?: number
    diary_count?: number
  }
  type CallRow = {
    seq: number
    ts: string
    agent: string
    tool: string
    args: string
    elapsed_ms: number
    ok: boolean
    result_hint?: string
    error?: string
  }
  type CallStreamState = 'loading' | 'live' | 'empty' | 'stale' | 'error'
  type SparkPoint = { count: number; height: number; label: string }

  const CALL_TIMEOUT_MS = 8_000
  const SPARK_BUCKET_COUNT = 12
  const SPARK_BUCKET_MS = 5 * 60_000

  let agents = $state<AgentRow[]>([])
  let operations = $state<any[]>([])
  let calls = $state<CallRow[]>([])
  let facts = $state<any[]>([])
  let factsTotal = $state(0)
  let busy = $state(false)
  let error = $state('')
  let liveBusy = false
  let callsLoaded = false
  let callsRequestId = 0
  let callsState = $state<CallStreamState>('loading')
  let callsError = $state('')
  let callsUpdatedAt = $state<number | null>(null)
  let opFilter = $state('all')
  let selectedAgent = $state('')
  let selectedOperation = $state<string | null>(null)

  const selectedAgentKey = $derived(agentKey(selectedAgent))
  const filteredOperations = $derived(operations.filter((item) =>
    (opFilter === 'all' || (item.prefix || item.op || '').toUpperCase() === opFilter)
    && (!selectedAgentKey || agentKey(item.agent_name) === selectedAgentKey),
  ))
  const filteredCalls = $derived(calls.filter((item) => !selectedAgentKey || agentKey(item.agent) === selectedAgentKey))
  const visibleAgents = $derived.by(() => {
    const first = agents.slice(0, 4)
    if (!selectedAgentKey || first.some((agent) => agentKey(agent.agent) === selectedAgentKey)) return first
    const selected = agents.find((agent) => agentKey(agent.agent) === selectedAgentKey)
    return selected ? [selected, ...first.filter((agent) => agentKey(agent.agent) !== selectedAgentKey).slice(0, 3)] : first
  })
  const callPanelState = $derived<CallStreamState>(
    callsState === 'live' && filteredCalls.length === 0 ? 'empty' : callsState,
  )
  const visibleCallStats = $derived.by(() => {
    const values = filteredCalls
      .map((item) => Number(item.elapsed_ms))
      .filter((value) => Number.isFinite(value) && value >= 0)
      .sort((a, b) => a - b)
    if (!values.length) return null
    return {
      count: values.length,
      p50: percentile(values, 0.5),
      p95: percentile(values, 0.95),
    }
  })
  const sparkByAgent = $derived.by(() => {
    const now = Date.now()
    const start = now - SPARK_BUCKET_COUNT * SPARK_BUCKET_MS
    const buckets = new Map<string, number[]>()
    for (const agent of agents) buckets.set(agentKey(agent.agent), Array(SPARK_BUCKET_COUNT).fill(0))
    for (const item of calls) {
      const timestamp = Date.parse(item.ts)
      const key = agentKey(item.agent)
      const values = buckets.get(key)
      if (!values || !Number.isFinite(timestamp) || timestamp < start || timestamp > now) continue
      const index = Math.min(SPARK_BUCKET_COUNT - 1, Math.floor((timestamp - start) / SPARK_BUCKET_MS))
      values[index] += 1
    }
    const output = new Map<string, SparkPoint[]>()
    for (const [key, values] of buckets) {
      const maximum = Math.max(0, ...values)
      output.set(key, values.map((count, index) => {
        const from = new Date(start + index * SPARK_BUCKET_MS).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
        const to = new Date(start + (index + 1) * SPARK_BUCKET_MS).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
        return {
          count,
          height: maximum ? (count ? Math.max(3, Math.round(count / maximum * 14)) : 1) : 1,
          label: `${from}–${to} · ${count} вызовов`,
        }
      }))
    }
    return output
  })

  function agentKey(value?: string | null) { return value?.trim().toLocaleLowerCase() ?? '' }
  function percentile(sorted: number[], quantile: number) {
    const index = Math.round((sorted.length - 1) * quantile)
    return sorted[Math.min(index, sorted.length - 1)]
  }
  async function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined
    try {
      return await Promise.race([
        promise,
        new Promise<T>((_resolve, reject) => {
          timer = setTimeout(() => reject(new Error(`Timeout after ${timeoutMs} ms`)), timeoutMs)
        }),
      ])
    } finally {
      if (timer) clearTimeout(timer)
    }
  }

  async function fetchCalls() {
    const requestId = ++callsRequestId
    if (!callsLoaded) callsState = 'loading'
    try {
      const response = await withTimeout(api.get<{ items?: CallRow[] }>('/v1/calls?limit=100'), CALL_TIMEOUT_MS)
      if (requestId !== callsRequestId) return
      calls = response.items ?? []
      callsLoaded = true
      callsError = ''
      callsUpdatedAt = Date.now()
      callsState = calls.length ? 'live' : 'empty'
    } catch (cause) {
      if (requestId !== callsRequestId) return
      callsError = cause instanceof Error ? cause.message : String(cause)
      callsState = callsLoaded ? 'stale' : 'error'
      throw cause
    }
  }

  async function loadAgents() {
    const response = await api.get<{ items?: AgentRow[] }>('/v1/agents')
    agents = response.items ?? []
    if (selectedAgentKey && !agents.some((agent) => agentKey(agent.agent) === selectedAgentKey)) {
      selectedAgent = ''
      selectedOperation = null
    }
  }

  async function load() {
    busy = true
    error = ''
    const failures = await loadPanels([
      ['агенты', loadAgents],
      ['ops_log', async () => { operations = (await api.get<any>('/v1/ops-log?limit=100')).items ?? [] }],
      ['MCP-вызовы', fetchCalls],
      ['KG', async () => {
        const response = await api.get<any>('/v1/kg')
        const items = response.items ?? response.facts ?? []
        factsTotal = response.count ?? response.total ?? items.length
        facts = items.slice(0, 80)
      }],
    ])
    const pageFailures = failures.filter((label) => label !== 'MCP-вызовы')
    error = pageFailures.length ? `Не обновлены панели: ${pageFailures.join(', ')}` : ''
    busy = false
  }

  async function refreshCalls() {
    if (liveBusy) return
    liveBusy = true
    try {
      await fetchCalls()
    } catch {
      // fetchCalls retains the last valid snapshot and exposes stale/error itself.
    } finally {
      liveBusy = false
    }
  }

  function selectAgent(name: string) {
    selectedAgent = agentKey(selectedAgent) === agentKey(name) ? '' : name
    selectedOperation = null
  }
  function selectOpFilter(filter: string) {
    opFilter = filter
    selectedOperation = null
  }
  function initials(name: string) {
    const parts = name.trim().split(/[\s_-]+/).filter(Boolean)
    if (!parts.length) return '?'
    return (parts.length === 1 ? parts[0].slice(0, 2) : parts.slice(0, 2).map((part) => part[0]).join('')).toUpperCase()
  }
  function time(value?: string) { return value ? new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false }) : '—' }
  function payload(item: any) { try { const parsed = JSON.parse(item.payload_json || '{}'); return Object.keys(parsed).length ? JSON.stringify(parsed, null, 2) : '' } catch { return item.payload_json || '' } }
  function streamLabel(state: CallStreamState) { return ({ loading: 'loading', live: 'live', empty: 'empty', stale: 'stale', error: 'error' } as const)[state] }
  function streamTitle() {
    if (callsError) return callsUpdatedAt ? `${callsError} · последний успешный снимок ${time(new Date(callsUpdatedAt).toISOString())}` : callsError
    if (callsUpdatedAt) return `Обновлено ${time(new Date(callsUpdatedAt).toISOString())}`
    return 'Ожидание первого снимка'
  }
  function emptyCallsText(state: CallStreamState) {
    if (state === 'loading') return 'Подключение к потоку MCP…'
    if (state === 'error') return 'Поток MCP недоступен'
    if (state === 'stale') return 'Последний снимок потока устарел'
    if (selectedAgent) return `В снимке нет вызовов агента ${selectedAgent}`
    return 'Поток доступен, вызовов пока нет'
  }
  function emptyMetricsText(state: CallStreamState) {
    if (state === 'loading') return 'Метрики появятся после загрузки'
    if (state === 'error') return 'Метрики недоступны'
    if (state === 'stale') return 'В устаревшем снимке нет видимых вызовов'
    return 'Нет видимых вызовов'
  }
  onMount(() => {
    void load()
    const timer = setInterval(() => void refreshCalls(), 5_000)
    return () => clearInterval(timer)
  })
</script>

<div class="agents-page screen">
  <div class="screen-head"><div><h1>Агенты · Журнал</h1><p>Источники активности, мутации и последние MCP-вызовы</p></div><button class="secondary" onclick={load} disabled={busy}>Обновить</button></div>
  {#if error}<div class="notice">{error}</div>{/if}

  <section class="agent-cards">
    {#each visibleAgents as agent (agent.agent)}
      <button class="agent-card" class:selected={agentKey(selectedAgent) === agentKey(agent.agent)} aria-pressed={agentKey(selectedAgent) === agentKey(agent.agent)} onclick={() => selectAgent(agent.agent)}>
        <span class="avatar">{initials(agent.agent)}</span><span class="agent-copy"><b>{agent.agent}</b><small>{agent.transport ?? 'transport неизвестен'} · {agent.last_tool ?? 'нет вызовов'}</small><em>{agent.calls_today ?? 0} вызовов сегодня · дневник {agent.diary_count ?? 0}</em></span><i class:online={agent.online}></i>
        <span class="spark" aria-label={`Активность ${agent.agent} за 60 минут`}>{#each sparkByAgent.get(agentKey(agent.agent)) ?? [] as point}<i class:empty={point.count === 0} style={`height:${point.height}px`} title={point.label}></i>{/each}</span>
      </button>
    {:else}<div class="agent-empty">Нет известных агентов</div>{/each}
  </section>

  <section class="agent-grid">
    <article class="panel ops">
      <header><div><strong>ops_log · мутации</strong><nav>{#each ['all','INGEST','WIKI','LINT'] as filter}<button class:active={opFilter === filter} aria-pressed={opFilter === filter} onclick={() => selectOpFilter(filter)}>{filter}</button>{/each}</nav></div><span>{filteredOperations.length}</span></header>
      <div class="scroll">
        {#each filteredOperations as item (item.id)}
          <button class="op" class:expanded={selectedOperation === item.id} onclick={() => selectedOperation = selectedOperation === item.id ? null : item.id}>
            <time>{time(item.ts)}</time><b class={`badge ${(item.prefix || item.op || '').toLowerCase()}`}>{item.prefix || item.op}</b><span><strong>{item.agent_name || 'system'}</strong><p>{item.message}</p>{#if selectedOperation === item.id && payload(item)}<pre>{payload(item)}</pre>{/if}</span>
          </button>
        {/each}
        {#if !filteredOperations.length}<div class="empty">Журнал пока пуст</div>{/if}
      </div><footer>append-only · только мутации · показано {filteredOperations.length} среди последних {operations.length} глобальных записей</footer>
    </article>

    <article class="panel calls">
      <header><div><strong>Поток MCP-вызовов</strong><b class="proposal">предложение</b><i class={`stream-dot ${callPanelState}`}></i></div><span class={`stream-status ${callPanelState}`} title={streamTitle()}>{streamLabel(callPanelState)}</span></header>
      <div class="call-head"><span>время</span><span>агент</span><span>инструмент · аргументы</span><span>мс</span><span>результат</span></div>
      <div class="scroll">
        {#each filteredCalls as item (item.seq)}<div class="call"><time>{time(item.ts)}</time><b>{item.agent}</b><span><strong>{item.tool}</strong><small>{item.args}</small></span><em class:slow={item.elapsed_ms > 300}>{Math.round(item.elapsed_ms)}</em><i class:bad={!item.ok}>{item.ok ? (item.result_hint || 'ok') : (item.error || 'error')}</i></div>{/each}
        {#if !filteredCalls.length}<div class={`empty call-empty ${callPanelState}`}><strong>{emptyCallsText(callPanelState)}</strong>{#if callsError && (callPanelState === 'error' || callPanelState === 'stale')}<small>{callsError}</small>{/if}</div>{/if}
      </div><footer class="call-footer">{#if visibleCallStats}<span>p50 {Math.round(visibleCallStats.p50)} мс · p95 {Math.round(visibleCallStats.p95)} мс · {visibleCallStats.count} видимых вызовов{selectedAgent ? ` · ${selectedAgent}` : ''}</span>{:else}<span>{emptyMetricsText(callPanelState)}</span>{/if}<em>read-поток — предложение; в append-only ops_log не персистится</em></footer>
    </article>

    <article class="panel kg">
      <header><strong>KG · факты во времени</strong><span>{facts.length}{factsTotal > facts.length ? ` / ${factsTotal}` : ''}</span></header>
      <div class="scroll">{#each facts as fact (fact.id)}<article class:invalid={fact.status === 'invalidated'}><i class={`state ${fact.status}`}></i><b>{fact.subject}</b><p><strong>{fact.predicate}</strong> → {fact.object}</p><div class="timeline"><i></i><span>{fact.valid_from?.slice(0,10) ?? '∞'}</span><span>{fact.valid_to?.slice(0,10) ?? '∞'}</span></div><small>{fact.status} · conf {fact.confidence ?? '—'}</small></article>{/each}{#if !facts.length}<div class="empty">Нет фактов для текущего среза</div>{/if}</div>
      <footer>Временная модель · active / superseded / invalidated</footer>
    </article>
  </section>
</div>

<style>
  .agents-page {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .agent-cards {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 10px;
    margin-bottom: 12px;
    flex: 0 0 auto;
  }

  .agent-card {
    position: relative;
    min-width: 0;
    height: 88px;
    padding: 11px 12px;
    border: 1px solid var(--border);
    border-radius: 11px;
    background: var(--surface);
    display: grid;
    grid-template-columns: 34px 1fr 8px;
    gap: 9px;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }

  .agent-card:hover,
  .agent-card.selected {
    border-color: color-mix(in srgb, var(--l4) 55%, var(--border));
  }

  .avatar {
    width: 34px;
    height: 34px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: color-mix(in srgb, var(--l4) 18%, var(--surface-2));
    color: var(--l4);
    font: 700 11px var(--mono);
  }

  .agent-copy {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .agent-copy b {
    font-size: 12px;
    line-height: 1.15;
  }

  .agent-copy small,
  .agent-copy em {
    margin-top: 2px;
    overflow: hidden;
    color: var(--text-faint);
    font: 11px/1.2 var(--mono);
    font-style: normal;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .agent-card > i {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--text-faint);
  }

  .agent-card > i.online {
    background: var(--ok);
    box-shadow: 0 0 7px var(--ok);
  }

  .agent-empty {
    grid-column: 1 / -1;
    height: 88px;
    display: grid;
    place-items: center;
    border: 1px dashed var(--border);
    border-radius: 11px;
    color: var(--text-faint);
    font-size: 11px;
  }

  .spark {
    position: absolute;
    right: 14px;
    bottom: 7px;
    left: 56px;
    height: 14px;
    display: flex;
    align-items: end;
    gap: 2px;
  }

  .spark i {
    flex: 1;
    max-width: 8px;
    border-radius: 1px;
    background: color-mix(in srgb, var(--l4) 35%, transparent);
  }

  .spark i.empty {
    opacity: .22;
  }

  .agent-grid {
    display: grid;
    grid-template-columns: minmax(370px, 1fr) minmax(420px, 1fr) 320px;
    gap: 12px;
    flex: 1;
    min-height: 0;
  }

  .agent-grid > .panel {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .panel > header {
    height: 42px;
    flex: 0 0 42px;
    padding: 0 12px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .panel > header > div {
    display: flex;
    align-items: center;
    gap: 7px;
  }

  .panel header strong {
    font-size: 12px;
  }

  .panel header span {
    color: var(--text-faint);
    font: 11px var(--mono);
  }

  .panel nav {
    display: flex;
    gap: 3px;
  }

  .panel nav button {
    padding: 3px 5px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--text-faint);
    font: 10px var(--mono);
    cursor: pointer;
  }

  .panel nav button.active {
    background: color-mix(in srgb, var(--l4) 12%, transparent);
    color: var(--l4);
  }

  .scroll {
    min-height: 0;
    overflow: auto;
    flex: 1;
  }

  .panel > footer {
    padding: 8px 11px;
    border-top: 1px solid var(--border);
    color: var(--text-faint);
    font: 11px/1.25 var(--mono);
  }

  .call-footer {
    display: grid;
    gap: 3px;
  }

  .call-footer em {
    color: var(--l4);
    font-style: normal;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .op {
    width: 100%;
    display: grid;
    grid-template-columns: 46px 58px 1fr;
    gap: 7px;
    padding: 9px 10px;
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }

  .op:hover,
  .op.expanded {
    background: var(--bg-hover);
  }

  .op time {
    color: var(--text-faint);
    font: 10px var(--mono);
  }

  .badge {
    justify-self: start;
    padding: 3px 5px;
    border-radius: 4px;
    background: color-mix(in srgb, var(--l0) 12%, transparent);
    color: var(--l0);
    font: 10px var(--mono);
  }

  .badge.wiki {
    color: var(--l3);
  }

  .badge.lint {
    color: var(--warn);
  }

  .op span > strong {
    font-size: 11px;
  }

  .op p {
    margin: 2px 0;
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.35;
  }

  .op pre {
    margin: 7px 0 0;
    padding: 7px;
    border-radius: 5px;
    background: #090b10;
    color: var(--text-muted);
    font: 11px/1.4 var(--mono);
    white-space: pre-wrap;
  }

  .proposal {
    padding: 3px 6px;
    border: 1px dashed var(--l4);
    border-radius: 4px;
    color: var(--l4);
    font: 10px var(--mono);
  }

  .stream-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--text-faint);
  }

  .stream-dot.live {
    background: var(--ok);
    animation: pulse 1.7s infinite;
  }

  .stream-dot.loading {
    background: var(--warn);
    animation: pulse 1.1s infinite;
  }

  .stream-dot.stale {
    background: var(--warn);
  }

  .stream-dot.error {
    background: var(--danger);
  }

  .stream-status.live {
    color: var(--ok);
  }

  .stream-status.loading,
  .stream-status.stale {
    color: var(--warn);
  }

  .stream-status.error {
    color: var(--danger);
  }

  .call-head,
  .call {
    display: grid;
    grid-template-columns: 44px 74px 1fr 40px 82px;
    gap: 6px;
    align-items: center;
  }

  .call-head {
    height: 29px;
    padding: 0 9px;
    border-bottom: 1px solid var(--border);
    color: var(--text-faint);
    font: 10px var(--mono);
  }

  .call {
    min-height: 45px;
    padding: 6px 9px;
    border-bottom: 1px solid var(--border);
    font: 11px var(--mono);
  }

  .call time,
  .call small {
    color: var(--text-faint);
  }

  .call > span {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .call > span strong,
  .call > span small {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .call > em {
    color: var(--ok);
    font-style: normal;
    text-align: right;
  }

  .call > em.slow {
    color: var(--warn);
  }

  .call > i {
    overflow: hidden;
    color: var(--ok);
    font-style: normal;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .call > i.bad {
    color: var(--danger);
  }

  .kg .scroll > article {
    position: relative;
    padding: 11px 11px 11px 23px;
    border-bottom: 1px solid var(--border);
  }

  .kg article > .state {
    position: absolute;
    top: 15px;
    left: 10px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--ok);
  }

  .kg article > .state.superseded {
    background: var(--warn);
  }

  .kg article > .state.invalidated {
    background: var(--danger);
  }

  .kg article > b {
    font-size: 12px;
  }

  .kg article > p {
    margin: 4px 0 7px;
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.4;
  }

  .kg article > p strong {
    color: var(--l2);
  }

  .kg article > small,
  .timeline {
    color: var(--text-faint);
    font: 10px var(--mono);
  }

  .kg article.invalid {
    opacity: .55;
    text-decoration: line-through;
  }

  .timeline {
    position: relative;
    display: flex;
    justify-content: space-between;
    margin: 7px 0;
  }

  .timeline > i {
    position: absolute;
    top: -3px;
    right: 0;
    left: 0;
    height: 2px;
    background: linear-gradient(90deg, var(--l2), var(--l3));
  }

  .empty {
    padding: 44px 15px;
    color: var(--text-faint);
    font-size: 11px;
    text-align: center;
  }

  .call-empty {
    display: grid;
    gap: 6px;
  }

  .call-empty strong {
    font-size: 11px;
    font-weight: 500;
  }

  .call-empty small {
    display: block;
    max-width: 100%;
    overflow: hidden;
    color: var(--danger);
    font: 10px var(--mono);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .call-empty.stale small {
    color: var(--warn);
  }

  .notice {
    margin-bottom: 10px;
    padding: 8px;
    border: 1px solid color-mix(in srgb, var(--danger) 30%, transparent);
    border-radius: 7px;
    color: var(--danger);
    font-size: 11px;
  }

  @keyframes pulse {
    50% { opacity: .25; }
  }

  @media (max-width: 1150px) {
    .agent-grid { grid-template-columns: 1fr 1fr; }
    .kg { display: none; }
    .agent-cards { grid-template-columns: 1fr 1fr; }
  }
</style>
