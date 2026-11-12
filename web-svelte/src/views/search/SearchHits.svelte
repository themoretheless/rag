<script lang="ts">
  import type { SearchExplanation, SearchHit } from '@/api/types'
  import { milliseconds, score } from '@/lib/format'
  import { timingPercent } from './searchModel'

  let {
    hits,
    selected,
    onselect,
    mode,
    timing,
    busy,
    stale,
  }: {
    hits: SearchHit[]
    /** Index of the highlighted hit. */
    selected: number
    onselect: (index: number) => void
    mode: string
    timing: SearchExplanation | null
    busy: boolean
    /** The form has changed since these results were fetched. */
    stale: boolean
  } = $props()
</script>

<section class="results panel">
  <header>
    <div><strong>Хиты</strong><span>{hits.length}</span><span>{mode}</span>{#if stale}<span class="stale">параметры изменены</span>{/if}</div>
    {#if timing}
      <div class="timing-summary">
        {#if timing.embed_ms != null}
          <span title="embed_ms измеряется до retrieval и не входит в search total">embed {milliseconds(timing.embed_ms)}</span>
        {/if}
        <b title="total_ms из API: retrieval и post-processing, без времени embedding">search {milliseconds(timing.total_ms)}</b>
      </div>
    {/if}
  </header>
  {#if timing}
    <div class="timing" title="Полосы lex, vec и post нормированы к search total; embedding показан отдельно в заголовке">
      <div class="timing-scope"><span>внутри search total</span><b>retrieval до post · {milliseconds(timing.retrieval_ms)}</b></div>
      <div class="timing-stages">
        <span><i style={`--w:${timingPercent(timing.lex_ms, timing.total_ms)}%`}></i>lex <b>{milliseconds(timing.lex_ms)}</b></span>
        <span><i style={`--w:${timingPercent(timing.vec_ms, timing.total_ms)}%`}></i>vec <b>{milliseconds(timing.vec_ms)}</b></span>
        <span><i style={`--w:${timingPercent(timing.postprocess_ms, timing.total_ms)}%`}></i>post <b>{milliseconds(timing.postprocess_ms)}</b></span>
      </div>
    </div>
  {/if}
  <div class="hit-list">
    {#each hits as hit, index (hit.chunk_id)}
      <button class="hit" class:selected={selected === index} onclick={() => onselect(index)}>
        <strong class="rank">{index + 1}</strong>
        <span class="hit-copy">
          <b>{hit.document_title}</b>
          <small>{hit.heading_path?.join(' › ') || hit.section || hit.document_uri}</small>
          <p>{hit.snippet || hit.content}</p>
          <em>{#each hit.explanation?.reasons ?? [] as reason}<span>{reason}</span>{/each}</em>
        </span>
        <span class="scores"><b>{score(hit.score_rrf ?? hit.score)}</b><small>rrf / score</small><em>lex {score(hit.score_lex)} · #{hit.rank_lex ?? '—'}</em><em>vec {score(hit.score_vec)} · #{hit.rank_vec ?? '—'}</em></span>
      </button>
    {/each}
    {#if !hits.length && !busy}<div class="empty">Введите запрос — результаты появятся вместе с объяснением ранжирования</div>{/if}
  </div>
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

  .panel header div {
    display: flex;
    align-items: center;
    gap: 7px;
  }

  .panel header span {
    padding: 3px 7px;
    border-radius: 10px;
    background: var(--bg-hover);
    color: var(--text-faint);
    font: 9px var(--mono);
  }

  .panel header .timing-summary {
    justify-content: flex-end;
    gap: 6px;
  }

  .panel header .timing-summary > span {
    background: transparent;
    border: 1px dashed var(--border-strong);
  }

  .panel header .timing-summary > b {
    color: var(--l1);
    font: 11px var(--mono);
    white-space: nowrap;
  }

  .panel header span.stale {
    border: 1px solid color-mix(in srgb, var(--warn) 44%, var(--border));
    background: color-mix(in srgb, var(--warn) 10%, transparent);
    color: var(--warn);
  }

  .results {
    display: flex;
    flex-direction: column;
  }

  .timing {
    display: flex;
    flex-direction: column;
    gap: 5px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
  }

  .timing-scope {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    color: var(--text-faint);
    font: 8px var(--mono);
  }

  .timing-scope > span {
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .timing-scope > b {
    color: var(--text-muted);
    font-weight: 500;
  }

  .timing-stages {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 7px;
  }

  .timing-stages span {
    position: relative;
    overflow: hidden;
    padding: 7px;
    border-radius: 6px;
    background: #090b10;
    color: var(--text-faint);
    font: 9px var(--mono);
  }

  .timing-stages span i {
    position: absolute;
    bottom: 0;
    left: 0;
    width: var(--w);
    height: 2px;
    background: var(--l1);
  }

  .timing-stages b {
    float: right;
    color: var(--text-muted);
  }

  .hit-list {
    overflow: auto;
  }

  .hit {
    width: 100%;
    display: grid;
    grid-template-columns: 30px 1fr 118px;
    gap: 9px;
    padding: 12px;
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    text-align: left;
    cursor: pointer;
  }

  .hit:hover,
  .hit.selected {
    background: var(--bg-hover);
  }

  .hit.selected {
    box-shadow: inset 2px 0 var(--l1);
  }

  .rank {
    padding-top: 2px;
    color: var(--text-faint);
    font: 13px var(--mono);
  }

  .hit-copy {
    min-width: 0;
  }

  .hit-copy > b {
    color: var(--text);
    font-size: 12px;
  }

  .hit-copy small {
    display: block;
    margin-top: 2px;
    overflow: hidden;
    color: var(--text-faint);
    font: 9px var(--mono);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .hit-copy p {
    display: -webkit-box;
    margin: 7px 0;
    overflow: hidden;
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.5;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 2;
    line-clamp: 2;
  }

  .hit-copy em {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .hit-copy em span {
    padding: 2px 5px;
    border-radius: 4px;
    background: var(--bg-hover);
    color: var(--text-faint);
    font: 8px var(--mono);
  }

  .scores {
    display: flex;
    flex-direction: column;
    text-align: right;
  }

  .scores > b {
    color: var(--l1);
    font: 600 13px var(--mono);
  }

  .scores small,
  .scores em {
    margin-top: 3px;
    color: var(--text-faint);
    font: 8px var(--mono);
    font-style: normal;
  }

  .empty {
    padding: 54px 24px;
    color: var(--text-faint);
    font-size: 11px;
    text-align: center;
  }

  .panel header span { font-size: 10px; }
  .timing-scope { font-size: 9.5px; }
  .timing-stages span { font-size: 10px; }
  .hit-copy > b { font-size: 12.5px; }
  .hit-copy small { font-size: 10px; }
  .hit-copy p { font-size: 11.5px; }
  .hit-copy em span,
  .scores small,
  .scores em { font-size: 9.5px; }

  @media (max-width: 920px) {
    .results {
      min-height: 480px;
    }
  }
</style>
