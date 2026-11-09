<script lang="ts">
  import type { SearchHit } from '@/api/types'
  import SearchAssessment from '@/components/SearchAssessment.svelte'
  import { goWiki, navigate, route } from '@/lib/router.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import PackPanel from './search/PackPanel.svelte'
  import RankFlow from './search/RankFlow.svelte'
  import SearchHits from './search/SearchHits.svelte'
  import { SEARCH_MODES, corpusLink } from './search/searchModel'
  import { createSearchSession } from './search/searchSession.svelte'

  const session = createSearchSession()
  const form = session.form

  function select(index: number) {
    session.selected = index
  }

  function openCorpusDocument(hit: SearchHit) {
    navigate(corpusLink(hit, session.resultSnapshot))
  }

  function clearDocumentScope() {
    form.documentId = ''
    if (route.name !== 'search') return
    const params = new URLSearchParams(route.query)
    params.delete('document_id')
    const suffix = params.toString()
    navigate(`/search${suffix ? `?${suffix}` : ''}`, { replace: true })
  }

  async function copyContext() {
    if (!session.packed?.context_text) return
    await navigator.clipboard.writeText(session.packed.context_text)
    ui.toast('Контекст скопирован', 'ok')
  }

  async function createWikiDraft() {
    const documentId = await session.createWikiDraft()
    if (documentId === undefined) return
    if (documentId === null) {
      ui.toast('Не удалось создать wiki-черновик', 'error')
      return
    }
    ui.toast('Wiki-черновик создан', 'ok')
    goWiki(documentId)
  }

  let appliedRoute = ''
  $effect(() => {
    const signature = route.query.toString()
    if (route.name !== 'search' || signature === appliedRoute) return
    appliedRoute = signature
    const routedQuery = route.query.get('q')
    form.documentId = route.query.get('document_id') ?? ''
    if (routedQuery) { form.query = routedQuery; void session.run() }
  })
</script>

