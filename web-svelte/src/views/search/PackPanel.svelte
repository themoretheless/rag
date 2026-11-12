<script lang="ts">
  import type { PackContextResponse } from '@/api/types'

  let {
    packed,
    sourceCount,
    hitCount,
    packing,
    creatingWiki,
    resultsDirty,
    packedDirty,
    onpack,
    oncopy,
    oncreate,
  }: {
    packed: PackContextResponse | null
    /** Hits the packed context was built from. */
    sourceCount: number | null
    /** Hits currently shown; packing needs at least one. */
    hitCount: number
    packing: boolean
    creatingWiki: boolean
    /** The form is ahead of the shown results. */
    resultsDirty: boolean
    /** The packed context no longer matches the form or the results. */
    packedDirty: boolean
    onpack: () => void
    oncopy: () => void
    oncreate: () => void
  } = $props()
</script>

<section class="panel pack">
  <header><strong>pack_context → агенту</strong>{#if packed}<span>{packed.total_tokens} / {packed.max_tokens} tok</span>{/if}</header>
  {#if packed}
    <div class="budget"><i style={`width:${Math.min(100, packed.total_tokens / packed.max_tokens * 100)}%`}></i></div>
    <p>{packed.hits.length} из {sourceCount ?? '—'} хитов · исключено {packed.omitted_count}</p>
    {#if packedDirty}<p class="pack-stale">Параметры изменены. Повторите search и заново соберите контекст перед созданием wiki.</p>{/if}
    <pre>{packed.context_text}</pre>
    <div class="pack-actions">
      <button class="secondary" onclick={oncopy}>Скопировать</button>
      <button
        class="primary"
        disabled={creatingWiki || packedDirty}
        title={packedDirty ? 'Сначала повторите search и заново соберите контекст' : 'Создаст новую wiki-страницу и откроет её'}
        onclick={oncreate}
      >{creatingWiki ? 'Создаю…' : 'Создать wiki-черновик'}</button>
    </div>
  {:else}
    <p>Упакуйте ранжированные хиты в тот контекст, который реально увидит агент.</p>
    <button class="primary" disabled={!hitCount || packing || resultsDirty} onclick={onpack}>{packing ? 'Упаковка…' : resultsDirty ? 'Сначала обновить search' : 'Собрать контекст'}</button>
  {/if}
</section>

<style>
  .panel {
    min-height: 0;
    overflow: hidden;
  }

  .panel header {
    height: 42px;
    padding: 0 13px;
    display: flex;
    align-items: center;
    justify-content: space-between;
    border-bottom: 1px solid var(--border);
  }

  .panel header span {
    padding: 3px 7px;
    border-radius: 10px;
    background: var(--bg-hover);
    color: var(--text-faint);
    font: 9px var(--mono);
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.42;
  }

  .pack {
    display: flex;
    flex-direction: column;
    height: 100%;
  }

  .pack > p {
    margin: 12px 13px;
    color: var(--text-muted);
    font-size: 10px;
    line-height: 1.5;
  }

  .pack > p.pack-stale {
    margin-block: 0 10px;
    padding: 8px 9px;
    border: 1px solid color-mix(in srgb, var(--warn) 36%, var(--border));
    border-radius: 7px;
    background: color-mix(in srgb, var(--warn) 8%, transparent);
    color: var(--warn);
  }

  .pack pre {
    flex: 1;
    min-height: 80px;
    margin: 0 12px 10px;
    padding: 10px;
    overflow: auto;
    border-radius: 7px;
    background: #090b10;
    color: var(--text-muted);
    font: 9px/1.5 var(--mono);
    white-space: pre-wrap;
  }

  .pack > button {
    align-self: flex-start;
    margin: 0 12px 12px;
  }

  .pack-actions {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 7px;
    padding: 0 12px 12px;
  }

  .pack-actions button {
    min-width: 0;
    margin: 0;
    padding-inline: 10px;
    white-space: nowrap;
  }

  .budget {
    height: 3px;
    margin: 10px 13px 0;
    background: var(--border);
  }

  .budget i {
    display: block;
    height: 100%;
    background: var(--l1);
  }

  .panel header span { font-size: 10px; }
  .pack > p { font-size: 11.5px; }
  .pack pre { font-size: 10.5px; }
</style>
