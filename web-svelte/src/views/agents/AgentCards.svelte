<script lang="ts">
  import type { AgentInfo } from '@/api/types'
  import { initials } from '@/lib/format'
  import { agentKey, type SparkPoint } from './agentsModel'

  let {
    agents,
    selectedKey,
    sparks,
    onselect,
  }: {
    /** Cards to show, already limited to the visible few. */
    agents: AgentInfo[]
    /** `agentKey` of the selected agent, '' when none. */
    selectedKey: string
    /** Activity sparkline per `agentKey`. */
    sparks: Map<string, SparkPoint[]>
    onselect: (name: string) => void
  } = $props()
</script>

<section class="agent-cards">
  {#each agents as agent (agent.agent)}
    {@const key = agentKey(agent.agent)}
    <button class="agent-card" class:selected={selectedKey === key} aria-pressed={selectedKey === key} onclick={() => onselect(agent.agent)}>
      <span class="avatar">{initials(agent.agent)}</span><span class="agent-copy"><b>{agent.agent}</b><small>{agent.transport ?? 'transport неизвестен'} · {agent.last_tool ?? 'нет вызовов'}</small><em>{agent.calls_today ?? 0} вызовов сегодня · дневник {agent.diary_count ?? 0}</em></span><i class:online={agent.online}></i>
      <span class="spark" aria-label={`Активность ${agent.agent} за 60 минут`}>{#each sparks.get(key) ?? [] as point}<i class:empty={point.count === 0} style={`height:${point.height}px`} title={point.label}></i>{/each}</span>
    </button>
  {:else}<div class="agent-empty">Нет известных агентов</div>{/each}
</section>

<style>
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

  @media (max-width: 1150px) {
    .agent-cards { grid-template-columns: 1fr 1fr; }
  }
</style>
