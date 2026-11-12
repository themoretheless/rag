<script lang="ts">
  import type { OpsLogItem } from '@/api/types'
  import { time } from '@/lib/format'
  import { OP_FILTERS, opCategory, payloadText } from './agentsModel'

  let {
    operations,
    total,
    filter,
    onfilter,
    expandedId,
    onexpand,
  }: {
    /** Journal rows after the category and agent filters. */
    operations: OpsLogItem[]
    /** Rows loaded before filtering. */
    total: number
    filter: string
    onfilter: (filter: string) => void
    /** Row whose payload is open, null when none. */
    expandedId: string | null
    onexpand: (id: string | null) => void
  } = $props()

  /** Payload of the open row, parsed once per expansion instead of once per render pass. */
  const expandedPayload = $derived.by(() => {
    const item = expandedId === null ? undefined : operations.find((candidate) => candidate.id === expandedId)
    return item ? payloadText(item.payload_json) : ''
  })
</script>

<article class="panel ops">
  <header><div><strong>ops_log · мутации</strong><nav>{#each OP_FILTERS as option}<button class:active={filter === option} aria-pressed={filter === option} onclick={() => onfilter(option)}>{option}</button>{/each}</nav></div><span>{operations.length}</span></header>
  <div class="scroll">
    {#each operations as item (item.id)}
      <button class="op" class:expanded={expandedId === item.id} onclick={() => onexpand(expandedId === item.id ? null : item.id)}>
        <time>{time(item.ts)}</time><b class={`badge ${opCategory(item).toLowerCase()}`}>{item.prefix || item.op}</b><span><strong>{item.agent_name || 'system'}</strong><p>{item.message}</p>{#if expandedId === item.id && expandedPayload}<pre>{expandedPayload}</pre>{/if}</span>
      </button>
    {/each}
    {#if !operations.length}<div class="empty">Журнал пока пуст</div>{/if}
  </div><footer>append-only · только мутации · показано {operations.length} среди последних {total} глобальных записей</footer>
</article>

<style>
  .panel {
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

  .empty {
    padding: 44px 15px;
    color: var(--text-faint);
    font-size: 11px;
    text-align: center;
  }
</style>
