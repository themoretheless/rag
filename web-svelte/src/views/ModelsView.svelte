<script lang="ts">
  import { onMount } from 'svelte'
  import { api, loadPanels } from '@/api/client'
  import { ui } from '@/lib/state/ui.svelte'

  let llm = $state<any>({})
  let embedding = $state<any>({})
  let status = $state<any>({})
  let runtime = $state<any>({})
  let capabilities = $state<any>({})
  let doctor = $state<any>({})
  let llmKnown = $state(false)
  let embeddingKnown = $state(false)
  let statusKnown = $state(false)
  let runtimeKnown = $state(false)
  let capabilitiesKnown = $state(false)
  let doctorKnown = $state(false)
  let doctorBusy = $state(false)
  let busy = $state(false)
  let error = $state('')

  async function fetchDoctor() {
    const response = await api.get<any>('/v1/doctor')
    doctor = response
    doctorKnown = true
    return response
  }

  async function load() {
    busy = true
    error = ''
    const failures = await loadPanels([
      ['LLM', async () => { llm = (await api.get<any>('/v1/llm-status')).llm ?? {}; llmKnown = true }],
      ['embedding manifest', async () => { embedding = await api.get<any>('/v1/embedding-manifest'); embeddingKnown = true }],
      ['storage status', async () => { status = await api.get<any>('/v1/status'); statusKnown = true }],
      ['runtime', async () => { runtime = (await api.get<any>('/v1/runtime')).runtime ?? {}; runtimeKnown = true }],
      ['capabilities', async () => { capabilities = await api.get<any>('/v1/capabilities'); capabilitiesKnown = true }],
      ['doctor', async () => { await fetchDoctor() }],
    ])
    error = failures.length ? `Не обновлены панели: ${failures.join(', ')}` : ''
    busy = false
  }

  async function checkpoint() {
    try { await api.post('/v1/operations/checkpoint'); ui.toast('Checkpoint завершён', 'ok'); await load() }
    catch (cause) { ui.toast(cause instanceof Error ? cause.message : String(cause), 'error') }
  }

  function summarizeDoctor(report: any, known: boolean) {
    if (!known || typeof report?.ok !== 'boolean') return { ok: null as boolean | null, label: 'целостность неизвестна' }
    if (report.ok) return { ok: true as boolean | null, label: 'целостность ✓' }
    const issues = [
      report.schema_ok === false ? 'schema' : null,
      report.embed_ok === false ? 'embeddings' : null,
      report.relational_integrity_ok === false ? 'связи' : null,
      typeof report.documents_without_chunks === 'number' && report.documents_without_chunks > 0 ? 'без чанков' : null,
    ].filter((item): item is string => item !== null)
    return { ok: false as boolean | null, label: issues.length ? `проблемы: ${issues.join(', ')}` : 'целостность !' }
  }

  async function checkDoctor() {
    doctorBusy = true
    try {
      const response = await fetchDoctor()
      const summary = summarizeDoctor(response, true)
      ui.toast(`Doctor: ${summary.label}`, summary.ok === true ? 'ok' : summary.ok === false ? 'error' : 'info')
    } catch (cause) {
      ui.toast(cause instanceof Error ? cause.message : String(cause), 'error')
    } finally {
      doctorBusy = false
    }
  }

  function bytes(value?: number) {
    if (value == null) return '—'
    const units = ['Б','КБ','МБ','ГБ']; let size = value; let unit = 0
    while (size >= 1024 && unit < units.length - 1) { size /= 1024; unit += 1 }
    return `${size.toFixed(unit > 1 ? 1 : 0)} ${units[unit]}`
  }

  function isLoopbackBind(value: unknown) {
    const bind = String(value ?? '').trim().toLowerCase().replace(/^https?:\/\//, '')
    const host = bind.startsWith('[') ? bind.slice(1, bind.indexOf(']')) : bind.split(':')[0]
    return host === '127.0.0.1' || host === 'localhost' || host === '::1'
  }

  const llmEnabled = $derived(!llmKnown || typeof llm.llm_enabled !== 'boolean' ? null : llm.llm_enabled)
  const manifestState = $derived(!embeddingKnown || typeof embedding.match !== 'boolean' ? 'unknown' : !embedding.manifest ? 'absent' : embedding.match ? 'match' : 'mismatch')
  const ingestRootsConfigured = $derived(!statusKnown || typeof status.ingest_roots_configured !== 'boolean' ? null : status.ingest_roots_configured)
  const ftsReady = $derived(!statusKnown || typeof status.fts_ready !== 'boolean' ? null : status.fts_ready)
  const autoBackupEnabled = $derived(!runtimeKnown || typeof runtime.auto_backup_enabled !== 'boolean' ? null : runtime.auto_backup_enabled)
  const doctorIntegrity = $derived(summarizeDoctor(doctor, doctorKnown))
  const bindKnown = $derived(runtimeKnown && Boolean(runtime.http_bind))
  const loopbackBind = $derived(bindKnown && isLoopbackBind(runtime.http_bind))
  const exposedBind = $derived(bindKnown && !loopbackBind)

  const providers = $derived([
    { id: llm.provider || 'current', name: llmKnown ? (llm.model || 'Текущая модель') : 'Модель не проверена', endpoint: llmKnown ? (llm.base_url || 'не настроено') : 'неизвестно', current: true, state: !llmKnown || typeof llm.reachable !== 'boolean' ? 'не проверена' : llm.reachable ? 'reachable' : llm.error || 'unreachable' },
    { id: 'ollama', name: 'Ollama', endpoint: 'локальный OpenAI-compatible', state: llmKnown && llm.provider === 'ollama' ? (llm.reachable === true ? 'reachable' : llm.reachable === false ? 'unreachable' : 'не проверен') : 'не проверен' },
    { id: 'claude', name: 'Claude', endpoint: 'anthropic_messages', state: llmKnown && llm.provider === 'claude' ? (llm.reachable === true ? 'reachable' : llm.reachable === false ? 'unreachable' : 'не проверен') : 'не проверен' },
    { id: 'openai', name: 'OpenAI / Codex', endpoint: 'openai_compat', state: llmKnown && ['openai','codex'].includes(llm.provider) ? (llm.reachable === true ? 'reachable' : llm.reachable === false ? 'unreachable' : 'не проверен') : 'не проверен' },
    { id: 'custom', name: 'Custom', endpoint: 'LM Studio / vLLM / compatible', state: llmKnown && llm.provider === 'custom' ? (llm.reachable === true ? 'reachable' : llm.reachable === false ? 'unreachable' : 'не проверен') : 'не проверен' },
  ])

  onMount(load)
</script>

<div class="models-page screen">
  <div class="screen-head"><div><h1>Модели и пайплайн</h1><p>Фактическая runtime-конфигурация · изменения RAG_* применяются после перезапуска gateway</p></div><div class="actions"><button class="secondary" onclick={load} disabled={busy}>llm_status · проверить</button><button class="secondary" disabled title="Gateway не предоставляет экспорт секретов">Экспорт .env</button><button class="primary" disabled title="Настройки меняются через окружение и перезапуск">Сохранить и перезапустить</button></div></div>
  {#if error}<div class="notice">{error}</div>{/if}

  <div class="columns">
    <section>
      <article class="panel providers"><header><div><strong>Чат-модель</strong><code>RAG_LLM_PROVIDER</code></div><span class:enabled={llmEnabled === true}>{llmEnabled === null ? 'unknown' : llmEnabled ? 'enabled' : 'disabled'}</span></header><div class="provider-list">{#each providers as provider}<div class:current={provider.current}><i></i><span><b>{provider.name}</b><small>{provider.endpoint}</small></span><em class:ok={provider.state === 'reachable'}>{provider.state}</em></div>{/each}</div><footer>Gateway проверяет только выбранный provider; остальные пресеты показаны как доступные варианты, не как проверенные.</footer></article>
      <article class="panel manifest"><header><div><strong>Эмбеддинги</strong><code>RAG_EMBEDDING_*</code></div><span class:ok={manifestState === 'match'} class:danger={manifestState === 'mismatch'}>{manifestState === 'unknown' ? 'manifest unknown' : manifestState === 'absent' ? 'manifest absent' : manifestState === 'match' ? 'manifest ✓ match' : 'manifest mismatch'}</span></header><dl><dt>provider</dt><dd>{embedding.live?.provider ?? (llmKnown ? llm.embed_provider : null) ?? '—'}</dd><dt>model</dt><dd>{embedding.live?.model ?? (llmKnown ? llm.embed_model : null) ?? '—'}</dd><dt>dims</dt><dd>{embedding.live?.dims ?? (llmKnown ? llm.embed_dims : null) ?? '—'}</dd><dt>base_url</dt><dd>{embedding.live?.base_url ?? (llmKnown ? llm.embed_base_url : null) ?? '—'}</dd><dt>stored</dt><dd>{!embeddingKnown ? 'неизвестно' : embedding.manifest ? `${embedding.manifest.provider} / ${embedding.manifest.model} / ${embedding.manifest.dims}` : 'манифест отсутствует'}</dd></dl><div class="warning">При несовпадении identity режимы vec и hybrid отказывают fail-closed. Lex продолжает работать.</div><div class="buttons"><button class="secondary" disabled>Сменить модель…</button><button class="secondary" disabled>reembed · всё</button></div></article>
    </section>

    <section>
      <article class="panel retrieval"><header><div><strong>Рекомендованный retrieval preset</strong><code>пример · не runtime</code></div><span>API не раскрывает</span></header><div class="segments"><button disabled>lex</button><button disabled>vec</button><button class="active" disabled>hybrid</button></div>{#each [['default_top_k','8','1','100','34'],['rrf_k','60','1','100','60'],['min_score','0.20','0','1','20'],['max chunks / doc','2','1','10','20'],['max context tokens','2 000','100','8000','25'],['recency half-life','30 дн','0','180','17']] as setting}<div class="setting"><span>{setting[0]}</span><b>{setting[1]}</b><i><em style={`width:${setting[4]}%`}></em></i></div>{/each}<div class="setting inline"><span>timeout_ms</span><b>5 000</b></div><div class="stemmer"><span>Варианты RAG_FTS_STEMMER</span><div><button disabled>english</button><button disabled>russian</button><button disabled>уточнить runtime</button></div></div><p>Это рекомендуемый пример интерфейса, а не текущая конфигурация. Фактические defaults и env HTTP API сейчас не возвращает.</p></article>
      <article class="panel chunking"><header><strong>Рекомендованный чанкинг</strong><span>пример · не runtime</span></header><div class="numbers"><span>размер <b>≈800</b></span><span>перекрытие <b>≈120</b></span></div><div class="windows"><i></i><i></i><i></i><em></em><em></em></div><div class="toggle-row"><span>markdown heading_path</span><b>рекомендация</b></div><div class="toggle-row muted"><span>код по функциям</span><b>идея</b></div></article>
    </section>

    <section>
      <article class="panel safety"><header><strong>Безопасность и целостность</strong><span class:danger={exposedBind}>{exposedBind ? 'ВНЕШНИЙ BIND !' : loopbackBind ? 'loopback ✓' : 'bind неизвестен'}</span></header>{#if exposedBind}<div class="danger-banner"><strong>Gateway доступен вне loopback</strong><span>{runtime.http_bind} слушает не только 127.0.0.1/localhost. Ограничьте bind или защитите доступ сетевыми правилами.</span></div>{/if}<div class="root"><b>RAG_INGEST_ROOTS</b><p>{ingestRootsConfigured === null ? 'статус корней неизвестен' : ingestRootsConfigured ? 'корни настроены; произвольные пути запрещены' : 'корни не настроены'}</p></div><div class="toggle-row"><span>wiki if_match</span><b>server policy</b></div><div class="toggle-row"><span>maintain dry_run</span><b>default</b></div><dl><dt>HTTP bind</dt><dd class:danger-text={exposedBind}>{runtimeKnown ? (runtime.http_bind ?? '—') : '—'}</dd><dt>MCP surface</dt><dd>{(capabilitiesKnown ? capabilities.tool_surface : null) ?? (runtimeKnown ? runtime.tool_surface : null) ?? '—'} · {capabilitiesKnown ? (capabilities.tool_count ?? '—') : '—'} tools</dd><dt>API</dt><dd>{capabilitiesKnown ? (capabilities.api_version ?? '—') : '—'}</dd></dl></article>
      <article class="panel storage"><header><div><strong>Хранилище</strong><code>{statusKnown ? (status.backend ?? '—') : '—'} · schema v{statusKnown ? (status.schema_version ?? '—') : '—'}</code></div><span class:ok={ftsReady === true} class:danger={ftsReady === false}>{ftsReady === null ? 'FTS unknown' : ftsReady ? 'FTS ✓' : 'FTS !'}</span></header><code class="path">{statusKnown ? (status.db_path ?? '—') : '—'}</code><div class="storage-metric"><span>База</span><b>{bytes(statusKnown ? status.db_file_bytes : undefined)}</b></div><div class="storage-metric"><span>WAL</span><b>{bytes(statusKnown ? status.wal_bytes : undefined)} / {bytes(statusKnown ? status.wal_warn_bytes : undefined)}</b><i><em style={`width:${statusKnown ? Math.min(100,(status.wal_bytes??0)/(status.wal_warn_bytes||1)*100) : 0}%`}></em></i></div><div class="storage-metric"><span>Автобэкап</span><b>{autoBackupEnabled === null ? 'неизвестно' : autoBackupEnabled ? (runtime.auto_backup_last_completed_at ?? 'включён') : 'выключен'}</b></div><div class:ok={doctorIntegrity.ok === true} class:danger={doctorIntegrity.ok === false} class="doctor-summary"><span>doctor</span><b>{doctorIntegrity.label}</b></div><div class="buttons"><button class="secondary" onclick={checkpoint}>checkpoint</button><button class="secondary" onclick={checkDoctor} disabled={busy || doctorBusy} title="GET /v1/doctor и обновление сводки целостности">{doctorBusy ? 'проверка…' : 'doctor'}</button></div></article>
    </section>
  </div>
</div>

<style>
  .models-page{display:flex;flex-direction:column;overflow:hidden}.actions{display:flex;gap:7px}.columns{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px;flex:1;min-height:0}.columns>section{display:flex;flex-direction:column;gap:12px;min-height:0}.columns>section>.panel:last-child{flex:1}.panel{overflow:hidden}.panel>header{min-height:43px;padding:0 12px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between}.panel>header>div{display:flex;flex-direction:column;gap:2px}.panel header code,.panel header span{font:8px var(--mono);color:var(--text-faint)}.panel header span.enabled,.panel header span.ok{color:var(--ok)}.panel header span.danger{color:var(--danger);font-weight:700}.provider-list>div{min-height:51px;padding:8px 11px;display:grid;grid-template-columns:12px 1fr auto;gap:7px;align-items:center;border-bottom:1px solid var(--border)}.provider-list>div.current{background:color-mix(in srgb,var(--l3) 7%,transparent)}.provider-list>div>i{width:9px;height:9px;border:1px solid var(--text-faint);border-radius:50%}.provider-list>div.current>i{border:3px solid var(--l3)}.provider-list span{display:flex;flex-direction:column}.provider-list b{font-size:10px}.provider-list small{margin-top:2px;color:var(--text-faint);font:7px var(--mono)}.provider-list em{font:7px var(--mono);font-style:normal;color:var(--text-faint)}.provider-list em.ok{color:var(--ok)}.providers footer,.retrieval>p{padding:9px 11px;color:var(--text-faint);font-size:8px;line-height:1.45}.manifest dl,.safety dl{display:grid;grid-template-columns:75px 1fr;gap:6px;padding:11px;margin:0;font-size:8px}.manifest dt,.safety dt{color:var(--text-faint)}.manifest dd,.safety dd{margin:0;font-family:var(--mono);word-break:break-all}.safety dd.danger-text{color:var(--danger);font-weight:700}.warning{margin:0 11px 10px;padding:8px;border:1px solid color-mix(in srgb,var(--warn) 35%,transparent);border-radius:6px;background:color-mix(in srgb,var(--warn) 6%,transparent);color:var(--warn);font-size:8px;line-height:1.45}.danger-banner{display:flex;flex-direction:column;gap:3px;margin:10px 11px 0;padding:8px;border:1px solid color-mix(in srgb,var(--danger) 50%,transparent);border-radius:6px;background:color-mix(in srgb,var(--danger) 9%,transparent);color:var(--danger);font-size:8px;line-height:1.45}.danger-banner strong{font-size:9px}.buttons{display:flex;gap:6px;padding:0 11px 11px}.buttons .secondary{flex:1}.segments{display:flex;margin:10px 11px;border:1px solid var(--border);border-radius:6px;overflow:hidden}.segments button,.stemmer button{flex:1;height:27px;border:0;border-right:1px solid var(--border);background:transparent;color:var(--text-faint);font:8px var(--mono)}.segments button.active{background:color-mix(in srgb,var(--l1) 13%,transparent);color:var(--l1)}.setting{display:grid;grid-template-columns:1fr auto;gap:5px 8px;margin:8px 11px;font-size:9px}.setting>b{font:8px var(--mono)}.setting>i,.storage-metric>i{grid-column:1/3;height:3px;background:var(--border)}.setting>i em,.storage-metric>i em{display:block;height:100%;background:var(--l1)}.setting.inline{padding-bottom:8px;border-bottom:1px solid var(--border)}.stemmer{padding:2px 11px}.stemmer>span{font-size:8px;color:var(--text-faint)}.stemmer>div{display:flex;margin-top:6px;border:1px solid var(--border);border-radius:6px;overflow:hidden}.numbers{display:flex;gap:22px;padding:11px;font-size:9px}.numbers b{font:9px var(--mono)}.windows{position:relative;height:54px;margin:0 11px 9px}.windows i{position:absolute;width:48%;height:27px;border-radius:5px;background:color-mix(in srgb,var(--l0) 30%,transparent);border:1px solid color-mix(in srgb,var(--l0) 45%,transparent)}.windows i:nth-child(2){left:26%;top:13px}.windows i:nth-child(3){right:0;top:26px}.windows em{position:absolute;left:26%;top:13px;width:22%;height:27px;background:color-mix(in srgb,var(--l1) 35%,transparent)}.windows em:nth-of-type(2){left:auto;right:26%;top:26px}.toggle-row{display:flex;align-items:center;justify-content:space-between;min-height:35px;padding:0 11px;border-top:1px solid var(--border);font-size:9px}.toggle-row b{font:8px var(--mono);color:var(--ok)}.toggle-row.muted{color:var(--text-faint)}.root{padding:11px;border-bottom:1px solid var(--border)}.root b{font:8px var(--mono)}.root p{margin:5px 0 0;color:var(--text-faint);font-size:8px}.path{display:block;margin:10px 11px;padding:7px;border-radius:5px;background:#090b10;color:var(--text-faint);font:7px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.storage-metric{display:grid;grid-template-columns:1fr auto;gap:5px;padding:7px 11px;font-size:9px}.storage-metric>b{font:8px var(--mono);max-width:180px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.doctor-summary{display:flex;align-items:center;justify-content:space-between;gap:8px;margin:4px 11px 10px;padding:7px;border:1px solid var(--border);border-radius:6px;color:var(--text-faint);font-size:9px}.doctor-summary b{font:8px var(--mono);font-weight:500;text-align:right}.doctor-summary.ok{border-color:color-mix(in srgb,var(--ok) 35%,var(--border));color:var(--ok)}.doctor-summary.danger{border-color:color-mix(in srgb,var(--danger) 35%,var(--border));color:var(--danger)}.notice{padding:8px;color:var(--danger)}@media(max-width:1100px){.models-page{overflow:auto}.columns{grid-template-columns:1fr 1fr;flex:none}.columns>section:last-child{grid-column:1/3;display:grid;grid-template-columns:1fr 1fr}}@media(max-width:760px){.columns{grid-template-columns:1fr}.columns>section:last-child{grid-column:auto;display:flex}}
  .panel header code,.panel header span{font-size:10px}.provider-list b{font-size:12px}.provider-list small,.provider-list em{font-size:9px}.providers footer,.retrieval>p{font-size:10px}.manifest dl,.safety dl{font-size:10.5px}.warning,.danger-banner{font-size:10px}.danger-banner strong{font-size:11px}.segments button,.stemmer button{font-size:10px}.setting{font-size:11px}.setting>b{font-size:10px}.stemmer>span{font-size:10px}.numbers,.numbers b{font-size:11px}.toggle-row{font-size:11px}.toggle-row b{font-size:10px}.root b{font-size:10px}.root p{font-size:10px}.path{font-size:9px}.storage-metric{font-size:11px}.storage-metric>b{font-size:10px}
</style>
