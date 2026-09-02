<script lang="ts">
  import { api } from '@/api/client'
  import type { PackContextResponse, SearchHit, SearchResponse } from '@/api/types'
  import { goWiki, navigate, route } from '@/lib/router.svelte'
  import { ui } from '@/lib/state/ui.svelte'

  let query = $state('')
  let mode = $state<'lex' | 'vec' | 'hybrid'>('hybrid')
  let wing = $state('')
  let room = $state('')
  let layer = $state('')
  let topK = $state(8)
  let minScore = $state(0)
  let rrfK = $state(60)
  let maxPerDocument = $state(2)
  let neighborChunks = $state(1)
  let maxTokens = $state(2000)
  let recencyDays = $state(30)
  let timeoutMs = $state(5000)
  let documentId = $state('')
  let busy = $state(false)
  let packing = $state(false)
  let creatingWiki = $state(false)
  let error = $state('')
  let response = $state<SearchResponse | null>(null)
  let packed = $state<PackContextResponse | null>(null)
  let selected = $state(0)

  interface SearchSnapshot {
    signature: string
    query: string
    mode: 'lex' | 'vec' | 'hybrid'
    wing: string
    room: string
    layer: string
    topK: number
    minScore: number
    rrfK: number
    maxPerDocument: number
    neighborChunks: number
    maxTokens: number
    recencyDays: number
    timeoutMs: number
    documentId: string
  }

  interface PackedSnapshot {
    search: SearchSnapshot
    sourceHitCount: number
  }

  let resultSnapshot = $state<SearchSnapshot | null>(null)
  let packedSnapshot = $state<PackedSnapshot | null>(null)
  let searchRequestId = 0
  let packRequestId = 0

  const hits = $derived(response?.items ?? [])
  // Response-level timings are the only timing source used here. Per-hit
  // explanations describe the same search, but mixing both levels can make
  // the summary silently change when the selected result changes.
  const timing = $derived(response?.timings ?? null)
  const selectedHit = $derived(hits[selected] ?? null)
  const resultsDirty = $derived(Boolean(resultSnapshot && resultSnapshot.signature !== captureSearchSnapshot().signature))
  const packedDirty = $derived(Boolean(
    packedSnapshot && (
      packedSnapshot.search.signature !== captureSearchSnapshot().signature ||
      packedSnapshot.search.signature !== resultSnapshot?.signature ||
      packedSnapshot.sourceHitCount !== hits.length
    ),
  ))
  const appliedRrfK = $derived(timing?.rrf_k ?? resultSnapshot?.rrfK ?? rrfK)
  const flowHits = $derived(hits.slice(0, 8))
  // Source columns contain the same displayed fused hits, sorted by their real
  // API ranks. That makes every visible path traceable end-to-end.
  const flowLexRanks = $derived([...flowHits].filter((hit) => hit.rank_lex != null).sort((a, b) => (a.rank_lex ?? 999) - (b.rank_lex ?? 999)))
  const flowVecRanks = $derived([...flowHits].filter((hit) => hit.rank_vec != null).sort((a, b) => (a.rank_vec ?? 999) - (b.rank_vec ?? 999)))
  const flowConnections = $derived(flowHits.map((hit, fusedIndex) => ({
    hit,
    fusedIndex,
    lexIndex: flowLexRanks.findIndex((candidate) => sameHit(candidate, hit)),
    vecIndex: flowVecRanks.findIndex((candidate) => sameHit(candidate, hit)),
  })))

  const flowPalette = ['var(--l1)', 'var(--l3)', 'var(--l0)', 'var(--l2)', 'var(--l4)', 'var(--ok)', 'var(--warn)', 'var(--accent)']

  function captureSearchSnapshot(): SearchSnapshot {
    const values = {
      query: query.trim(),
      mode,
      wing: wing.trim(),
      room: room.trim(),
      layer: layer.trim(),
      topK,
      minScore,
      rrfK,
      maxPerDocument,
      neighborChunks,
      maxTokens,
      recencyDays,
      timeoutMs,
      documentId: documentId.trim(),
    }
    return { ...values, signature: JSON.stringify(values) }
  }

  async function run() {
    const snapshot = captureSearchSnapshot()
    if (!snapshot.query) return
    const requestId = ++searchRequestId
    ++packRequestId
    busy = true
    packing = false
    error = ''
    response = null
    resultSnapshot = null
    packed = null
    packedSnapshot = null
    selected = 0
    try {
      const next = await api.post<SearchResponse>('/v1/search', {
        query: snapshot.query,
        mode: snapshot.mode,
        top_k: snapshot.topK,
        wing: snapshot.wing || undefined,
        room: snapshot.room || undefined,
        layer: snapshot.layer || undefined,
        min_score: snapshot.minScore || undefined,
        rrf_k: snapshot.rrfK,
        max_chunks_per_document: snapshot.maxPerDocument,
        context_expansion: snapshot.neighborChunks > 0 ? 'neighbors' : undefined,
        neighbor_chunks: snapshot.neighborChunks,
        max_context_tokens: snapshot.maxTokens,
        recency_half_life_days: snapshot.recencyDays,
        timeout_ms: snapshot.timeoutMs,
        document_id: snapshot.documentId || undefined,
      })
      if (requestId !== searchRequestId) return
      response = next
      resultSnapshot = snapshot
    } catch (cause) {
      if (requestId === searchRequestId) error = cause instanceof Error ? cause.message : String(cause)
    } finally {
      if (requestId === searchRequestId) busy = false
    }
  }

  async function pack() {
    if (!hits.length || !resultSnapshot || resultsDirty) return
    const requestId = ++packRequestId
    const sourceHits = [...hits]
    const search = resultSnapshot
    packing = true
    error = ''
    try {
      const next = await api.post<PackContextResponse>('/v1/pack-context', {
        hits: sourceHits,
        max_tokens: search.maxTokens,
        context_expansion: search.neighborChunks > 0 ? 'neighbors' : undefined,
        neighbor_chunks: search.neighborChunks,
      })
      if (requestId !== packRequestId) return
      packed = next
      packedSnapshot = { search, sourceHitCount: sourceHits.length }
    } catch (cause) {
      if (requestId === packRequestId) error = cause instanceof Error ? cause.message : String(cause)
    } finally {
      if (requestId === packRequestId) packing = false
    }
  }

  function score(value?: number) {
    return value == null ? '—' : value.toFixed(value >= 1 ? 2 : 4)
  }

  function milliseconds(value?: number) {
    return value == null ? '—' : `${Math.round(value)} мс`
  }

  function timingPercent(value?: number, total?: number) {
    if (value == null || total == null || total <= 0) return 0
    return Math.min(100, Math.max(0, value / total * 100))
  }

  function sameHit(left: SearchHit, right: SearchHit) {
    return left.chunk_id === right.chunk_id && left.document_id === right.document_id
  }

  function flowY(index: number) {
    return 40.5 + index * 31
  }

  function flowPath(fromX: number, fromY: number, toX: number, toY: number) {
    const controlX = (fromX + toX) / 2
    return `M ${fromX} ${fromY} C ${controlX} ${fromY}, ${controlX} ${toY}, ${toX} ${toY}`
  }

  function flowColor(index: number) {
    return flowPalette[index % flowPalette.length]
  }

  function openCorpusDocument(hit: SearchHit) {
    const params = new URLSearchParams({
      document_id: hit.document_id,
      q: hit.document_title || hit.document_uri,
    })
    if (resultSnapshot?.wing) params.set('project', resultSnapshot.wing)
    if (resultSnapshot?.room) params.set('room', resultSnapshot.room)
    navigate(`/corpus?${params.toString()}`)
  }

  function clearDocumentScope() {
    documentId = ''
    if (route.name !== 'search') return
    const params = new URLSearchParams(route.query)
    params.delete('document_id')
    const suffix = params.toString()
    navigate(`/search${suffix ? `?${suffix}` : ''}`, { replace: true })
  }

  async function copyContext() {
    if (!packed?.context_text) return
    await navigator.clipboard.writeText(packed.context_text)
    ui.toast('Контекст скопирован', 'ok')
  }

  async function createWikiDraft() {
    if (!packed?.context_text || !packedSnapshot || packedDirty || creatingWiki) return
    const snapshot = packedSnapshot.search
    const cleanQuery = snapshot.query.replace(/\s+/g, ' ')
    const title = `Контекст поиска: ${cleanQuery.slice(0, 72) || 'без названия'}`
    const slug = `search-context-${Date.now().toString(36)}`
    const content = [
      `# ${title}`,
      '',
      '> Черновик создан из pack_context. Это извлечённый контекст, а не сгенерированный ответ.',
      '',
      '## Запрос',
      '',
      snapshot.query,
      '',
      '## Параметры поиска',
      '',
      `- mode: ${snapshot.mode}`,
      `- hits: ${packed.hits.length} из ${packedSnapshot.sourceHitCount}`,
      `- tokens: ${packed.total_tokens} из ${packed.max_tokens}`,
      '',
      '## Упакованный контекст',
      '',
      packed.context_text,
    ].join('\n')

    creatingWiki = true
    error = ''
    try {
      const created = await api.putWiki({
        slug,
        title,
        content,
        kind: 'wiki',
        category: 'search-context',
        summary: `Черновик из ${packed.hits.length} хитов для запроса «${cleanQuery.slice(0, 96)}»`,
      })
      ui.toast('Wiki-черновик создан', 'ok')
      goWiki(created.document_id)
    } catch (cause) {
      error = cause instanceof Error ? cause.message : String(cause)
      ui.toast('Не удалось создать wiki-черновик', 'error')
    } finally {
      creatingWiki = false
    }
  }

  let appliedRoute = ''
  $effect(() => {
    const signature = route.query.toString()
    if (route.name !== 'search' || signature === appliedRoute) return
    appliedRoute = signature
    const routedQuery = route.query.get('q')
    documentId = route.query.get('document_id') ?? ''
    if (routedQuery) { query = routedQuery; void run() }
  })
