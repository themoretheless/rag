<script lang="ts">
  import type { CallRow } from '@/api/types'
  import { time } from '@/lib/format'
  import type { CallStats } from './agentsModel'
  import type { CallStreamState } from './callStream.svelte'

  let {
    calls,
    stats,
    streamState,
    error,
    updatedAt,
    selectedAgent,
  }: {
    /** Calls after the agent filter. */
    calls: CallRow[]
    /** Latency summary of `calls`, null when there is nothing to measure. */
    stats: CallStats | null
    streamState: CallStreamState
    /** Message of the last failed refresh. */
    error: string
    /** Time of the last successful refresh (ms since epoch). */
    updatedAt: number | null
    /** Name of the agent the stream is filtered to, '' when unfiltered. */
    selectedAgent: string
  } = $props()

  /** A healthy stream whose filtered view is empty reads as `empty`, not `live`. */
  const panelState = $derived<CallStreamState>(streamState === 'live' && calls.length === 0 ? 'empty' : streamState)

  const statusTitle = $derived.by(() => {
    const at = updatedAt === null ? null : time(new Date(updatedAt).toISOString())
    if (error) return at ? `${error} · последний успешный снимок ${at}` : error
    if (at) return `Обновлено ${at}`
    return 'Ожидание первого снимка'
  })

  const emptyText = $derived.by(() => {
    if (panelState === 'loading') return 'Подключение к потоку MCP…'
    if (panelState === 'error') return 'Поток MCP недоступен'
    if (panelState === 'stale') return 'Последний снимок потока устарел'
    if (selectedAgent) return `В снимке нет вызовов агента ${selectedAgent}`
    return 'Поток доступен, вызовов пока нет'
  })

  const emptyMetricsText = $derived.by(() => {
    if (panelState === 'loading') return 'Метрики появятся после загрузки'
    if (panelState === 'error') return 'Метрики недоступны'
    if (panelState === 'stale') return 'В устаревшем снимке нет видимых вызовов'
    return 'Нет видимых вызовов'
  })
</script>

<article class="panel calls">
  <header><div><strong>Поток MCP-вызовов</strong><b class="proposal">предложение</b><i class={`stream-dot ${panelState}`}></i></div><span class={`stream-status ${panelState}`} title={statusTitle}>{panelState}</span></header>
  <div class="call-head"><span>время</span><span>агент</span><span>инструмент · аргументы</span><span>мс</span><span>результат</span></div>
  <div class="scroll">
    {#each calls as item (item.seq)}<div class="call"><time>{time(item.ts)}</time><b>{item.agent}</b><span><strong>{item.tool}</strong><small>{item.args}</small></span><em class:slow={item.elapsed_ms > 300}>{Math.round(item.elapsed_ms)}</em><i class:bad={!item.ok}>{item.ok ? (item.result_hint || 'ok') : (item.error || 'error')}</i></div>{/each}
    {#if !calls.length}<div class={`empty call-empty ${panelState}`}><strong>{emptyText}</strong>{#if error && (panelState === 'error' || panelState === 'stale')}<small>{error}</small>{/if}</div>{/if}
  </div><footer class="call-footer">{#if stats}<span>p50 {Math.round(stats.p50)} мс · p95 {Math.round(stats.p95)} мс · {stats.count} видимых вызовов{selectedAgent ? ` · ${selectedAgent}` : ''}</span>{:else}<span>{emptyMetricsText}</span>{/if}<em>read-поток — предложение; в append-only ops_log не персистится</em></footer>
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

  @keyframes pulse {
    50% { opacity: .25; }
  }
</style>
