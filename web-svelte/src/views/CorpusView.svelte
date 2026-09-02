<script lang="ts">
  import { onMount } from 'svelte'
  import { api, apiUrl, loadPanels } from '@/api/client'
  import { goGraph, route } from '@/lib/router.svelte'
  import { ui } from '@/lib/state/ui.svelte'

  type Project = { project_id: string; document_count: number; rooms: { room: string; document_count: number }[] }
  type Row = { id: string; uri: string; title: string; wing?: string | null; room?: string | null; source_file?: string | null; layer: string; kind: string; status: string; revision?: number; pinned?: boolean; boost?: number; content_hash?: string | null; updated_at: string }
  type DetailDocument = Row & { revision: number; metadata_json: string; content: string }
  type DocumentDetail = { document: DetailDocument; chunks?: { id: string; chunk_index: number; content: string; char_start: number; char_end: number }[]; chunks_total?: number; chunks_truncated?: boolean }
  type DocumentPage = { items?: Row[]; page?: { total?: number; next_cursor?: string | null } }
  type EmbeddingStatus = { match?: boolean; live?: { provider?: string; model?: string; dims?: number }; manifest?: { provider?: string; model?: string; dims?: number } }
  type Taxonomy = { unscoped_count?: number; total_documents?: number }

  let projects = $state<Project[]>([])
  let rows = $state<Row[]>([])
  let total = $state(0)
  let project = $state('')
  let room = $state('')
  let layer = $state('')
  let status = $state('')
  let query = $state('')
  let selected = $state<Row | null>(null)
  let detail = $state<DocumentDetail | null>(null)
  let busy = $state(false)
  let detailBusy = $state(false)
  let detailError = $state('')
  let documentError = $state('')
  let appendError = $state('')
  let panelWarning = $state('')
  let nextCursor = $state<string | null>(null)
  let appliedFilterSignature = $state('')
  let loadingMore = $state(false)
  let syncPath = $state('')
  let syncBusy = $state(false)
  let projectHome = $state<any>(null)
  let embeddingStatus = $state<EmbeddingStatus | null>(null)
  let taxonomy = $state<Taxonomy>({})
  let backlinks = $state<{ id: string; label: string }[]>([])
  let showText = $state(false)
  let dialogEl: HTMLDivElement | null = $state(null)
  let pendingSelectionId = $state('')
  let appliedRoute = $state<string | null>(null)
  let documentRequestId = 0
  let detailRequestId = 0
  let projectHomeRequestId = 0
  let refreshGeneration = 0

  const detailDocument = $derived(detail?.document ?? null)
  const scopedDocumentCount = $derived(projects.reduce((sum, item) => sum + item.document_count, 0))
  const allDocumentCount = $derived(taxonomy.total_documents ?? (scopedDocumentCount + (taxonomy.unscoped_count ?? 0)))
  const filtersDirty = $derived.by(() => Boolean(appliedFilterSignature) && documentQuery() !== appliedFilterSignature)

  function documentQuery() {
    const params = new URLSearchParams({ limit: '200' })
    if (project) params.set('wing', project)
    if (room) params.set('room', room)
    if (layer) params.set('layer', layer)
    if (status) params.set('status', status)
    if (status === 'archived') params.set('include_archived', 'true')
    if (query.trim()) params.set('q', query.trim())
    return params.toString()
  }

  async function loadProjects(generation?: number) {
    const response = await api.get<{ items: Project[] }>('/v1/projects')
    if (generation != null && generation !== refreshGeneration) return
    projects = response.items ?? []
  }

  async function loadEmbeddingStatus(generation?: number) {
    const response = await api.get<EmbeddingStatus>('/v1/embedding-manifest')
    if (generation != null && generation !== refreshGeneration) return
    embeddingStatus = response
  }

  async function loadTaxonomy(generation?: number) {
    const response = await api.get<{ taxonomy?: Taxonomy }>('/v1/taxonomy')
    if (generation != null && generation !== refreshGeneration) return
    taxonomy = response.taxonomy ?? {}
  }

  async function loadDocuments(append = false, generation?: number) {
    const cursor = append ? nextCursor : null
    const requestSignature = documentQuery()
    if (append && (!cursor || requestSignature !== appliedFilterSignature)) return
    const requestQuery = new URLSearchParams(requestSignature)
    if (cursor) requestQuery.set('cursor', cursor)
    const requestId = ++documentRequestId
    if (append) {
      loadingMore = true
      appendError = ''
    } else {
      busy = true
      documentError = ''
      appendError = ''
    }
    try {
      const response = await api.get<DocumentPage>(`/v1/documents?${requestQuery.toString()}`)
      if (requestId !== documentRequestId || (generation != null && generation !== refreshGeneration)) return
      if (append && requestSignature !== documentQuery()) return
      const incoming = response.items ?? []
      rows = append
        ? [...new Map([...rows, ...incoming].map((row) => [row.id, row])).values()]
        : incoming
      total = response.page?.total ?? rows.length
      nextCursor = response.page?.next_cursor ?? null
      if (!append) appliedFilterSignature = requestSignature
      if (!append && selected && !rows.some((row: Row) => row.id === selected?.id)) {
        selected = null
        detail = null
      }
      if (!append && pendingSelectionId) {
        const targetId = pendingSelectionId
        const target = rows.find((row) => row.id === targetId)
        if (target) {
          pendingSelectionId = ''
          void choose(target)
        } else {
          void loadRoutedDocument(targetId, requestId)
        }
      } else if (!selected && rows.length) {
        void choose(rows[0])
      }
    } catch (cause) {
      if (requestId === documentRequestId && (generation == null || generation === refreshGeneration)) {
        const message = cause instanceof Error ? cause.message : String(cause)
        if (append) appendError = message
        else documentError = message
      }
    } finally {
      if (requestId === documentRequestId && (generation == null || generation === refreshGeneration)) {
        busy = false
        loadingMore = false
      }
    }
  }

  async function loadRoutedDocument(documentId: string, catalogRequestId: number) {
    try {
      const response = await api.post<{ items?: DocumentDetail[] }>('/v1/multi-get', {
        document_ids: [documentId],
        include_chunks: false,
      })
      if (catalogRequestId !== documentRequestId || pendingSelectionId !== documentId) return
      const routed = response.items?.[0]?.document
      if (!routed) {
        pendingSelectionId = ''
        panelWarning = `Документ ${documentId} не найден`
        return
      }
      rows = [routed, ...rows.filter((row) => row.id !== routed.id)]
      pendingSelectionId = ''
      void choose(routed)
    } catch (cause) {
      if (catalogRequestId === documentRequestId && pendingSelectionId === documentId) {
        pendingSelectionId = ''
        panelWarning = `Не удалось открыть документ по ссылке: ${cause instanceof Error ? cause.message : String(cause)}`
      }
    }
  }

  async function loadAll() {
    const generation = ++refreshGeneration
    const selectedAtStart = selected?.id ?? ''
    panelWarning = ''
    const failures = await loadPanels([
      ['проекты', () => loadProjects(generation)],
      ['документы', () => loadDocuments(false, generation)],
      ['taxonomy', () => loadTaxonomy(generation)],
      ['манифест эмбеддингов', () => loadEmbeddingStatus(generation)],
    ])
    if (generation !== refreshGeneration) return
    if (failures.length) panelWarning = `Не обновлены панели: ${failures.join(', ')}`
    if (selectedAtStart && selected?.id === selectedAtStart) await choose(selected, generation)
  }

  async function choose(row: Row, generation?: number) {
    const requestId = ++detailRequestId
    selected = row
    detail = null
    backlinks = []
    showText = false
    detailBusy = true
    detailError = ''
    try {
      const [response, backlinkResponse] = await Promise.all([
        api.post<any>('/v1/multi-get', { document_ids: [row.id], include_chunks: true, chunk_limit: 80 }),
        api.backlinks(row.id).catch(() => null),
      ])
      if (requestId === detailRequestId && selected?.id === row.id && (generation == null || generation === refreshGeneration)) {
        detail = response.items?.[0] ?? null
        backlinks = backlinkResponse?.backlinks ?? []
      }
    } catch (cause) {
      if (requestId === detailRequestId && selected?.id === row.id && (generation == null || generation === refreshGeneration)) {
        detailError = cause instanceof Error ? cause.message : String(cause)
        ui.toast(detailError, 'error')
      }
    } finally {
      if (requestId === detailRequestId && selected?.id === row.id && (generation == null || generation === refreshGeneration)) detailBusy = false
    }
  }

  function chooseScope(nextProject: string, nextRoom = '') {
    const requestId = ++projectHomeRequestId
    project = nextProject
    room = nextRoom
    projectHome = null
    if (nextProject) {
      void api.get<any>(`/v1/project-home?project=${encodeURIComponent(nextProject)}`).then((response) => {
        if (requestId === projectHomeRequestId && project === nextProject && room === nextRoom) projectHome = response.project ?? null
      }).catch(() => {})
    }
    void loadDocuments()
  }

  async function startSync() {
    if (!syncPath.trim()) return
    syncBusy = true
    try {
      const response = await api.post<any>('/v1/jobs/sync', { path: syncPath.trim(), wing: project || undefined, room: room || undefined })
      ui.toast(`Sync поставлен в очередь · ${response.id ?? response.job?.id ?? ''}`, 'ok')
    } catch (cause) {
      ui.toast(cause instanceof Error ? cause.message : String(cause), 'error')
    } finally {
      syncBusy = false
    }
  }

  function shortDate(value: string) {
    if (!value) return '—'
    return value.replace('T', ' ').slice(0, 16)
  }

  function relativeDate(value: string) {
    const timestamp = Date.parse(value)
    if (!Number.isFinite(timestamp)) return shortDate(value)
    const minutes = Math.max(0, Math.floor((Date.now() - timestamp) / 60_000))
    if (minutes < 60) return `${minutes} мин назад`
    const hours = Math.floor(minutes / 60)
    if (hours < 24) return `${hours} ч назад`
    const days = Math.floor(hours / 24)
    if (days < 7) return `${days} дн назад`
    return value.slice(0, 10)
  }

  function shortHash(value?: string | null) {
    if (!value) return '—'
    return value.length > 24 ? `${value.slice(0, 12)}…${value.slice(-6)}` : value
  }

  function contentSize(value?: string | null) {
    if (!value) return '—'
    const bytes = new TextEncoder().encode(value).length
    return `${new Intl.NumberFormat('ru-RU').format(bytes)} байт`
  }

  $effect(() => {
    if (route.name !== 'corpus') return
    const signature = route.query.toString()
    if (signature === appliedRoute) return
    appliedRoute = signature
    const routedDocumentId = route.query.get('document_id')?.trim() ?? ''
    project = route.query.get('project')?.trim() ?? ''
    room = route.query.get('room')?.trim() ?? ''
    query = route.query.get('q')?.trim() ?? ''
    pendingSelectionId = routedDocumentId
    void loadDocuments()
  })

  $effect(() => {
    if (showText) requestAnimationFrame(() => dialogEl?.focus())
  })

  function onWindowKeydown(event: KeyboardEvent) {
    if (showText && event.key === 'Escape') {
      event.preventDefault()
      showText = false
    }
  }

  onMount(loadAll)
