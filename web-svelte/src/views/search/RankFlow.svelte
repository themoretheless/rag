<script lang="ts">
  import type { SearchHit } from '@/api/types'
  import { score } from '@/lib/format'
  import { flowColor, flowPath, flowY, rankFlow } from './searchModel'

  let {
    hits,
    selected,
    onselect,
    mode,
    rrfK,
    onopen,
  }: {
    hits: SearchHit[]
    /** Index of the highlighted hit in `hits`. */
    selected: number
    onselect: (index: number) => void
    mode: string
    /** `rrf_k` the shown ranking was fused with. */
    rrfK: number
    /** Open the highlighted hit's document in the corpus. */
    onopen: (hit: SearchHit) => void
  } = $props()

  const flow = $derived(rankFlow(hits))
  const selectedHit = $derived(hits[selected] ?? null)
  const hybrid = $derived(mode === 'hybrid')
</script>

<section class="panel fusion">
  <header>
    <strong>{hybrid ? 'Слияние рангов · RRF' : 'Поток рангов'}</strong>
    <span>{hybrid ? `Σ 1 / (${rrfK} + rank)` : `${mode} · без fusion`}</span>
  </header>
  <div class="rank-flow">
    <svg class="rank-links" viewBox="0 0 1000 289" preserveAspectRatio="none" aria-hidden="true">
      {#each flow.connections as connection (connection.hit.chunk_id)}
        {@const color = `--flow-color:${flowColor(connection.fusedIndex)}`}
        {#if connection.lexIndex >= 0}
          <path
            class:selected-path={selected === connection.fusedIndex}
            style={color}
            d={flowPath(300, flowY(connection.lexIndex), 360, flowY(connection.fusedIndex))}
          />
          <circle style={color} cx="300" cy={flowY(connection.lexIndex)} r="2.4" />
          <circle style={color} cx="360" cy={flowY(connection.fusedIndex)} r="2.4" />
        {/if}
        {#if connection.vecIndex >= 0}
          <path
            class:selected-path={selected === connection.fusedIndex}
            style={color}
            d={flowPath(700, flowY(connection.vecIndex), 640, flowY(connection.fusedIndex))}
          />
          <circle style={color} cx="700" cy={flowY(connection.vecIndex)} r="2.4" />
          <circle style={color} cx="640" cy={flowY(connection.fusedIndex)} r="2.4" />
        {/if}
      {/each}
    </svg>

    <div class="rank-column">
      <b>lex · bm25</b>
      {#each flow.lex as { hit, fusedIndex } (hit.chunk_id)}
        <button
          class:selected={selected === fusedIndex}
          style={`--rank-color:${flowColor(fusedIndex)}`}
          title={`lex #${hit.rank_lex} → fused #${fusedIndex + 1}`}
          onclick={() => onselect(fusedIndex)}
        ><em>#{hit.rank_lex}</em><span>{hit.document_title}</span><strong>{score(hit.score_lex)}</strong></button>
      {/each}
    </div>
    <span class="flow-lane" aria-hidden="true">→</span>
    <div class="rank-column hybrid">
      <b>{hybrid ? 'hybrid · rrf' : `${mode} · result`}</b>
      {#each flow.fused as { hit, fusedIndex } (hit.chunk_id)}
        <button
          class:selected={selected === fusedIndex}
          style={`--rank-color:${flowColor(fusedIndex)}`}
          onclick={() => onselect(fusedIndex)}
        ><em>#{fusedIndex + 1}</em><span>{hit.document_title}</span><strong>{score(hit.score_rrf ?? hit.score)}</strong></button>
      {/each}
    </div>
    <span class="flow-lane" aria-hidden="true">←</span>
    <div class="rank-column">
      <b>vec · cosine</b>
      {#each flow.vec as { hit, fusedIndex } (hit.chunk_id)}
        <button
          class:selected={selected === fusedIndex}
          style={`--rank-color:${flowColor(fusedIndex)}`}
          title={`vec #${hit.rank_vec} → fused #${fusedIndex + 1}`}
          onclick={() => onselect(fusedIndex)}
        ><em>#{hit.rank_vec}</em><span>{hit.document_title}</span><strong>{score(hit.score_vec)}</strong></button>
      {/each}
    </div>
  </div>
  {#if selectedHit}<div class="fusion-note"><strong>{selectedHit.document_title}</strong><p>{selectedHit.explanation?.reasons?.join(' · ') || 'Результат ранжирован выбранным режимом.'}</p><dl><dt>chunk / offset</dt><dd>#{selectedHit.chunk_index} · {selectedHit.char_start??'—'}–{selectedHit.char_end??'—'}</dd><dt>dedup</dt><dd>{selectedHit.explanation?.deduplication??'нет'}</dd></dl><button class="secondary" onclick={()=>onopen(selectedHit)}>Открыть в корпусе</button></div>{:else}<div class="empty small">Здесь появится поток lex → hybrid ← vec</div>{/if}
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

  .fusion {
    display: flex;
    flex-direction: column;
  }

  .rank-flow {
    position: relative;
    display: grid;
    grid-template-columns: minmax(0, 1fr) 30px minmax(0, 1fr) 30px minmax(0, 1fr);
    flex: 0 0 289px;
    min-height: 289px;
    gap: 0;
    padding: 10px;
    overflow: hidden;
  }

  .rank-links {
    position: absolute;
    z-index: 0;
    inset: 0;
    width: 100%;
    height: 100%;
    pointer-events: none;
  }

  .rank-links path {
    fill: none;
    stroke: var(--flow-color);
    stroke-width: 1.15;
    opacity: 0.42;
    vector-effect: non-scaling-stroke;
  }

  .rank-links path.selected-path {
    stroke-width: 2.4;
    opacity: 1;
  }

  .rank-links circle {
    fill: var(--flow-color);
    opacity: 0.78;
    vector-effect: non-scaling-stroke;
  }

  .flow-lane {
    position: relative;
    z-index: 1;
    padding-top: 1px;
    color: var(--text-faint);
    font: 9px var(--mono);
    text-align: center;
  }

  .rank-column {
    position: relative;
    z-index: 1;
    min-width: 0;
  }

  .rank-column > b {
    display: block;
    margin-bottom: 6px;
    color: var(--text-faint);
    font: 8px var(--mono);
  }

  .rank-flow button {
    width: 100%;
    height: 31px;
    display: grid;
    grid-template-columns: 20px 1fr 38px;
    align-items: center;
    gap: 4px;
    border: 0;
    border-bottom: 1px solid var(--border);
    background: transparent;
    box-shadow: inset 2px 0 color-mix(in srgb, var(--rank-color) 46%, transparent);
    color: var(--text);
    text-align: left;
    cursor: pointer;
  }

  .rank-flow button:hover,
  .rank-flow button.selected {
    background: var(--bg-hover);
  }

  .rank-flow button.selected {
    box-shadow: inset 2px 0 var(--rank-color);
  }

  .rank-flow button em,
  .rank-flow button strong {
    color: var(--text-faint);
    font: 7px var(--mono);
    font-style: normal;
  }

  .rank-flow button span {
    overflow: hidden;
    font-size: 8px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .rank-flow .hybrid button strong {
    color: var(--l1);
  }

  .fusion-note {
    margin: auto 10px 10px;
    padding: 10px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in srgb, var(--l1) 5%, transparent);
  }

  .fusion-note > strong {
    font-size: 10px;
  }

  .fusion-note p {
    margin: 5px 0;
    color: var(--text-muted);
    font-size: 8px;
    line-height: 1.45;
  }

  .fusion-note dl {
    display: grid;
    grid-template-columns: 80px 1fr;
    gap: 4px;
    margin: 7px 0;
    font-size: 8px;
  }

  .fusion-note dt {
    color: var(--text-faint);
  }

  .fusion-note dd {
    margin: 0;
    font-family: var(--mono);
  }

  .fusion-note button {
    margin: 0;
  }

  .empty {
    padding: 54px 24px;
    color: var(--text-faint);
    font-size: 11px;
    text-align: center;
  }

  .empty.small {
    padding: 36px 16px;
  }

  .panel header span { font-size: 10px; }
  .rank-column > b { font-size: 10px; }
  .rank-flow button em,
  .rank-flow button strong { font-size: 9px; }
  .rank-flow button span { font-size: 10px; }
  .fusion-note > strong { font-size: 11.5px; }
  .fusion-note p,
  .fusion-note dl { font-size: 10px; }
</style>
