<script lang="ts">
  import { onMount } from 'svelte'
  import { api, loadPanels } from '@/api/client'
  import { agentsResource } from '@/api/resources.svelte'
  import type { AgentInfo, KgFact, OpsLogItem } from '@/api/types'
  import AgentCards from './agents/AgentCards.svelte'
  import CallStream from './agents/CallStream.svelte'
  import KgFacts from './agents/KgFacts.svelte'
  import OpsLogPanel from './agents/OpsLogPanel.svelte'
  import {
    agentKey,
    callStats,
    filterCalls,
    filterOperations,
    sparkByAgent,
    visibleAgents,
  } from './agents/agentsModel'
  import { useCallStream } from './agents/callStream.svelte'

  const OPS_LIMIT = 100
  const KG_LIMIT = 80
  const CALLS_PANEL = 'MCP-вызовы'

  const stream = useCallStream()

  let agents = $state.raw<AgentInfo[]>([])
  let operations = $state.raw<OpsLogItem[]>([])
  let facts = $state.raw<KgFact[]>([])
  let factsTotal = $state(0)
  let busy = $state(false)
  let error = $state('')
  let opFilter = $state('all')
  let selectedAgent = $state('')
  let expandedOperation = $state<string | null>(null)

  const selectedAgentKey = $derived(agentKey(selectedAgent))
  const filteredOperations = $derived(filterOperations(operations, opFilter, selectedAgentKey))
  const filteredCalls = $derived(filterCalls(stream.calls, selectedAgentKey))
  const shownAgents = $derived(visibleAgents(agents, selectedAgentKey))
  const stats = $derived(callStats(filteredCalls))
  const sparks = $derived(sparkByAgent(agents, stream.calls, Date.now()))

  async function loadAgents() {
    agents = await agentsResource.refresh()
    if (selectedAgentKey && !agents.some((agent) => agentKey(agent.agent) === selectedAgentKey)) {
      selectedAgent = ''
      expandedOperation = null
    }
  }

  async function load() {
    busy = true
    error = ''
    const failures = await loadPanels([
      ['агенты', loadAgents],
      ['ops_log', async () => { operations = await api.opsLog(OPS_LIMIT) }],
      [CALLS_PANEL, () => stream.fetch()],
      ['KG', async () => {
        const response = await api.kgFacts(KG_LIMIT)
        factsTotal = response.total
        facts = response.items.slice(0, KG_LIMIT)
      }],
    ])
    // The call stream reports its own stale / error state inside its panel.
    const pageFailures = failures.filter((failure) => failure !== CALLS_PANEL && !failure.startsWith(`${CALLS_PANEL}: `))
    error = pageFailures.length ? `Не обновлены панели: ${pageFailures.join(', ')}` : ''
    busy = false
  }

  function selectAgent(name: string) {
    selectedAgent = selectedAgentKey === agentKey(name) ? '' : name
    expandedOperation = null
  }

  function selectOpFilter(filter: string) {
    opFilter = filter
    expandedOperation = null
  }

  onMount(() => {
    void load()
  })
</script>

<div class="agents-page screen">
  <div class="screen-head"><div><h1>Агенты · Журнал</h1><p>Источники активности, мутации и последние MCP-вызовы</p></div><button class="secondary" onclick={load} disabled={busy}>Обновить</button></div>
  {#if error}<div class="notice">{error}</div>{/if}

  <AgentCards agents={shownAgents} selectedKey={selectedAgentKey} {sparks} onselect={selectAgent} />

  <section class="agent-grid">
    <OpsLogPanel
      operations={filteredOperations}
      total={operations.length}
      filter={opFilter}
      onfilter={selectOpFilter}
      expandedId={expandedOperation}
      onexpand={(id) => expandedOperation = id}
    />
    <CallStream
      calls={filteredCalls}
      {stats}
      streamState={stream.state}
      error={stream.error}
      updatedAt={stream.updatedAt}
      {selectedAgent}
    />
    <KgFacts {facts} total={factsTotal} />
  </section>
</div>

<style>
  .agents-page {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .agent-grid {
    display: grid;
    grid-template-columns: minmax(370px, 1fr) minmax(420px, 1fr) 320px;
    gap: 12px;
    flex: 1;
    min-height: 0;
  }

  .notice {
    margin-bottom: 10px;
    padding: 8px;
    border: 1px solid color-mix(in srgb, var(--danger) 30%, transparent);
    border-radius: 7px;
    color: var(--danger);
    font-size: 11px;
  }

  @media (max-width: 1150px) {
    .agent-grid { grid-template-columns: 1fr 1fr; }
  }
</style>