<div class="search-lab">
  <form class="query-bar" onsubmit={(event) => { event.preventDefault(); void session.run() }}>
    <div class="modes" aria-label="Режим поиска">
      {#each SEARCH_MODES as value}
        <button type="button" class:active={form.mode === value} onclick={() => form.mode = value}>{value}</button>
      {/each}
    </div>
    <input bind:value={form.query} aria-label="Поисковый запрос" placeholder="Спросите корпус — здесь видны ранги, причины и стоимость…" />
    <kbd>↵</kbd>
    <button class="run" type="submit" disabled={session.busy || !form.query.trim()}>{session.busy ? 'Ищу…' : 'Выполнить search'}</button>

  </form>

  <div class="filters">
    {#if form.documentId}<button class="document-scope" type="button" title={`Поиск ограничен документом ${form.documentId}`} onclick={clearDocumentScope}>doc · {form.documentId.slice(0, 8)}… <span>×</span></button>{/if}
    <label>wing<input bind:value={form.wing} placeholder="все" /></label>
    <label>room<input bind:value={form.room} placeholder="все" /></label>
    <label>layer<select bind:value={form.layer}><option value="">все</option><option>raw</option><option>wiki</option><option>diary</option></select></label>
    <label>top_k<input type="number" min="1" max="100" bind:value={form.topK} /></label>
    <label>min_score<input type="number" min="0" step="0.01" bind:value={form.minScore} /></label>
    <label>rrf_k<input type="number" min="1" bind:value={form.rrfK} /></label>
    <label>max/doc<input type="number" min="1" bind:value={form.maxPerDocument} /></label>
    <label>neighbors<input type="number" min="0" max="20" bind:value={form.neighborChunks} /></label>
    <label>tokens<input type="number" min="100" bind:value={form.maxTokens} /></label>
    <label>recency<input type="number" min="0" bind:value={form.recencyDays} /><span>дн</span></label>
    <label>timeout<input type="number" min="100" bind:value={form.timeoutMs} /><span>мс</span></label>
  </div>

  {#if session.error}<div class="error-banner">{session.error}</div>{/if}

  {#if session.executedRequest}
    <SearchAssessment disabled={session.busy || session.resultsDirty} hits={session.hits} search={session.executedRequest} />
  {/if}

  <div class="workspace">
    <SearchHits
      hits={session.hits}
      selected={session.selected}
      onselect={select}
      mode={session.mode}
      timing={session.timing}
      busy={session.busy}
      stale={session.resultsDirty}
    />

    <RankFlow
      hits={session.hits}
      selected={session.selected}
      onselect={select}
      mode={session.mode}
      rrfK={session.appliedRrfK}
      onopen={openCorpusDocument}
    />

    <aside class="analysis">
      <PackPanel
        packed={session.packed}
        sourceCount={session.packedSourceCount}
        hitCount={session.hits.length}
        packing={session.packing}
        creatingWiki={session.creatingWiki}
        resultsDirty={session.resultsDirty}
        packedDirty={session.packedDirty}
        onpack={() => void session.pack()}
        oncopy={() => void copyContext()}
        oncreate={() => void createWikiDraft()}
      />
    </aside>
  </div>
</div>

<style>
  .search-lab {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    overflow: hidden;
    background: var(--bg);
  }

  .query-bar {
    min-height: 58px;
    padding: 11px 16px;
    display: flex;
    align-items: center;
    gap: 8px;
    border-bottom: 1px solid var(--border);
  }

  .modes {
    height: 34px;
    display: flex;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 8px;
  }

  .modes button {
    padding: 0 11px;
    border: 0;
    border-right: 1px solid var(--border);
    background: var(--surface);
    color: var(--text-faint);
    font: 11px var(--mono);
    cursor: pointer;
  }

  .modes button.active {
    background: color-mix(in srgb, var(--l1) 15%, var(--surface));
    color: var(--l1);
  }

  .query-bar > input {
    height: 34px;
    flex: 1;
    min-width: 180px;
    padding: 0 36px 0 11px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--surface);
  }

  .query-bar > kbd {
    margin-left: -38px;
    margin-right: 8px;
    color: var(--text-faint);
  }

  .query-bar button.run {
    height: 34px;
    padding: 0 12px;
    border-radius: 8px;
    font-size: 11px;
    cursor: pointer;
  }

  .run {
    border: 0;
    background: var(--l1);
    color: #07110f;
    font-weight: 700;
  }

  button:disabled {
    cursor: not-allowed;
    opacity: 0.42;
  }

  .filters {
    min-height: 49px;
    display: flex;
    align-items: center;
    gap: 7px;
    padding: 7px 16px;
    overflow-x: auto;
    border-bottom: 1px solid var(--border);
  }

  .filters label {
    height: 31px;
    display: flex;
    align-items: center;
    gap: 5px;
    padding-left: 8px;
    border: 1px solid var(--border);
    border-radius: 7px;
    color: var(--text-faint);
    font: 9px var(--mono);
    white-space: nowrap;
  }

  .filters input,
  .filters select {
    width: 64px;
    height: 29px;
    padding: 0 6px;
    border: 0;
    background: var(--surface-2);
    font: 10px var(--mono);
  }

  .filters label:nth-child(-n + 3) input {
    width: 84px;
  }

  .filters span {
    padding-right: 6px;
  }

  .filters .document-scope {
    height: 31px;
    padding: 0 8px;
    border: 1px solid color-mix(in srgb, var(--l2) 42%, var(--border));
    border-radius: 7px;
    background: color-mix(in srgb, var(--l2) 10%, var(--surface));
    color: var(--l2);
    font: 10px var(--mono);
    white-space: nowrap;
    cursor: pointer;
  }

  .filters .document-scope span {
    padding: 0 0 0 4px;
  }

  .error-banner {
    padding: 8px 16px;
    border-bottom: 1px solid color-mix(in srgb, var(--danger) 30%, transparent);
    background: color-mix(in srgb, var(--danger) 10%, transparent);
    color: var(--danger);
    font-size: 11px;
  }

  .workspace {
    display: grid;
    grid-template-columns: minmax(390px, 1fr) minmax(360px, 520px) minmax(270px, 330px);
    flex: 1;
    min-height: 0;
    gap: 12px;
    padding: 12px;
  }

  .analysis {
    display: block;
    min-height: 0;
  }

  .filters label { font-size: 10.5px; }
  .filters input,
  .filters select { font-size: 11px; }

  @media (max-width: 1180px) {
    .workspace {
      grid-template-columns: minmax(420px, 1fr) minmax(320px, 0.8fr);
    }

    .analysis {
      display: none;
    }
  }

  @media (max-width: 920px) {
    .workspace {
      grid-template-columns: 1fr;
      overflow: auto;
    }
  }
</style>
