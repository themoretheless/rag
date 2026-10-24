<script lang="ts">
  import RealQuestionEvaluation from '@/components/RealQuestionEvaluation.svelte'
  import { onMount } from 'svelte'
  import { api } from '@/api/client'
  import { ui } from '@/lib/state/ui.svelte'
  import type { EvalHistoryResponse, EvalModeReport, EvalQueryReport, EvalRun, EvalScaleRecommendation, StorageStatus } from '@/api/types'

  type MetricKey = 'recall_at_k' | 'mrr' | 'ndcg_at_k' | 'p95_search_ms' | 'reciprocal_rank' | 'search_ms'
  type MetricSource = Partial<Record<MetricKey, unknown>>
  const MODE_NAMES = ['lex', 'vec', 'hybrid'] as const

  let history = $state<EvalRun[]>([])
  let configured = $state<boolean | null>(null)
  let path = $state('')
  let busy = $state(false)
  let error = $state('')
  let selectedQuery = $state<string | null>(null)
  let liveChunks = $state<number | null>(null)

  const latest = $derived(history[0] ?? null)
  const chronologicalHistory = $derived([...history].reverse())
  const modes = $derived<EvalModeReport[]>(latest?.modes ?? [])
  const queries = $derived<EvalQueryReport[]>(modes.find((item) => item.mode === 'hybrid')?.queries ?? modes[0]?.queries ?? [])
  const scale = $derived<EvalScaleRecommendation>(latest?.scale_recommendation ?? {})
  const hybrid = $derived(modes.find((item) => item.mode === 'hybrid') ?? null)

  const bestMode = $derived.by(() => {
    let best: { mode: string; quality: number; p95: number } | null = null
    for (const report of modes) {
      const qualityMetrics = (['recall_at_k', 'mrr', 'ndcg_at_k'] as const).map((key) => metric(report, key))
      if (qualityMetrics.some((value) => value === null)) continue
      const quality = qualityMetrics.reduce<number>((sum, value) => sum + (value ?? 0), 0) / qualityMetrics.length
      const p95 = metric(report, 'p95_search_ms') ?? Number.POSITIVE_INFINITY
      if (!best || quality > best.quality || (quality === best.quality && p95 < best.p95)) {
        best = { mode: report.mode, quality, p95 }
      }
    }
    return best?.mode ?? null
  })

  const scaleP95 = $derived.by(() => {
    const values = modes
      .filter((item) => item.mode === 'vec' || item.mode === 'hybrid')
      .map((item) => metric(item, 'p95_search_ms'))
      .filter((value): value is number => value !== null)
    return values.length ? Math.max(...values) : null
  })

  const chartHistory = $derived(chronologicalHistory.flatMap((run) => {
    const recall = metric(run.modes?.find((mode) => mode.mode === 'hybrid'), 'recall_at_k')
    return recall === null ? [] : [{ run, recall }]
  }))

  /** Per-mode metric cards, computed once per history change instead of per render call. */
  const modeCards = $derived(MODE_NAMES.map((name) => {
    const report: MetricSource = modes.find((item) => item.mode === name) ?? {}
    return {
      name,
      recall: metric(report, 'recall_at_k'),
      mrr: metric(report, 'mrr'),
      ndcg: metric(report, 'ndcg_at_k'),
      p95: metric(report, 'p95_search_ms'),
    }
  }))

  /** Per-query table rows with metrics and relevant results precomputed. */
  const queryRows = $derived(queries.map((query) => {
    const results = query.results ?? []
    const relevant = results.filter((result) => result.relevance > 0)
    const rr = metric(query, 'reciprocal_rank')
    const searchMs = metric(query, 'search_ms')
    return {
      query,
      results,
      relevantCount: relevant.length,
      relevantTitles: relevant.map((result) => result.document_title).join(' · '),
      recall: metric(query, 'recall_at_k'),
      rr,
      ndcg: metric(query, 'ndcg_at_k'),
      searchMs: searchMs === null ? null : Math.round(searchMs),
      weak: rr !== null && rr < .5,
    }
  }))

  const recentRuns = $derived(history.slice(0, 5).map((run) => ({
    run,
    recall: metric(run.modes?.find((mode) => mode.mode === 'hybrid'), 'recall_at_k'),
  })))

  const hybridRecall = $derived(metric(hybrid, 'recall_at_k'))
  const hybridMrr = $derived(metric(hybrid, 'mrr'))
  const hybridP95 = $derived(metric(hybrid, 'p95_search_ms'))
  const recallCi = $derived(hybridRecall === null ? null : hybridRecall >= .75)
  const mrrCi = $derived(hybridMrr === null ? null : hybridMrr >= .6)
  const p95Ci = $derived(hybridP95 === null ? null : hybridP95 <= 300)

  async function load() {
    busy = true
    error = ''
    const [historyResult, statusResult] = await Promise.allSettled([
      api.get<EvalHistoryResponse>('/v1/eval/history?limit=30'),
      api.get<StorageStatus>('/v1/status'),
    ])
    if (historyResult.status === 'fulfilled') {
      const response = historyResult.value ?? {}
      // The backend preserves append-only JSONL order (oldest -> newest).
      // Keep the screen state newest-first for latest, tables and the run list.
      history = [...(response.items ?? [])].reverse()
      configured = typeof response.configured === 'boolean' ? response.configured : null
      path = response.path ?? ''
    } else {
      const cause = historyResult.reason
      error = cause instanceof Error ? cause.message : String(cause)
    }
    if (statusResult.status === 'fulfilled') liveChunks = statusResult.value?.chunk_count ?? null
    busy = false
  }

  function metric(mode: MetricSource | null | undefined, key: MetricKey): number | null {
    const value = mode?.[key]
    return typeof value === 'number' && Number.isFinite(value) ? value : null
  }

  function fixed(value: number | null, digits = 2) { return value === null ? '—' : value.toFixed(digits) }
  function milliseconds(value: number | null) { return value === null ? '—' : `${Math.round(value)} мс` }
  function percent(value: number | null) { return value === null ? '0%' : `${Math.max(0, Math.min(100, Math.round(value * 100)))}%` }
  function modeLabel(name: string) {
    const base = name === 'lex' ? 'bm25' : name === 'vec' ? 'cosine' : 'default'
    return bestMode === name ? `${base} · лучший` : base
  }
  function ciMark(value: boolean | null) { return value === null ? '—' : value ? '✓' : '!' }
  async function copyCommand() {
    const command = 'cargo run --release --bin eval -- --dataset data/eval/example-v1.json --min-recall-at-k 0.75 --min-mrr 0.6 --max-p95-ms 300 --history-jsonl bench.jsonl'
    await navigator.clipboard.writeText(command)
    ui.toast('Команда eval скопирована', 'ok')
  }
  onMount(load)
