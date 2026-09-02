<script lang="ts">
  import { onMount } from 'svelte'
  import { api, loadPanels } from '@/api/client'
  import { go } from '@/lib/router.svelte'
  import { ui } from '@/lib/state/ui.svelte'

  let status = $state<any>({})
  let doctor = $state<any>({})
  let runtime = $state<any>({})
  let taxonomy = $state<any>({ wings: [] })
  let operations = $state<any[]>([])
  let agents = $state<any[]>([])
  let kg = $state<any>({})
  let busy = $state(false)
  let error = $state('')
  let backupOpen = $state(false)
  let backupPath = $state('')
  let backupBusy = $state(false)

  async function load() {
    busy = true
    error = ''
    const failures = await loadPanels([
      ['status', async () => { status = await api.get<any>('/v1/status') }],
      ['doctor', async () => { doctor = await api.get<any>('/v1/doctor') }],
      ['runtime', async () => { runtime = (await api.get<any>('/v1/runtime')).runtime ?? {} }],
      ['taxonomy', async () => { taxonomy = (await api.get<any>('/v1/taxonomy')).taxonomy ?? taxonomy }],
      ['ops_log', async () => { operations = (await api.get<any>('/v1/ops-log?limit=8')).items ?? [] }],
      ['agents', async () => { agents = (await api.get<any>('/v1/agents')).items ?? [] }],
      ['KG', async () => { kg = (await api.get<any>('/v1/kg/stats')).stats ?? {} }],
    ])
    error = failures.length ? `Не обновлены панели: ${failures.join(', ')}` : ''
    busy = false
  }

  async function checkpoint() {
    try { await api.post('/v1/operations/checkpoint'); ui.toast('Checkpoint завершён', 'ok'); await load() }
    catch (cause) { ui.toast(cause instanceof Error ? cause.message : String(cause), 'error') }
  }

  async function backup() {
    if (!backupPath.trim()) return
    backupBusy = true
    try {
      await api.post('/v1/operations/backup', { path: backupPath.trim(), dry_run: false, overwrite: false })
      ui.toast('Резервная копия создана', 'ok')
      backupOpen = false
    } catch (cause) { ui.toast(cause instanceof Error ? cause.message : String(cause), 'error') }
    finally { backupBusy = false }
  }

  function count(value?: number) { return value == null ? '—' : new Intl.NumberFormat('ru-RU').format(value) }
  function bytes(value?: number) { if (value == null) return '—'; return value > 1024 ** 3 ? `${(value / 1024 ** 3).toFixed(1)} ГБ` : `${Math.round(value / 1024 ** 2)} МБ` }
  function uptime(seconds?: number) { if (seconds == null) return '—'; return `${Math.floor(seconds / 3600)} ч ${Math.floor((seconds % 3600) / 60)} мин` }
  function time(value?: string) { return value ? new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false }) : '—' }
  function triState(value: unknown): boolean | undefined {
    return typeof value === 'boolean' ? value : undefined
  }
  function zeroState(value: unknown): boolean | undefined {
    return typeof value === 'number' ? value === 0 : undefined
  }
  function stateLabel(value: unknown, ready: string, failed: string): string {
    return value === true ? ready : value === false ? failed : 'проверка…'
  }

  const onlineAgents = $derived(agents.filter((agent) => agent.online).length)
  const wikiIndexPercent = $derived.by(() => {
    if (typeof status.index_coverage !== 'number') return null
    const percent = Math.max(0, Math.min(1, status.index_coverage)) * 100
    return status.index_coverage >= 1 ? 100 : Math.floor(percent * 10) / 10
  })
  const wikiIndexState = $derived(
    wikiIndexPercent === null
      ? 'проверка…'
      : status.index_coverage >= 1
        ? 'готово'
        : typeof status.index_entry_count === 'number' && typeof status.wiki_count === 'number' && status.wiki_count > status.index_entry_count
          ? `${status.wiki_count - status.index_entry_count} без индекса`
          : `покрытие ${wikiIndexPercent}%`,
  )
  const integrityState = $derived(
    stateLabel(doctor.relational_integrity_ok, 'целостен', 'нарушен'),
  )
  const doctorRows = $derived([
    { label: `schema_version ${doctor.schema_version ?? '—'} = ${doctor.expected_schema_version ?? '—'}`, ok: triState(doctor.schema_ok), action: '' },
    { label: `FTS ${stateLabel(doctor.fts_ready, 'готов · read-your-writes', 'не готов')}`, ok: triState(doctor.fts_ready), action: '' },
    { label: `Манифест эмбеддингов ${doctor.embed_dims ?? '—'}d`, ok: triState(doctor.embed_ok), action: '' },
    { label: `${count(doctor.orphan_chunks)} orphan chunks · ${count(doctor.orphan_edges)} orphan edges`, ok: triState(doctor.relational_integrity_ok), action: '' },
    { label: `WAL ${bytes(doctor.wal_bytes)} из ${bytes(doctor.wal_warn_bytes)}`, ok: typeof doctor.wal_too_large === 'boolean' ? !doctor.wal_too_large : undefined, action: doctor.wal_too_large === true ? 'checkpoint' : '' },
    { label: `${count(doctor.documents_without_chunks)} документов без чанков`, ok: zeroState(doctor.documents_without_chunks), action: 'reingest' },
    { label: `${count(doctor.unscoped_documents)} документов без крыла`, ok: zeroState(doctor.unscoped_documents), action: 'refile' },
  ])
  const maxWing = $derived(Math.max(1, ...((taxonomy.wings ?? []).map((wing:any) => wing.document_count))))
  onMount(load)
