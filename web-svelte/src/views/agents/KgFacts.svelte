<script lang="ts">
  import type { KgFact } from '@/api/types'

  let {
    facts,
    total,
  }: {
    /** Facts shown (a bounded slice). */
    facts: KgFact[]
    /** Facts known to the gateway for this slice. */
    total: number
  } = $props()
</script>

<article class="panel kg">
  <header><strong>KG · факты во времени</strong><span>{facts.length}{total > facts.length ? ` / ${total}` : ''}</span></header>
  <div class="scroll">{#each facts as fact (fact.id)}<article class:invalid={fact.status === 'invalidated'}><i class={`state ${fact.status}`}></i><b>{fact.subject}</b><p><strong>{fact.predicate}</strong> → {fact.object}</p><div class="timeline"><i></i><span>{fact.valid_from?.slice(0,10) ?? '∞'}</span><span>{fact.valid_to?.slice(0,10) ?? '∞'}</span></div><small>{fact.status} · conf {fact.confidence ?? '—'}</small></article>{/each}{#if !facts.length}<div class="empty">Нет фактов для текущего среза</div>{/if}</div>
  <footer>Временная модель · active / superseded / invalidated</footer>
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

</style>