</script>

<div class="eval-page screen">
  <RealQuestionEvaluation />
  <div class="screen-head">
    <div><h1>Оценка извлечения</h1><p>{latest ? `${latest.dataset_name} v${latest.dataset_version} · ${latest.sampling?.queries ?? '—'} запросов · временная БД` : 'Recall, MRR, nDCG и задержка на воспроизводимом датасете'}</p></div>
    <div class="head-actions"><span class:ready={configured === true}>{configured === null ? 'history неизвестна' : configured ? 'history подключена' : 'history не настроена'}</span><button class="secondary" onclick={load}>Обновить</button><button class="primary" onclick={copyCommand} title="Копирует команду для ручного запуска в терминале">Скопировать CLI</button></div>
  </div>
  {#if error}<div class="notice">{error}</div>{/if}

  <section class="metric-grid">
    {#each modeCards as card (card.name)}
      <article class:best={bestMode === card.name} class="metric-card">
        <header><strong>{card.name}</strong><span>{modeLabel(card.name)}</span></header>
        <div class="metric-row"><span>recall@{latest?.top_k ?? 'k'}</span><b>{fixed(card.recall)}</b><i><em style={`width:${percent(card.recall)}`}></em></i></div>
        <div class="metric-row"><span>MRR</span><b>{fixed(card.mrr)}</b><i><em style={`width:${percent(card.mrr)}`}></em></i></div>
        <div class="metric-row"><span>nDCG@{latest?.top_k ?? 'k'}</span><b>{fixed(card.ndcg)}</b><i><em style={`width:${percent(card.ndcg)}`}></em></i></div>
        <footer>p95 поиска <b>{milliseconds(card.p95)}</b></footer>
      </article>
    {/each}
    <article class="metric-card scale"><header><strong>Пороги масштаба</strong><span>{scale.path ?? 'наблюдение'}</span></header><div class="threshold"><span>чанков в корпусе</span><b>{latest?.corpus?.chunks ?? liveChunks ?? '—'} / {scale.chunk_threshold ?? 50000}</b><i><em style={`width:${Math.min(100,(latest?.corpus?.chunks ?? liveChunks ?? 0)/(scale.chunk_threshold ?? 50000)*100)}%`}></em></i></div><div class="threshold"><span>p95 vec / hybrid</span><b>{scaleP95 === null ? '—' : `${Math.round(scaleP95)} / ${scale.p95_search_ms_threshold ?? 300} мс`}</b><i><em style={`width:${scaleP95 === null ? 0 : Math.min(100,scaleP95/(scale.p95_search_ms_threshold ?? 300)*100)}%`}></em></i></div><p>{scale.reason ?? 'ANN включается только по измеренным порогам, не по предположению.'}</p></article>
  </section>

  <section class="lower">
    <article class="panel query-table">
      <header><strong>Разбор по запросам</strong><span>{queries.length}</span></header>
      <div class="thead"><span>id</span><span>Релевантные документы · текст запроса не хранится</span><span>recall</span><span>MRR</span><span>nDCG</span><span>мс</span></div>
      <div class="query-scroll">
        {#each queryRows as row (row.query.id)}
          <button class:weak={row.weak} class:expanded={selectedQuery === row.query.id} aria-expanded={selectedQuery === row.query.id} onclick={() => { selectedQuery = selectedQuery === row.query.id ? null : row.query.id }}>
            <code>{row.query.id}</code><span><b>{row.relevantCount} релевантных</b><small>{row.relevantTitles || 'нет размеченных результатов'}</small>{#if selectedQuery === row.query.id}<em>{#each row.results.slice(0, 6) as result}<i>#{result.rank} {result.document_title} · rel {result.relevance}</i>{/each}</em>{/if}</span><strong>{fixed(row.recall)}</strong><strong>{fixed(row.rr)}</strong><strong>{fixed(row.ndcg)}</strong><strong>{row.searchMs ?? '—'}</strong>
          </button>
        {/each}
        {#if !queries.length}<div class="empty">{busy ? 'Загрузка истории…' : configured === null ? 'Статус history неизвестен' : configured ? 'История пуста: запустите eval с --history-jsonl' : 'Укажите RAG_EVAL_HISTORY и перезапустите gateway'}</div>{/if}
      </div><footer>Клик раскрывает ранжированные результаты; history хранит ID и метрики, но не текст запроса.</footer>
    </article>

    <aside>
      <article class="panel history"><header><strong>История · hybrid recall</strong><span>{history.length} прогонов</span></header><div class="chart"><svg viewBox="0 0 320 112" preserveAspectRatio="none"><line x1="0" x2="320" y1="92" y2="92"></line><polyline points={chartHistory.map((point,index) => `${chartHistory.length <= 1 ? 160 : index/(chartHistory.length-1)*310+5},${98-point.recall*80}`).join(' ')}></polyline>{#each chartHistory as point,index}<circle cx={chartHistory.length <= 1 ? 160 : index/(chartHistory.length-1)*310+5} cy={98-point.recall*80} r="3"></circle>{/each}</svg></div><div class="runs">{#each recentRuns as { run, recall },index}<div><span>{run.dataset_name} v{run.dataset_version}</span><b>{fixed(recall)}</b><em>{index === 0 ? 'последний' : `−${index}`}</em></div>{/each}</div></article>
      <article class="panel ci"><header><strong>Пороги CI · exit ≠ 0</strong></header><div><span>min recall@k</span><b>.75</b><i class:pass={recallCi === true} class:unknown={recallCi === null}>{ciMark(recallCi)}</i></div><div><span>min MRR</span><b>.60</b><i class:pass={mrrCi === true} class:unknown={mrrCi === null}>{ciMark(mrrCi)}</i></div><div><span>max p95</span><b>300 ms</b><i class:pass={p95Ci === true} class:unknown={p95Ci === null}>{ciMark(p95Ci)}</i></div><code>{path || 'RAG_EVAL_HISTORY'}</code></article>
    </aside>
  </section>
</div>

<style>
  .eval-page{display:flex;flex-direction:column;overflow:hidden}.head-actions{display:flex;align-items:center;gap:7px}.head-actions>span{font:8px var(--mono);color:var(--warn);padding:4px 7px;border:1px solid color-mix(in srgb,var(--warn) 35%,transparent);border-radius:6px}.head-actions>span.ready{color:var(--ok);border-color:color-mix(in srgb,var(--ok) 35%,transparent)}.metric-grid{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:10px;margin-bottom:12px;flex:0 0 auto}.metric-card{padding:12px 13px;background:var(--surface);border:1px solid var(--border);border-radius:11px}.metric-card.best{border-color:color-mix(in srgb,var(--l1) 55%,var(--border));box-shadow:inset 0 2px var(--l1)}.metric-card header{display:flex;align-items:center;justify-content:space-between;margin-bottom:11px}.metric-card header strong{font:600 13px var(--mono)}.metric-card header span{font:8px var(--mono);color:var(--text-faint)}.metric-row{display:grid;grid-template-columns:72px 34px 1fr;align-items:center;gap:6px;margin:7px 0;font-size:9px;color:var(--text-muted)}.metric-row>b{font:10px var(--mono);color:var(--text)}.metric-row>i,.threshold>i{height:3px;background:var(--border);border-radius:2px;overflow:hidden}.metric-row>i em,.threshold>i em{display:block;height:100%;background:var(--l1)}.metric-card footer{border-top:1px solid var(--border);padding-top:8px;margin-top:10px;color:var(--text-faint);font-size:9px}.metric-card footer b{float:right;color:var(--text)}.threshold{display:grid;grid-template-columns:1fr auto;gap:4px 8px;margin:8px 0;color:var(--text-muted);font-size:9px}.threshold>i{grid-column:1/3}.threshold b{font:9px var(--mono);color:var(--text)}.scale p{margin:9px 0 0;color:var(--text-faint);font-size:8px;line-height:1.4}.lower{display:grid;grid-template-columns:minmax(560px,1fr) 360px;gap:12px;flex:1;min-height:0}.query-table{display:flex;flex-direction:column;min-height:0}.panel>header{height:41px;padding:0 12px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between}.panel>header span{font:8px var(--mono);color:var(--text-faint)}.thead,.query-scroll>button{display:grid;grid-template-columns:90px 1fr 48px 48px 48px 38px;gap:6px;align-items:center}.thead{height:28px;padding:0 10px;border-bottom:1px solid var(--border);font:7px var(--mono);color:var(--text-faint)}.query-scroll{overflow:auto;flex:1;min-height:0}.query-scroll>button{width:100%;min-height:48px;padding:7px 10px;border:0;border-bottom:1px solid var(--border);background:transparent;color:var(--text);text-align:left;cursor:pointer}.query-scroll>button:hover,.query-scroll>button.expanded{background:var(--bg-hover)}.query-scroll>button.weak{background:color-mix(in srgb,var(--warn) 5%,transparent)}.query-scroll code{font:8px var(--mono);color:var(--l1)}.query-scroll button>span{display:flex;flex-direction:column;min-width:0}.query-scroll button>span b{font-size:9px}.query-scroll button>span small{margin-top:2px;color:var(--text-faint);font:7px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.query-scroll button>span em{display:flex;flex-wrap:wrap;gap:3px;margin-top:6px}.query-scroll button>span em i{font:7px var(--mono);font-style:normal;padding:2px 4px;border-radius:3px;background:var(--bg-hover)}.query-scroll button>strong{font:9px var(--mono);text-align:right}.query-table>footer{padding:7px 10px;border-top:1px solid var(--border);color:var(--text-faint);font-size:8px}.lower>aside{display:grid;grid-template-rows:1fr auto;gap:12px;min-height:0}.history{min-height:0}.chart{height:125px;padding:9px}.chart svg{width:100%;height:100%;overflow:visible}.chart line{stroke:var(--border);stroke-width:1}.chart polyline{fill:none;stroke:var(--l1);stroke-width:2}.chart circle{fill:var(--bg);stroke:var(--l1);stroke-width:2}.runs{padding:0 11px}.runs>div{display:grid;grid-template-columns:1fr 35px 44px;padding:6px 0;border-top:1px solid var(--border);font-size:8px}.runs b,.runs em{font:8px var(--mono);font-style:normal;text-align:right}.runs em{color:var(--text-faint)}.ci{padding-bottom:9px}.ci>div{display:grid;grid-template-columns:1fr 60px 20px;gap:7px;padding:5px 11px;font-size:9px}.ci>div b{font:9px var(--mono)}.ci>div i{font-style:normal;color:var(--danger)}.ci>div i.pass{color:var(--ok)}.ci>div i.unknown{color:var(--text-faint)}.ci>code{display:block;margin:7px 10px 0;padding:6px;background:#090b10;border-radius:5px;color:var(--text-faint);font:7px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.empty{padding:50px;text-align:center;color:var(--text-faint);font-size:9px}.notice{padding:8px;color:var(--danger)}@media(max-width:1080px){.eval-page{overflow:auto}.metric-grid{grid-template-columns:1fr 1fr}.lower{grid-template-columns:1fr;flex:none;min-height:400px}.lower>aside{display:none}}
  .head-actions>span{font-size:10px}.metric-card header span{font-size:10px}.metric-row{font-size:11px}.metric-row>b{font-size:11px}.metric-card footer{font-size:10.5px}.threshold{font-size:10.5px}.threshold b{font-size:10px}.scale p{font-size:10px}.panel>header span{font-size:10px}.thead{font-size:9.5px}.query-scroll code{font-size:10px}.query-scroll button>span b{font-size:11px}.query-scroll button>span small{font-size:9px}.query-scroll button>span em i{font-size:9px}.query-scroll button>strong{font-size:10.5px}.query-table>footer{font-size:10px}.runs>div,.runs b,.runs em{font-size:10px}.ci>div{font-size:10.5px}.ci>div b{font-size:10px}.ci>code{font-size:9px}.empty{font-size:11px}
</style>