</script>

<div class="search-lab">
  <form class="query-bar" onsubmit={(event) => { event.preventDefault(); void run() }}>
    <div class="modes" aria-label="Режим поиска">
      {#each ['lex', 'vec', 'hybrid'] as value}
        <button type="button" class:active={mode === value} onclick={() => mode = value as typeof mode}>{value}</button>
      {/each}
    </div>
    <input bind:value={query} aria-label="Поисковый запрос" placeholder="Спросите корпус — здесь видны ранги, причины и стоимость…" />
    <kbd>↵</kbd>
    <button class="run" type="submit" disabled={busy || !query.trim()}>{busy ? 'Ищу…' : 'Выполнить search'}</button>
    <button class="save" type="button" disabled title="HTTP API для записи eval-набора пока нет; сохранение доступно только через CLI">Сохранить как eval-запрос</button>
  </form>

  <div class="filters">
    {#if documentId}<button class="document-scope" type="button" title={`Поиск ограничен документом ${documentId}`} onclick={clearDocumentScope}>doc · {documentId.slice(0, 8)}… <span>×</span></button>{/if}
    <label>wing<input bind:value={wing} placeholder="все" /></label>
    <label>room<input bind:value={room} placeholder="все" /></label>
    <label>layer<select bind:value={layer}><option value="">все</option><option>raw</option><option>wiki</option><option>diary</option></select></label>
    <label>top_k<input type="number" min="1" max="100" bind:value={topK} /></label>
    <label>min_score<input type="number" min="0" step="0.01" bind:value={minScore} /></label>
    <label>rrf_k<input type="number" min="1" bind:value={rrfK} /></label>
    <label>max/doc<input type="number" min="1" bind:value={maxPerDocument} /></label>
    <label>neighbors<input type="number" min="0" max="20" bind:value={neighborChunks} /></label>
    <label>tokens<input type="number" min="100" bind:value={maxTokens} /></label>
    <label>recency<input type="number" min="0" bind:value={recencyDays} /><span>дн</span></label>
    <label>timeout<input type="number" min="100" bind:value={timeoutMs} /><span>мс</span></label>
  </div>

  {#if error}<div class="error-banner">{error}</div>{/if}

  <div class="workspace">
    <section class="results panel">
      <header>
        <div><strong>Хиты</strong><span>{hits.length}</span><span>{response?.mode ?? mode}</span>{#if resultsDirty}<span class="stale">параметры изменены</span>{/if}</div>
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
          <button class="hit" class:selected={selected === index} onclick={() => selected = index}>
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

    <section class="panel fusion">
      <header>
        <strong>{(response?.mode ?? mode) === 'hybrid' ? 'Слияние рангов · RRF' : 'Поток рангов'}</strong>
        <span>{(response?.mode ?? mode) === 'hybrid' ? `Σ 1 / (${appliedRrfK} + rank)` : `${response?.mode ?? mode} · без fusion`}</span>
      </header>
      <div class="rank-flow">
        <svg class="rank-links" viewBox="0 0 1000 289" preserveAspectRatio="none" aria-hidden="true">
          {#each flowConnections as connection (connection.hit.chunk_id)}
            {#if connection.lexIndex >= 0}
              <path
                class:selected-path={selected === connection.fusedIndex}
                style={`--flow-color:${flowColor(connection.fusedIndex)}`}
                d={flowPath(300, flowY(connection.lexIndex), 360, flowY(connection.fusedIndex))}
              />
              <circle style={`--flow-color:${flowColor(connection.fusedIndex)}`} cx="300" cy={flowY(connection.lexIndex)} r="2.4" />
              <circle style={`--flow-color:${flowColor(connection.fusedIndex)}`} cx="360" cy={flowY(connection.fusedIndex)} r="2.4" />
            {/if}
            {#if connection.vecIndex >= 0}
              <path
                class:selected-path={selected === connection.fusedIndex}
                style={`--flow-color:${flowColor(connection.fusedIndex)}`}
                d={flowPath(700, flowY(connection.vecIndex), 640, flowY(connection.fusedIndex))}
              />
              <circle style={`--flow-color:${flowColor(connection.fusedIndex)}`} cx="700" cy={flowY(connection.vecIndex)} r="2.4" />
              <circle style={`--flow-color:${flowColor(connection.fusedIndex)}`} cx="640" cy={flowY(connection.fusedIndex)} r="2.4" />
            {/if}
          {/each}
        </svg>

        <div class="rank-column">
          <b>lex · bm25</b>
          {#each flowLexRanks as hit (hit.chunk_id)}
            <button
              class:selected={selected === hits.indexOf(hit)}
              style={`--rank-color:${flowColor(hits.indexOf(hit))}`}
              title={`lex #${hit.rank_lex} → fused #${hits.indexOf(hit) + 1}`}
              onclick={() => selected = hits.indexOf(hit)}
            ><em>#{hit.rank_lex}</em><span>{hit.document_title}</span><strong>{score(hit.score_lex)}</strong></button>
          {/each}
        </div>
        <span class="flow-lane" aria-hidden="true">→</span>
        <div class="rank-column hybrid">
          <b>{(response?.mode ?? mode) === 'hybrid' ? 'hybrid · rrf' : `${response?.mode ?? mode} · result`}</b>
          {#each flowHits as hit, index (hit.chunk_id)}
            <button
              class:selected={selected === index}
              style={`--rank-color:${flowColor(index)}`}
              onclick={() => selected = index}
            ><em>#{index + 1}</em><span>{hit.document_title}</span><strong>{score(hit.score_rrf ?? hit.score)}</strong></button>
          {/each}
        </div>
        <span class="flow-lane" aria-hidden="true">←</span>
        <div class="rank-column">
          <b>vec · cosine</b>
          {#each flowVecRanks as hit (hit.chunk_id)}
            <button
              class:selected={selected === hits.indexOf(hit)}
              style={`--rank-color:${flowColor(hits.indexOf(hit))}`}
              title={`vec #${hit.rank_vec} → fused #${hits.indexOf(hit) + 1}`}
              onclick={() => selected = hits.indexOf(hit)}
            ><em>#{hit.rank_vec}</em><span>{hit.document_title}</span><strong>{score(hit.score_vec)}</strong></button>
          {/each}
        </div>
      </div>
      {#if selectedHit}<div class="fusion-note"><strong>{selectedHit.document_title}</strong><p>{selectedHit.explanation?.reasons?.join(' · ') || 'Результат ранжирован выбранным режимом.'}</p><dl><dt>chunk / offset</dt><dd>#{selectedHit.chunk_index} · {selectedHit.char_start??'—'}–{selectedHit.char_end??'—'}</dd><dt>dedup</dt><dd>{selectedHit.explanation?.deduplication??'нет'}</dd></dl><button class="secondary" onclick={()=>openCorpusDocument(selectedHit)}>Открыть в корпусе</button></div>{:else}<div class="empty small">Здесь появится поток lex → hybrid ← vec</div>{/if}
    </section>

    <aside class="analysis">
      <section class="panel pack">
        <header><strong>pack_context → агенту</strong>{#if packed}<span>{packed.total_tokens} / {packed.max_tokens} tok</span>{/if}</header>
        {#if packed}
          <div class="budget"><i style={`width:${Math.min(100, packed.total_tokens / packed.max_tokens * 100)}%`}></i></div>
          <p>{packed.hits.length} из {packedSnapshot?.sourceHitCount ?? '—'} хитов · исключено {packed.omitted_count}</p>
          {#if packedDirty}<p class="pack-stale">Параметры изменены. Повторите search и заново соберите контекст перед созданием wiki.</p>{/if}
          <pre>{packed.context_text}</pre>
          <div class="pack-actions">
            <button class="secondary" onclick={copyContext}>Скопировать</button>
            <button
              class="primary"
              disabled={creatingWiki || packedDirty}
              title={packedDirty ? 'Сначала повторите search и заново соберите контекст' : 'Создаст настоящую wiki-страницу через PUT /v1/wiki и откроет её'}
              onclick={createWikiDraft}
            >{creatingWiki ? 'Создаю…' : 'Создать wiki-черновик'}</button>
          </div>
        {:else}
          <p>Упакуйте ранжированные хиты в тот контекст, который реально увидит агент.</p>
          <button class="primary" disabled={!hits.length || packing || resultsDirty} onclick={pack}>{packing ? 'Упаковка…' : resultsDirty ? 'Сначала обновить search' : 'Собрать контекст'}</button>
        {/if}
      </section>
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

  .query-bar button.run,
  .query-bar button.save {
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

  .save {
    border: 1px solid var(--border);
    background: transparent;
    color: var(--text-muted);
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

  .results,
  .fusion,
  .pack {
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

  .analysis {
    display: block;
    min-height: 0;
  }

  .pack {
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

  .filters label { font-size: 10.5px; }
  .filters input,
  .filters select { font-size: 11px; }
  .panel header span { font-size: 10px; }
  .timing-scope { font-size: 9.5px; }
  .timing-stages span { font-size: 10px; }
  .hit-copy > b { font-size: 12.5px; }
  .hit-copy small { font-size: 10px; }
  .hit-copy p { font-size: 11.5px; }
  .hit-copy em span,
  .scores small,
  .scores em { font-size: 9.5px; }
  .pack > p { font-size: 11.5px; }
  .pack pre { font-size: 10.5px; }
  .rank-column > b { font-size: 10px; }
  .rank-flow button em,
  .rank-flow button strong { font-size: 9px; }
  .rank-flow button span { font-size: 10px; }
  .fusion-note > strong { font-size: 11.5px; }
  .fusion-note p,
  .fusion-note dl { font-size: 10px; }

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

    .results {
      min-height: 480px;
    }

    .query-bar .save {
      display: none;
    }
  }
</style>
