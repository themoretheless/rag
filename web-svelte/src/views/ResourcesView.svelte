<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '@/api/client'
  import type { ResourcesResponse } from '@/api/types'

  const POLL_MS = 2_000
  const HISTORY = 90 // 90 точек × 2 с ≈ 3 минуты истории

  let data = $state<ResourcesResponse | null>(null)
  let error = $state('')
  let cpuHistory = $state<number[]>([])
  let rssHistory = $state<number[]>([])
  let embedHistory = $state<number[]>([])

  function fmtBytes(bytes: number): string {
    if (!bytes) return '0'
    const units = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ']
    const i = Math.min(units.length - 1, Math.floor(Math.log2(bytes) / 10))
    const v = bytes / 2 ** (10 * i)
    return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${units[i]}`
  }

  function fmtUptime(seconds: number): string {
    const d = Math.floor(seconds / 86400)
    const h = Math.floor((seconds % 86400) / 3600)
    const m = Math.floor((seconds % 3600) / 60)
    if (d) return `${d} д ${h} ч`
    if (h) return `${h} ч ${m} мин`
    return `${m} мин ${Math.floor(seconds % 60)} с`
  }

  function fmtMs(ms: number): string {
    if (ms >= 1000) return `${(ms / 1000).toFixed(1)} с`
    return `${ms} мс`
  }

  /** SVG polyline points для спарклайна значений 0..max. */
  function spark(values: number[], width: number, height: number, max: number): string {
    if (!values.length) return ''
    const cap = max > 0 ? max : 1
    const step = values.length > 1 ? width / (values.length - 1) : width
    return values
      .map((v, i) => `${(i * step).toFixed(1)},${(height - (Math.min(v, cap) / cap) * (height - 2) - 1).toFixed(1)}`)
      .join(' ')
  }

  async function tick() {
    try {
      const next = await api.resources()
      data = next
      error = ''
      cpuHistory = [...cpuHistory, next.process.cpu_percent].slice(-HISTORY)
      rssHistory = [...rssHistory, next.process.rss_bytes / 2 ** 30].slice(-HISTORY)
      embedHistory = [...embedHistory, next.embedder.in_flight].slice(-HISTORY)
    } catch (cause) {
      error = cause instanceof Error ? cause.message : 'Не удалось получить /v1/resources'
    }
  }

  onMount(() => {
    void tick()
    const timer = setInterval(() => void tick(), POLL_MS)
    return () => clearInterval(timer)
  })

  const cpuMax = $derived(Math.max(100, ...cpuHistory) * 1.1)
  const rssMax = $derived(Math.max(1, ...rssHistory) * 1.1)
  const embedMax = $derived(Math.max(2, data?.embedder.concurrency_limit ?? 0, ...embedHistory))
  const memUsedPct = $derived(
    data && data.process.system_memory_total > 0
      ? (data.process.system_memory_used / data.process.system_memory_total) * 100
      : 0,
  )
</script>

<div class="resources-page">
  {#if error}<p class="error">{error}</p>{/if}
  {#if data}
    <div class="cards">
      <section class="panel">
        <header><b>CPU процесса</b><span>pid {data.process.pid}</span></header>
        <div class="metric"><strong>{data.process.cpu_percent.toFixed(0)}%</strong>
          <small>из ~{data.process.logical_cpus * 100}% · потоков tokio: {data.process.tokio_worker_threads}</small>
        </div>
        <svg class="spark" viewBox="0 0 300 48" preserveAspectRatio="none">
          <polyline points={spark(cpuHistory, 300, 48, cpuMax)} />
        </svg>
        <footer>load avg: {data.process.load_avg_1m.toFixed(2)} / {data.process.load_avg_5m.toFixed(2)} / {data.process.load_avg_15m.toFixed(2)}</footer>
      </section>

      <section class="panel">
        <header><b>Память процесса</b><span>RSS</span></header>
        <div class="metric"><strong>{fmtBytes(data.process.rss_bytes)}</strong>
          <small>virtual {fmtBytes(data.process.virtual_bytes)} · система занята на {memUsedPct.toFixed(0)}%</small>
        </div>
        <svg class="spark" viewBox="0 0 300 48" preserveAspectRatio="none">
          <polyline points={spark(rssHistory, 300, 48, rssMax)} />
        </svg>
        <footer>система: {fmtBytes(data.process.system_memory_used)} / {fmtBytes(data.process.system_memory_total)}</footer>
      </section>

      <section class="panel">
        <header><b>Эмбеддер</b><span>лимит {data.embedder.concurrency_limit}</span></header>
        <div class="metric"><strong>{data.embedder.in_flight}</strong>
          <small>активных батчей · всего вызовов {data.embedder.calls_total} · текстов {data.embedder.texts_total}</small>
        </div>
        <svg class="spark" viewBox="0 0 300 48" preserveAspectRatio="none">
          <polyline points={spark(embedHistory, 300, 48, embedMax)} />
        </svg>
        <footer>
          последний батч {fmtMs(data.embedder.last_batch_ms)} · макс {fmtMs(data.embedder.max_batch_ms)}
          · ожидание {fmtMs(data.embedder.wait_ms_total)}
        </footer>
      </section>

      <section class="panel">
        <header><b>База данных</b><span>DuckDB</span></header>
        <dl>
          <dt>размер</dt><dd>{fmtBytes(data.database.bytes)}</dd>
          <dt>WAL</dt>
          <dd class:danger-text={data.database.wal_bytes >= data.database.wal_warn_bytes}>
            {fmtBytes(data.database.wal_bytes)}{#if data.database.wal_bytes >= data.database.wal_warn_bytes} ⚠{/if}
          </dd>
          <dt>threads</dt><dd>{data.database.threads > 0 ? data.database.threads : 'по умолчанию (все ядра)'}</dd>
          <dt>memory_limit</dt><dd>{data.database.memory_limit ?? 'не задан'}</dd>
        </dl>
      </section>

      <section class="panel">
        <header><b>Рантайм</b><span>v{data.runtime.version}</span></header>
        <dl>
          <dt>аптайм</dt><dd>{fmtUptime(data.process.uptime_seconds)}</dd>
          <dt>commit</dt><dd>{data.runtime.build_commit}</dd>
          <dt>семплирование</dt><dd>каждые 2 с · {new Date(data.process.sampled_at).toLocaleTimeString()}</dd>
        </dl>
      </section>

      <section class="panel tuning">
        <header><b>Настройка</b><span>env</span></header>
        <dl>
          <dt>RAG_WORKER_THREADS</dt><dd>{data.runtime.worker_threads_env ?? 'не задано (= ядра)'}</dd>
          <dt>RAG_EMBED_CONCURRENCY</dt><dd>{data.embedder.concurrency_limit}</dd>
          <dt>RAG_DB_THREADS</dt><dd>{data.database.threads > 0 ? data.database.threads : 'не задано'}</dd>
          <dt>RAG_DB_MEMORY_LIMIT</dt><dd>{data.database.memory_limit ?? 'не задано'}</dd>
        </dl>
        <p class="hint">Переменные читаются при старте процесса; перезапустите rag-mcp после изменения.</p>
      </section>
    </div>
  {:else if !error}
    <p class="loading">Загружаю метрики…</p>
  {/if}
</div>

<style>
  .resources-page{display:flex;flex-direction:column;gap:12px;overflow:auto;padding:12px}
  .cards{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:12px}
  .panel{border:1px solid var(--border);border-radius:8px;background:var(--bg);overflow:hidden}
  .panel>header{min-height:38px;padding:0 12px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between}
  .panel>header b{font-size:12px}
  .panel>header span{font:9px var(--mono);color:var(--text-faint)}
  .metric{padding:10px 12px 4px;display:flex;flex-direction:column;gap:3px}
  .metric strong{font-size:24px;font-weight:700}
  .metric small{font-size:10px;color:var(--text-muted)}
  .spark{display:block;width:100%;height:48px;padding:0}
  .spark polyline{fill:none;stroke:var(--l3);stroke-width:1.5}
  .panel footer{padding:6px 12px 9px;font:9px var(--mono);color:var(--text-faint)}
  dl{display:grid;grid-template-columns:110px 1fr;gap:6px;padding:11px 12px;margin:0;font-size:11px}
  dt{color:var(--text-faint)}
  dd{margin:0;font-family:var(--mono);word-break:break-all}
  dd.danger-text{color:var(--danger);font-weight:700}
  .hint{margin:0 12px 10px;font-size:10px;color:var(--text-faint);line-height:1.45}
  .error{margin:0;padding:10px 12px;border:1px solid var(--danger);border-radius:8px;color:var(--danger);font-size:12px}
  .loading{color:var(--text-muted);font-size:12px}
</style>