</script>

<svelte:window onkeydown={onWindowKeydown} />

<div class="corpus">
  <aside class="palace">
    <header><strong>Крылья · комнаты</strong><span>{allDocumentCount}</span></header>
    <button class:active={!project} class="scope all" onclick={() => chooseScope('')}><span>Все документы</span><b>{allDocumentCount}</b></button>
    <div class="scope-list">
      {#each projects as item (item.project_id)}
        <button class="scope wing" class:active={project === item.project_id && !room} onclick={() => chooseScope(item.project_id)}><span>▾ {item.project_id}</span><b>{item.document_count}</b></button>
        {#if project === item.project_id}
          {#each item.rooms as child (child.room)}
            <button class="scope room" class:active={room === child.room} onclick={() => chooseScope(item.project_id, child.room)}><span>{child.room}</span><b>{child.document_count}</b></button>
          {/each}
        {/if}
      {/each}
      <button class="scope unscoped" disabled title="Документы без крыла входят в «Все документы»; отдельный фильтр по пустому wing пока не поддерживается HTTP API"><span>◌ без крыла</span><b>{taxonomy.unscoped_count ?? 0}</b></button>
    </div>
    <section class="sync">
      <strong>Корень ingest · sync</strong>
      <p>Gateway читает только разрешённые пути из <code>RAG_INGEST_ROOTS</code>.</p>
      {#each projectHome?.source_roots ?? [] as root}<small>{root.canonical_root} · {root.file_count} файлов</small>{/each}
      <input bind:value={syncPath} placeholder="/разрешённый/путь" />
      <button class="primary" disabled={syncBusy || !syncPath.trim()} onclick={startSync}>{syncBusy ? 'Запуск…' : 'sync_sources'}</button>
    </section>
  </aside>

  <section class="catalog">
    <div class="screen-head">
      <div><h1>{room || project || 'Корпус'}</h1><p>{total} документов · immutable raw и производные слои</p></div>
      <button class="secondary" onclick={loadAll}>Обновить</button>
    </div>
    {#if panelWarning}<div class="panel-warning" role="status">{panelWarning}</div>{/if}
    <section class="ingest-zone"><div><strong>Индексировать разрешённую папку</strong><p><code>sync_sources</code> · дубли по content_hash пропускаются · чанки и граф обновляются одним writer</p></div><div class="connectors"><span class="native">Markdown · Obsidian</span><span class="native">Код · репозитории</span><span class="native">TXT · JSON · TOML</span><span class="external">PDF · pdftotext</span><span class="planned">Notion · экспорт md</span><span class="planned">Картинки · OCR</span></div></section>
    <div class="toolbar">
      <input bind:value={query} onkeydown={(event) => event.key === 'Enter' && loadDocuments()} placeholder="Фильтр по названию, URI или пути…" />
      <div class="layer-tabs" aria-label="Слой документа">{#each [['','все'],['raw','raw'],['wiki','wiki']] as option}<button class:active={layer === option[0]} onclick={() => { layer = option[0]; void loadDocuments() }}>{option[1]}</button>{/each}</div>
      <select bind:value={status} onchange={() => void loadDocuments()}><option value="">активные</option><option>active</option><option>draft</option><option>archived</option></select>
      <select disabled title="Сервер возвращает каталог в каноническом порядке"><option>обновлён ↓</option></select>
      <button class="secondary" onclick={() => void loadDocuments()}>Найти</button>
    </div>
    <div class="table panel">
      <div class="thead"><span>Документ</span><span>Тип</span><span>Крыло / комната</span><span>Чанки</span><span title="Ревизия">Rev.</span><span>Обновлён</span><span>Статус</span></div>
      <div class="tbody">
        {#if documentError}<div class="empty error">{documentError}</div>{:else if busy}<div class="empty">Загрузка корпуса…</div>{:else if !rows.length}<div class="empty">В этом срезе нет документов</div>{:else}
          {#each rows as row (row.id)}
            <button class="tr" class:selected={selected?.id === row.id} onclick={() => choose(row)}>
              <span><b>{row.title || row.uri}</b><small>{row.uri}</small></span><span><i>{row.kind || row.layer}</i></span><span>{[row.wing, row.room].filter(Boolean).join(' / ') || '— / —'}</span><span class="mono">{selected?.id === row.id ? (detail?.chunks_total ?? detail?.chunks?.length ?? '…') : '—'}</span><span class="mono">{selected?.id === row.id && detailDocument ? `r${detailDocument.revision}` : '—'}</span><span class="mono">{relativeDate(row.updated_at)}</span><span class={`status ${row.status}`}>{row.status}</span>
            </button>
          {/each}
          {#if nextCursor}
            <div class="load-more-row">
              <button class="load-more" disabled={loadingMore || filtersDirty} onclick={() => loadDocuments(true)}>{loadingMore ? 'Загрузка…' : filtersDirty ? 'Примените фильтр, чтобы продолжить' : `Загрузить ещё · показано ${rows.length} из ${total}`}</button>
              {#if appendError}<div class="append-error" role="alert"><span>{appendError}</span><button disabled={loadingMore || filtersDirty} onclick={() => loadDocuments(true)}>Повторить</button></div>{/if}
            </div>
          {/if}
        {/if}
      </div>
    </div>
  </section>

  {#if selected}
    <aside class="inspector">
      <header><span><i>{detailDocument?.layer ?? selected.layer}</i><b>{detailDocument?.kind ?? selected.kind}</b>{#if detailDocument?.pinned}<em>pinned</em>{/if}</span><button aria-label="Закрыть инспектор" onclick={() => { detailRequestId += 1; selected = null; detail = null; backlinks = []; detailBusy = false; detailError = ''; showText = false }}>×</button></header>
      <h2>{detailDocument?.title ?? selected.title}</h2><code>{detailDocument?.uri ?? selected.uri}</code>
      <dl><dt>content_hash</dt><dd title={detailDocument?.content_hash ?? undefined}>{shortHash(detailDocument?.content_hash)}</dd><dt>размер</dt><dd>{contentSize(detailDocument?.content)} · {detail?.chunks_total ?? detail?.chunks?.length ?? '—'} чанков</dd><dt>эмбеддинги</dt><dd>{embeddingStatus?.live?.model ?? embeddingStatus?.manifest?.model ?? '—'} · {embeddingStatus?.live?.dims ?? embeddingStatus?.manifest?.dims ?? '—'}d {embeddingStatus?.match ? '✓ манифест' : '· проверить'}</dd><dt>wing / room</dt><dd>{detailDocument ? [detailDocument.wing, detailDocument.room].filter(Boolean).join(' / ') || '—' : '—'}</dd><dt>source_file</dt><dd>{detailDocument?.source_file || '—'}</dd><dt>boost · status</dt><dd>{detailDocument?.boost ?? '—'} · {detailDocument?.status ?? '—'}</dd><dt>revision</dt><dd>{detailDocument ? `r${detailDocument.revision}` : '—'}</dd><dt>обновлён</dt><dd>{detailDocument ? shortDate(detailDocument.updated_at) : '—'}</dd></dl>
      <section class="chunks"><div><strong>Карта чанков</strong><span>{detail?.chunks_total ?? detail?.chunks?.length ?? '—'}</span></div>{#if detailBusy}<p>Загрузка…</p>{:else if detailError}<div class="detail-error" role="alert"><span>{detailError}</span><button onclick={() => selected && choose(selected)}>Повторить</button></div>{:else if detail?.chunks?.length}{#each detail.chunks.slice(0, 12) as chunk}<button><b>#{chunk.chunk_index}</b><span>{chunk.content}</span><small>{chunk.char_start}–{chunk.char_end}</small></button>{/each}{:else}<p>Чанки отсутствуют</p>{/if}</section>
      <section class="backlinks"><div><strong>Обратные ссылки</strong><span>{backlinks.length}</span></div>{#if backlinks.length}<div class="backlink-list">{#each backlinks.slice(0, 8) as backlink}<button onclick={() => goGraph(backlink.id, { project: selected?.wing ?? '' })}>↗ {backlink.label}</button>{/each}</div>{:else}<p>Обратных ссылок нет</p>{/if}</section>
      <div class="inspector-actions"><button class="secondary" disabled title="reembed_document пока доступен только через MCP">reembed_document</button><button class="secondary" disabled title="refile пока доступен только через MCP">refile · переложить</button><button class="secondary" disabled title="Архивация raw-документа пока недоступна в HTTP API">В архив</button><button class="secondary" onclick={() => selected?.id && goGraph(selected.id, { project: selected.wing ?? '' })}>Показать в графе</button>{#if detailDocument?.source_file}<a class="secondary" href={apiUrl(`/v1/source-file?document_id=${encodeURIComponent(detailDocument.id)}`)}>Открыть источник</a>{:else}<button class="secondary" disabled>Источник недоступен</button>{/if}<button class="primary text-action" disabled={!detailDocument?.content} onclick={() => showText = true}>Открыть текст · {contentSize(detailDocument?.content)}</button></div>
    </aside>
  {/if}
</div>

{#if showText && detailDocument}
  <div class="text-modal"><button class="backdrop" aria-label="Закрыть текст" onclick={() => showText = false}></button><div bind:this={dialogEl} class="document-dialog" role="dialog" aria-modal="true" aria-labelledby="document-text-title" tabindex="-1"><header><div><span>{detailDocument.layer} · {detailDocument.kind}</span><h2 id="document-text-title">{detailDocument.title}</h2><code>{detailDocument.uri}</code></div><button aria-label="Закрыть" onclick={() => showText = false}>×</button></header><pre>{detailDocument.content}</pre></div></div>
{/if}

<style>
  .corpus{height:100%;min-height:0;display:flex;background:var(--bg);overflow:hidden}.palace{width:196px;flex:0 0 196px;border-right:1px solid var(--border);background:var(--bg-sidebar);display:flex;flex-direction:column;min-height:0}.palace>header{height:44px;padding:0 12px;display:flex;align-items:center;justify-content:space-between;border-bottom:1px solid var(--border)}.palace>header strong{font-size:11px;white-space:nowrap}.palace header span{font:9px var(--mono);color:var(--text-faint)}.scope-list{overflow:auto}.scope{width:100%;min-height:35px;padding:0 12px;border:0;background:transparent;color:var(--text-muted);display:flex;align-items:center;justify-content:space-between;text-align:left;cursor:pointer}.scope:hover,.scope.active{background:var(--bg-hover);color:var(--text)}.scope:disabled{cursor:not-allowed;opacity:.65}.scope.active{box-shadow:inset 2px 0 var(--l0)}.scope span{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.scope b{font:9px var(--mono);color:var(--text-faint)}.scope.room{padding-left:29px;font-size:11px}.scope.unscoped{color:var(--warn);border-top:1px solid var(--border)}.sync{margin-top:auto;padding:12px;border-top:1px solid var(--border)}.sync strong{font-size:11px}.sync p{color:var(--text-faint);font-size:9px;line-height:1.45}.sync code{font-family:var(--mono)}.sync input{width:100%;height:30px;border:1px solid var(--border);border-radius:7px;background:var(--surface);padding:0 8px;font:9px var(--mono);margin-bottom:7px}.sync button{width:100%}.catalog{flex:1;min-width:0;padding:18px;display:flex;flex-direction:column}.screen-head{margin-bottom:12px}.toolbar{display:flex;gap:7px;margin-bottom:10px}.toolbar input{flex:1;min-width:150px}.toolbar input,.toolbar select{height:32px;border:1px solid var(--border);border-radius:7px;background:var(--surface);padding:0 9px;font-size:10px}.layer-tabs{height:32px;display:flex;border:1px solid var(--border);border-radius:7px;overflow:hidden}.layer-tabs button{min-width:34px;border:0;border-right:1px solid var(--border);background:var(--surface);color:var(--text-faint);font:9px var(--mono);cursor:pointer}.layer-tabs button.active{background:color-mix(in srgb,var(--l0) 14%,var(--surface));color:var(--l0)}.table{display:flex;flex-direction:column;min-height:0;flex:1}.thead,.tr{display:grid;grid-template-columns:minmax(170px,2fr) 66px minmax(100px,1fr) 42px 46px 82px 61px;gap:7px;align-items:center;padding:0 10px}.thead{height:36px;flex:0 0 36px;border-bottom:1px solid var(--border);color:var(--text-faint);font-size:8px;text-transform:uppercase;letter-spacing:.04em}.tbody{overflow:auto}.tr{width:100%;min-height:53px;border:0;border-bottom:1px solid var(--border);background:transparent;color:var(--text-muted);text-align:left;cursor:pointer}.tr:hover,.tr.selected{background:var(--bg-hover)}.tr.selected{box-shadow:inset 2px 0 var(--l0)}.tr>span:first-child{display:flex;flex-direction:column;min-width:0}.tr>span:nth-child(6){white-space:nowrap}.tr b{color:var(--text);font-size:11px}.tr small{color:var(--text-faint);font:8px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;margin-top:3px}.tr i{font:8px var(--mono);font-style:normal;color:var(--l0);padding:3px 4px;border-radius:4px;background:color-mix(in srgb,var(--l0) 12%,transparent)}.status{font:8px var(--mono);color:var(--ok)}.status.archived{color:var(--text-faint)}.status.draft{color:var(--warn)}.empty{padding:60px;text-align:center;color:var(--text-faint)}.error{color:var(--danger)}.inspector{width:330px;flex:0 0 330px;border-left:1px solid var(--border);background:var(--surface);padding:14px;overflow:auto}.inspector header{display:flex;align-items:center;justify-content:space-between}.inspector header span{display:flex;gap:5px}.inspector header i,.inspector header b,.inspector header em{font:9px var(--mono);font-style:normal;padding:3px 6px;border-radius:4px;background:var(--bg-hover)}.inspector header i{color:var(--l0)}.inspector header em{color:var(--star)}.inspector header button{border:0;background:transparent;color:var(--text-faint);font-size:18px;cursor:pointer}.inspector h2{font-size:17px;margin:14px 0 5px;line-height:1.25}.inspector>code{display:block;color:var(--text-faint);font:8px/1.4 var(--mono);word-break:break-all}.inspector dl{display:grid;grid-template-columns:90px 1fr;gap:7px;margin:16px 0;font-size:9px}.inspector dt{color:var(--text-faint)}.inspector dd{margin:0;font-family:var(--mono);word-break:break-word}.chunks,.backlinks{border-top:1px solid var(--border);padding-top:12px}.chunks>div,.backlinks>div:first-child{display:flex;justify-content:space-between}.chunks>div span,.backlinks>div span{font:9px var(--mono);color:var(--text-faint)}.chunks>button{width:100%;display:grid;grid-template-columns:24px 1fr;gap:5px;border:0;border-bottom:1px solid var(--border);background:transparent;padding:8px 0;text-align:left}.chunks button>b{font:9px var(--mono);color:var(--l1)}.chunks button>span{font-size:9px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.chunks button>small{grid-column:2;color:var(--text-faint);font:8px var(--mono)}.chunks p,.backlinks p{color:var(--text-faint);font-size:10px}.backlinks{margin-top:12px}.backlink-list{display:flex!important;justify-content:flex-start!important;flex-wrap:wrap;gap:5px;margin-top:8px}.backlink-list button{border:0;border-radius:999px;background:color-mix(in srgb,var(--l3) 14%,transparent);color:var(--l3);padding:4px 7px;font-size:9px;cursor:pointer}.inspector-actions{display:grid;grid-template-columns:1fr 1fr;gap:7px;margin-top:14px}.inspector-actions a{display:grid;place-items:center;text-decoration:none}.text-action{grid-column:1 / -1}.text-modal{position:fixed;inset:0;z-index:100;display:grid;place-items:center;padding:42px}.backdrop{position:absolute;inset:0;width:100%;height:100%;border:0;background:rgba(3,5,9,.76);backdrop-filter:blur(4px)}.document-dialog{position:relative;width:min(1040px,100%);height:min(760px,100%);display:flex;flex-direction:column;border:1px solid var(--border-strong);border-radius:14px;background:var(--surface);box-shadow:var(--shadow);overflow:hidden}.text-modal header{display:flex;justify-content:space-between;gap:16px;padding:16px 18px;border-bottom:1px solid var(--border)}.text-modal header span{color:var(--l0);font:10px var(--mono)}.text-modal h2{margin:3px 0;font-size:18px}.text-modal header code{color:var(--text-faint);font:9px var(--mono);word-break:break-all}.text-modal header button{align-self:flex-start;border:0;background:transparent;color:var(--text-muted);font-size:22px;cursor:pointer}.text-modal pre{flex:1;min-height:0;overflow:auto;margin:0;padding:20px;white-space:pre-wrap;color:var(--text-muted);font:12px/1.65 var(--mono)}@media(max-width:1050px){.inspector{position:absolute;right:0;top:48px;bottom:0;box-shadow:var(--shadow)}.palace{width:196px;flex-basis:196px}}@media(max-width:760px){.palace{display:none}.thead,.tr{grid-template-columns:1fr 70px 80px}.thead span:nth-child(n+4),.tr>span:nth-child(n+4),.thead span:nth-child(3),.tr>span:nth-child(3){display:none}}
  .sync small{display:block;margin:4px 0;color:var(--text-faint);font:7px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.ingest-zone{display:grid;grid-template-columns:minmax(220px,.75fr) 1.25fr;gap:12px;padding:10px 12px;margin-bottom:9px;border:1px dashed color-mix(in srgb,var(--l0) 45%,var(--border));border-radius:9px;background:color-mix(in srgb,var(--l0) 4%,transparent)}.ingest-zone strong{font-size:10px}.ingest-zone p{margin:4px 0 0;color:var(--text-faint);font-size:8px}.ingest-zone code{font-family:var(--mono);color:var(--l0)}.connectors{display:flex;align-items:center;align-content:center;flex-wrap:wrap;gap:4px}.connectors span{padding:3px 6px;border:1px solid var(--border);border-radius:8px;color:var(--text-faint);font-size:7px}.connectors .native{color:var(--ok);border-color:color-mix(in srgb,var(--ok) 35%,transparent)}.connectors .external{color:var(--warn);border-color:color-mix(in srgb,var(--warn) 35%,transparent)}.connectors .planned{border-style:dashed}
  .panel-warning{margin:-4px 0 8px;padding:6px 9px;border:1px solid color-mix(in srgb,var(--warn) 35%,var(--border));border-radius:7px;color:var(--warn);font:9px var(--mono)}.load-more-row{border-top:1px solid var(--border);background:var(--surface-2)}.load-more{width:100%;height:34px;border:0;background:transparent;color:var(--l0);font:9px var(--mono);cursor:pointer}.load-more:disabled{color:var(--text-faint);cursor:not-allowed}.append-error{display:flex;align-items:center;justify-content:space-between;gap:10px;padding:0 10px 8px;color:var(--danger);font:9px var(--mono)}.append-error span{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}.append-error button{flex:0 0 auto;border:0;background:transparent;color:var(--l0);font:inherit;cursor:pointer}.append-error button:disabled{color:var(--text-faint);cursor:not-allowed}
  .palace header span,.scope b{font-size:10.5px}.sync p{font-size:10px}.sync input{font-size:10.5px}.toolbar input,.toolbar select{font-size:11.5px}.layer-tabs button{font-size:10.5px}.thead{font-size:10px}.tr b{font-size:12.5px}.tr small{font-size:9.5px}.tr i,.status{font-size:10px}.ingest-zone strong{font-size:11.5px}.ingest-zone p{font-size:10px}.connectors span{font-size:9px}.inspector header i,.inspector header b,.inspector header em{font-size:10.5px}.inspector>code{font-size:10px}.inspector dl{font-size:11px}.chunks>div span,.backlinks>div span{font-size:10.5px}.chunks button>b{font-size:10.5px}.chunks button>span{font-size:11px}.chunks button>small{font-size:9.5px}.chunks p,.backlinks p{font-size:11px}.backlink-list button{font-size:10.5px}.panel-warning,.load-more{font-size:10.5px}
  .detail-error{display:grid;gap:7px;margin-top:8px;padding:8px;border:1px solid color-mix(in srgb,var(--danger) 35%,var(--border));border-radius:7px;color:var(--danger);font-size:10px}.detail-error button{justify-self:start;border:0;background:transparent;color:var(--l0);padding:0;cursor:pointer;font-size:10px}
</style>
