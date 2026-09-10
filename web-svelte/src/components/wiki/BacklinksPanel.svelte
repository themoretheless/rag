<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { goWiki } from '@/lib/router.svelte'

  function open(id: string) { goWiki(id) }

</script>

{#if wiki.current}
  <aside class="right">
    <h3>{ui.t('backlinks')}</h3>
    <p class="sub">{ui.t('incomingWikilinks')}</p>
    {#if wiki.backlinks.length}
      <ul>
        {#each wiki.backlinks as b (b.id + b.label)}
          <li>
            <button type="button" onclick={() => open(b.id)}>{b.label}</button>
            {#each b.contexts ?? [] as context}<p class="context">{context}</p>{/each}
          </li>
        {/each}
      </ul>
    {:else}
      <p class="empty">{ui.t('noIncoming')}</p>
    {/if}
    <h3 class="props">{ui.t('properties')}</h3>
    <dl>
      <dt>{ui.t('kind')}</dt>
      <dd>{wiki.current.kind}</dd>
      <dt>{ui.t('layer')}</dt>
      <dd>{wiki.current.layer}</dd>
      <dt>{ui.t('revision')}</dt>
      <dd>r{wiki.current.revision ?? '—'}</dd>
      <dt>{ui.t('updated')}</dt>
      <dd>{wiki.current.updated_at || '—'}</dd>
    </dl>
  </aside>
{/if}

<style>
  .context { font-size:12px; line-height:1.5; color:var(--text-muted); overflow-wrap:anywhere; margin:4px 8px 12px; }
  .right {
    width: var(--right-w);
    flex-shrink: 0;
    border-left: 1px solid var(--border);
    background: var(--bg-sidebar);
    padding: 16px 14px;
    overflow: auto;
  }
  h3 {
    margin: 0 0 4px;
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
  }
  .sub {
    margin: 0 0 10px;
    font-size: 12px;
    color: var(--text-muted);
  }
  ul {
    list-style: none;
    margin: 0 0 20px;
    padding: 0;
  }
  li button {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: transparent;
    padding: 8px 10px;
    border-radius: 8px;
    cursor: pointer;
    color: var(--link);
  }
  li button:hover {
    background: var(--bg-hover);
  }
  .empty {
    color: var(--text-faint);
    font-size: 13px;
  }
  .props {
    margin-top: 8px;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 6px 10px;
    font-size: 12px;
    margin: 8px 0 0;
  }
  dt {
    color: var(--text-faint);
  }
  dd {
    margin: 0;
    word-break: break-all;
  }
</style>