</script>

<div class="console-page screen">
  <div class="screen-head">
    <div><h1>Пульт</h1><p>один писатель: gateway <span class="mono">pid {status.pid ?? runtime.pid ?? '—'}</span> · аптайм {uptime(status.uptime_seconds ?? runtime.uptime_seconds)} · <span class="mono">{status.db_path ?? '—'}</span> · schema v{status.schema_version ?? '—'}</p></div>
    <div class="actions"><button class="secondary" onclick={load} disabled={busy}>doctor</button><button class="secondary" onclick={() => backupOpen = !backupOpen}>backup_db</button><button class="primary" disabled title="HTTP compile-all в gateway отсутствует">Скомпилировать {count(status.uncompiled_raw_count)} raw → вики</button></div>
  </div>
  {#if backupOpen}<div class="backup"><input bind:value={backupPath} placeholder="Путь внутри RAG_INGEST_ROOTS, например /…/rag-backup.duckdb" /><button class="primary" onclick={backup} disabled={backupBusy || !backupPath.trim()}>{backupBusy ? 'Копирую…' : 'Создать backup'}</button></div>{/if}
  {#if error}<div class="notice">{error}</div>{/if}

  <section class="kpis">
    <article><span>Документы</span><strong>{count(status.document_count)}</strong><small>raw {count(status.raw_count)} · wiki {count(status.wiki_count)}</small></article>
    <article><span>Чанки</span><strong>{count(status.chunk_count)}</strong><small>≈{status.document_count ? (status.chunk_count / status.document_count).toFixed(1) : '—'} / док · {status.embed_model ?? '—'} · {status.embed_dims ?? '—'}d</small></article>
    <article><span>Граф</span><strong>{count(status.node_count)} / {count(status.edge_count)}</strong><small>узлы / рёбра</small></article>
    <article><span>Индекс вики</span><strong>{wikiIndexPercent == null ? '—' : `${wikiIndexPercent}%`}</strong><div class="progress" class:unknown={wikiIndexPercent == null}><i style={`width:${wikiIndexPercent ?? 0}%`}></i></div><small>{count(status.index_entry_count)} из {count(status.wiki_count)} страниц</small></article>
    <article class="debt"><span>Долг компиляции</span><strong>{count(status.uncompiled_raw_count)}</strong><small>raw-документов без покрытия вики</small></article>
  </section>

  <section class="dashboard">
    <article class="panel layers">
      <header><div><strong>Слои хранилища</strong><small>верхние цитируют нижние · raw неизменяем</small></div></header>
      <button style="--layer:var(--l4)" onclick={() => go('agents')}><b>L4</b><span><strong>Агенты и клиенты</strong><small>{agents.length} источников активности · {onlineAgents} online heartbeat · {count(kg.total_facts)} фактов KG</small></span><em>{onlineAgents ? `${onlineAgents} онлайн` : 'нет активности'}</em><i>›</i></button>
      <button style="--layer:var(--l3)" onclick={() => go('wiki')}><b>L3</b><span><strong>Скомпилированное знание</strong><small>{count(status.wiki_count)} страниц вики · index {wikiIndexPercent == null ? '—' : `${wikiIndexPercent}%`}</small></span><em>{wikiIndexState}</em><i>›</i></button>
      <button style="--layer:var(--l2)" onclick={() => go('graph')}><b>L2</b><span><strong>Граф объектов</strong><small>{count(status.node_count)} узлов · {count(status.edge_count)} рёбер</small></span><em>{integrityState}</em><i>›</i></button>
      <button style="--layer:var(--l1)" onclick={() => go('search')}><b>L1</b><span><strong>Извлечение</strong><small>{count(status.chunk_count)} чанков · FTS {stateLabel(status.fts_ready, 'готов', 'не готов')} · {status.embed_provider ?? '—'}/{status.embed_model ?? '—'}</small></span><em>{stateLabel(status.ready_for_search, 'ready_for_search', 'not ready')}</em><i>›</i></button>
      <button style="--layer:var(--l0)" onclick={() => go('corpus')}><b>L0</b><span><strong>Сырой корпус</strong><small>{count(status.raw_count)} документов · {(taxonomy.wings ?? []).length} крыльев · {count(doctor.unscoped_documents)} без крыла</small></span><em>immutable</em><i>›</i></button>
      <footer><span>Хранилище: {status.backend ?? '—'} · {bytes(status.db_file_bytes)} · WAL {bytes(status.wal_bytes)} из {bytes(status.wal_warn_bytes)}</span><code>caps: {(status.storage_capabilities ?? []).join(' · ') || '—'}</code></footer>
    </article>

    <aside>
      <article class="panel doctor"><header><strong>doctor</strong><time>{new Date().toLocaleTimeString([], {hour:'2-digit',minute:'2-digit',hour12:false})}</time></header>{#each doctorRows as row}<div class:warn={row.ok === false} class:unknown={row.ok === undefined}><i>{row.ok === true ? '✓' : row.ok === false ? '!' : '·'}</i><span>{row.label}</span>{#if row.action}<button onclick={row.action === 'checkpoint' ? checkpoint : undefined} disabled={row.action !== 'checkpoint'} title={row.action === 'checkpoint' ? 'Выполнить checkpoint через gateway' : 'HTTP-операция пока не предоставлена gateway'}>{row.action} →</button>{/if}</div>{/each}</article>
      <article class="panel recent"><header><strong>ops_log · последние</strong><button onclick={() => go('agents')}>весь журнал →</button></header>{#each operations.slice(0,4) as item}<div><b class={(item.prefix || item.op || '').toLowerCase()}>{item.prefix || item.op}</b><span><strong>{item.agent_name || 'system'}</strong><small>{item.message}</small></span><time>{time(item.ts)}</time></div>{/each}{#if !operations.length}<p>Операций пока нет</p>{/if}</article>
    </aside>
  </section>

  <section class="panel wings"><header><div><strong>Крылья и комнаты</strong><small>ширина = документы · сегменты = комнаты · остальные доступны в Корпусе</small></div></header><div>{#each (taxonomy.wings ?? []).slice(0,4) as wing}<button onclick={() => go('corpus')}><b>{wing.wing}</b><span style={`width:${Math.max(3,wing.document_count/maxWing*100)}%`}>{#each wing.rooms ?? [] as room}<i style={`flex:${room.document_count}`} title={`${room.room}: ${room.document_count}`}></i>{/each}</span><em>{wing.document_count}</em></button>{/each}{#if taxonomy.unscoped_count}<button class="unscoped"><b>без крыла</b><span style={`width:${Math.max(3,taxonomy.unscoped_count/maxWing*100)}%`}></span><em>{taxonomy.unscoped_count}</em></button>{/if}</div></section>
</div>

<style>
  .console-page{display:flex;flex-direction:column;overflow:hidden}.actions{display:flex;gap:7px}.backup{display:flex;gap:8px;margin:-5px 0 12px;margin-left:auto;max-width:620px;width:100%}.backup input{height:32px;flex:1;border:1px solid var(--border);border-radius:7px;background:var(--surface);padding:0 9px;font:11px var(--mono)}.notice{padding:8px;margin-bottom:10px;color:var(--danger);border:1px solid color-mix(in srgb,var(--danger) 30%,transparent);border-radius:7px;font-size:11px}.kpis{display:grid;grid-template-columns:repeat(5,minmax(0,1fr));gap:9px;margin-bottom:11px;flex:0 0 auto}.kpis article{height:91px;padding:10px 12px;background:var(--surface);border:1px solid var(--border);border-radius:11px}.kpis article>span{text-transform:uppercase;letter-spacing:.055em;color:var(--text-muted);font-size:11px;font-weight:700}.kpis strong{display:block;margin:4px 0 1px;font-size:23px;line-height:1.05;letter-spacing:-.04em}.kpis small{font:11px/1.2 var(--mono);color:var(--text-faint)}.kpis .debt{border-color:color-mix(in srgb,var(--warn) 35%,var(--border));background:color-mix(in srgb,var(--warn) 4%,var(--surface))}.kpis .debt strong{color:var(--warn)}.progress{height:3px;background:var(--border);margin:4px 0}.progress.unknown{background:color-mix(in srgb,var(--border) 60%,transparent)}.progress i{display:block;height:100%;background:var(--l3)}.dashboard{display:grid;grid-template-columns:1fr 390px;gap:11px;margin-bottom:11px;flex:1;min-height:0}.layers{min-height:0;overflow:hidden;display:flex;flex-direction:column}.layers>header,.wings>header{height:42px;flex:0 0 42px;padding:0 12px;border-bottom:1px solid var(--border);display:flex;align-items:center}.layers header>div,.wings header>div{display:flex;align-items:baseline;gap:8px}.layers header strong,.wings header strong{font-size:12px}.layers header small,.wings header small{color:var(--text-faint);font-size:11px}.layers>button{width:100%;height:auto;min-height:55px;flex:1;padding:0 12px;border:0;border-bottom:1px solid var(--border);background:linear-gradient(90deg,color-mix(in srgb,var(--layer) 8%,transparent),transparent 40%);color:var(--text);display:grid;grid-template-columns:38px 1fr auto 12px;gap:9px;align-items:center;text-align:left;cursor:pointer}.layers>button:hover{background:linear-gradient(90deg,color-mix(in srgb,var(--layer) 15%,transparent),var(--bg-hover))}.layers>button>b{width:34px;height:27px;display:grid;place-items:center;border:1px solid color-mix(in srgb,var(--layer) 55%,var(--border));border-radius:7px;background:color-mix(in srgb,var(--layer) 16%,var(--surface));box-shadow:inset 3px 0 var(--layer);color:var(--layer);font:800 11px var(--mono)}.layers>button>span{display:flex;flex-direction:column;min-width:0}.layers>button>span strong{font-size:12px;line-height:1.2}.layers>button>span small{font-size:11px;line-height:1.2;color:var(--text-faint);margin-top:2px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.layers>button>em{padding:4px 7px;border:1px solid color-mix(in srgb,var(--layer) 30%,transparent);border-radius:8px;background:color-mix(in srgb,var(--layer) 12%,transparent);color:var(--layer);font:10px var(--mono);font-style:normal;white-space:nowrap}.layers>button>i{color:var(--text-faint);font-style:normal;font-size:13px}.layers>footer{height:35px;flex:0 0 35px;padding:0 11px;display:flex;align-items:center;justify-content:space-between;gap:10px;color:var(--text-faint);font-size:11px;min-width:0}.layers>footer span{min-width:0;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.layers>footer code{max-width:47%;font:10px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.dashboard>aside{display:grid;grid-template-rows:auto 1fr;gap:11px;min-height:0;overflow:hidden}.doctor header,.recent header{height:38px;padding:0 11px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between}.doctor header strong,.recent header strong{font-size:12px}.doctor header time,.recent header button{border:0;background:transparent;color:var(--text-faint);font:10px var(--mono);cursor:pointer}.doctor>div{min-height:28px;padding:0 10px;display:grid;grid-template-columns:18px 1fr auto;gap:5px;align-items:center;border-bottom:1px solid var(--border);font-size:11px}.doctor>div i{color:var(--ok);font-size:12px;font-style:normal}.doctor>div.warn i{color:var(--warn)}.doctor>div.unknown i{color:var(--text-faint)}.doctor>div button{border:0;background:transparent;color:var(--warn);font:10px var(--mono);cursor:pointer}.recent{min-height:0;overflow:hidden}.recent>div{display:grid;grid-template-columns:49px 1fr 52px;gap:7px;padding:6px 10px;border-bottom:1px solid var(--border)}.recent>div>b{align-self:start;font:10px var(--mono);color:var(--l0)}.recent>div>span{display:flex;flex-direction:column;min-width:0}.recent>div>span strong{font-size:11px}.recent>div>span small{color:var(--text-faint);font-size:10px;line-height:1.2;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.recent>div time{font:10px var(--mono);color:var(--text-faint);white-space:nowrap}.recent p{padding:20px;text-align:center;color:var(--text-faint);font-size:11px}.wings{flex:0 0 166px;min-height:0;overflow:hidden}.wings>div{padding:6px 12px}.wings button{width:100%;height:22px;display:grid;grid-template-columns:100px 1fr 48px;gap:8px;align-items:center;border:0;background:transparent;color:var(--text);font-size:11px}.wings button>b{text-align:left;font:11px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.wings button>span{display:flex;height:11px;max-width:100%;border-radius:3px;overflow:hidden;background:color-mix(in srgb,var(--l0) 18%,transparent)}.wings button>span i{background:color-mix(in srgb,var(--l0) 55%,transparent);border-right:1px solid var(--bg)}.wings button>em{text-align:right;color:var(--text-faint);font:11px var(--mono);font-style:normal}.wings .unscoped span{border:1px dashed var(--warn);background:transparent}.wings .unscoped b,.wings .unscoped em{color:var(--warn)}@media(max-width:1050px){.console-page{overflow:auto}.kpis{grid-template-columns:repeat(3,1fr)}.dashboard{grid-template-columns:1fr;flex:none}.dashboard>aside{grid-template-columns:1fr 1fr;grid-template-rows:auto}.wings{display:none}}
</style>
