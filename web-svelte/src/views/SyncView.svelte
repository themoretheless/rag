<script lang="ts">
  import { onMount } from 'svelte'
  import { api, loadPanels } from '@/api/client'

  type Status = { ok?: boolean; db_path?: string; document_count?: number; chunk_count?: number; documents?: number; chunks?: number; backend?: string; schema_version?: string | number }
  type Agent = { agent: string; transport?: string; online?: boolean; calls_today?: number; last_call_at?: string; last_tool?: string }
  type Activity = { seq?: number; at?: string; client?: string; action?: string; status?: string }
  type Node = { id: string; label: string; detail: string; online: boolean; isHost: boolean }

  let status = $state<Status>({})
  let agents = $state<Agent[]>([])
  let activity = $state<Activity[]>([])
  let busy = $state(false)
  let error = $state('')
  let updatedAt = $state<Date | null>(null)

  const documents = $derived(status.document_count ?? status.documents ?? 0)
  const chunks = $derived(status.chunk_count ?? status.chunks ?? 0)
  const dbName = $derived(status.db_path?.split('/').pop() || 'rag.duckdb')
  const dbPath = $derived(status.db_path || 'путь сервером не опубликован')
  const nodes = $derived.by<Node[]>(() => {
    const result = new Map<string, Node>()
    for (const item of activity) {
      const raw = item.client?.trim()
      if (!raw) continue
      const isHost = raw.startsWith('host:')
      const label = isHost ? raw.slice(5) : raw
      result.set(raw, { id: raw, label, detail: isHost ? 'хост замечен в запросах' : 'MCP-клиент', online: true, isHost })
    }
    for (const agent of agents) {
      const id = `agent:${agent.agent}`
      if ([...result.values()].some((node) => node.label.toLowerCase() === agent.agent.toLowerCase())) continue
      result.set(id, { id, label: agent.agent, detail: agent.transport || 'MCP-клиент', online: Boolean(agent.online), isHost: false })
    }
    return [...result.values()].slice(0, 5)
  })

  async function load() {
    busy = true
    error = ''
    const failures = await loadPanels([
      ['статус БД', async () => { status = await api.get<Status>('/v1/status') }],
      ['клиенты', async () => { agents = (await api.get<{ items?: Agent[] }>('/v1/agents')).items ?? [] }],
      ['активность', async () => { activity = (await api.get<{ items?: Activity[] }>('/v1/activity?limit=80')).items ?? [] }],
    ])
    error = failures.length ? `Не обновлены: ${failures.join(', ')}` : ''
    updatedAt = new Date()
    busy = false
  }

  onMount(() => {
    void load()
    const timer = setInterval(() => void load(), 10_000)
    return () => clearInterval(timer)
  })
</script>

<div class="sync-page screen">
  <div class="screen-head">
    <div><h1>Синхронизация БД</h1><p>Что запущено сейчас и куда будут перетекать изменения</p></div>
    <div class="head-actions"><span class="live"><i></i> обновление 10 сек</span><button class="secondary" onclick={load} disabled={busy}>{busy ? 'Обновляю…' : 'Обновить'}</button></div>
  </div>
  {#if error}<div class="notice">{error}</div>{/if}

  <section class="summary">
    <article><span>Подтверждено БД</span><strong>{status.ok === false ? 0 : 1}</strong><small>данные от текущего gateway</small></article>
    <article><span>Документы</span><strong>{documents.toLocaleString()}</strong><small>{chunks.toLocaleString()} чанков</small></article>
    <article><span>Замечено клиентов</span><strong>{nodes.length}</strong><small>это ещё не реплики БД</small></article>
    <article class="planned"><span>Репликация</span><strong>не включена</strong><small>схема потока уже определена</small></article>
  </section>

  <section class="topology panel">
    <header><div><strong>Живая топология</strong><span>сплошное — сейчас · пунктир — план sync</span></div><time>{updatedAt ? updatedAt.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }) : '—'}</time></header>
    <div class="canvas" class:empty={nodes.length === 0}>
      <div class="primary-db db-card" style="grid-column:1">
        <span class="role"><i></i> ГЛАВНАЯ БД</span>
        <div class="database"><i></i><i></i><i></i></div>
        <strong>{dbName}</strong><code title={dbPath}>{dbPath}</code>
        <footer><span>{status.backend || 'duckdb'}</span><span>schema {status.schema_version ?? '—'}</span></footer>
      </div>

      <div class="flow push"><span>push · новые изменения</span><i></i><i></i><i></i></div>
      <div class="flow pull"><span>pull · подтверждённые изменения</span><i></i><i></i><i></i></div>

      <div class="satellites" style="grid-column:3">
        {#each nodes as node (node.id)}
          <article class="client-card" class:online={node.online}>
            <div class="mini-db"><i></i><i></i></div>
            <span><strong>{node.label}</strong><small>{node.detail}</small></span>
            <b>{node.isHost ? 'LOCAL DB: НЕ ПОДТВЕРЖДЕНА' : 'CLIENT'}</b>
          </article>
        {:else}
          <article class="client-card placeholder"><div class="mini-db"><i></i><i></i></div><span><strong>Локальная машина</strong><small>появится после первого запроса</small></span><b>ОЖИДАНИЕ</b></article>
        {/each}
      </div>
    </div>
    <footer class="truth"><i></i><span>Анимация показывает согласованный будущий поток. Сейчас запросы клиентов идут в главную БД напрямую; локальные реплики и outbox ещё не отчитываются серверу.</span></footer>
  </section>

  <section class="stages">
    <article class="panel active"><b>01</b><span><strong>Локальная запись</strong><small>Своя DuckDB принимает изменения без сети</small></span><em>план</em></article>
    <article class="panel active"><b>02</b><span><strong>Outbox → primary</strong><small>Повторяемая доставка с node_id + seq</small></span><em>план</em></article>
    <article class="panel active"><b>03</b><span><strong>Primary решает</strong><small>Канонический порядок и конфликты</small></span><em>план</em></article>
    <article class="panel active"><b>04</b><span><strong>Pull → local</strong><small>Курсор, tombstone и локальный rebuild</small></span><em>план</em></article>
  </section>
</div>

<style>
  .sync-page{display:flex;flex-direction:column;overflow:auto}.head-actions{display:flex;align-items:center;gap:9px}.live{display:flex;align-items:center;gap:6px;color:var(--text-faint);font:9px var(--mono)}.live i{width:6px;height:6px;border-radius:50%;background:var(--ok);box-shadow:0 0 7px var(--ok)}.notice{padding:8px;margin-bottom:10px;color:var(--danger);border:1px solid color-mix(in srgb,var(--danger) 30%,transparent);border-radius:7px;font-size:11px}.summary{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:9px;margin-bottom:12px}.summary article{padding:11px 13px;border:1px solid var(--border);border-radius:11px;background:var(--surface)}.summary span{display:block;color:var(--text-muted);font-size:10px;text-transform:uppercase;letter-spacing:.05em}.summary strong{display:block;margin:5px 0 2px;font-size:21px}.summary small{color:var(--text-faint);font:9px var(--mono)}.summary .planned{border-color:color-mix(in srgb,var(--warn) 35%,var(--border))}.summary .planned strong{color:var(--warn);font-size:17px}.topology{min-height:430px;display:flex;flex-direction:column;overflow:hidden}.topology>header{height:45px;flex:0 0 45px;padding:0 14px;border-bottom:1px solid var(--border);display:flex;align-items:center;justify-content:space-between}.topology>header div{display:flex;align-items:baseline;gap:10px}.topology>header strong{font-size:12px}.topology>header span,.topology>header time{color:var(--text-faint);font:9px var(--mono)}.canvas{position:relative;flex:1;min-height:320px;display:grid;grid-template-columns:minmax(210px,28%) minmax(180px,1fr) minmax(220px,31%);align-items:center;padding:25px 5%;overflow:hidden;background:radial-gradient(circle at 22% 50%,color-mix(in srgb,var(--l2) 9%,transparent),transparent 28%),linear-gradient(90deg,transparent 49.9%,color-mix(in srgb,var(--border) 30%,transparent) 50%,transparent 50.1%)}.db-card{position:relative;z-index:2;padding:18px;border:1px solid color-mix(in srgb,var(--l2) 55%,var(--border));border-radius:15px;background:color-mix(in srgb,var(--l2) 7%,var(--surface));box-shadow:0 0 45px color-mix(in srgb,var(--l2) 8%,transparent)}.role{display:flex;align-items:center;gap:6px;color:var(--l2);font:700 9px var(--mono);letter-spacing:.08em}.role i{width:7px;height:7px;border-radius:50%;background:var(--ok);box-shadow:0 0 7px var(--ok)}.database{width:56px;height:55px;position:relative;margin:18px auto 12px;border:1px solid var(--l2);border-radius:50%/18%;background:color-mix(in srgb,var(--l2) 12%,transparent)}.database:before,.database i{content:'';position:absolute;left:-1px;width:56px;height:15px;border:1px solid var(--l2);border-radius:50%;background:color-mix(in srgb,var(--l2) 12%,var(--surface))}.database:before{top:-1px}.database i:nth-child(1){top:13px}.database i:nth-child(2){top:27px}.database i:nth-child(3){top:40px}.db-card>strong{display:block;text-align:center;font:700 14px var(--mono)}.db-card>code{display:block;margin-top:5px;color:var(--text-faint);font:8px var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis;text-align:center}.db-card footer{display:flex;justify-content:space-between;margin-top:15px;padding-top:10px;border-top:1px solid var(--border);color:var(--text-muted);font:8px var(--mono)}.satellites{position:relative;z-index:2;display:flex;flex-direction:column;gap:8px}.client-card{min-height:61px;padding:9px 10px;display:grid;grid-template-columns:34px 1fr auto;gap:9px;align-items:center;border:1px solid var(--border);border-radius:10px;background:var(--surface)}.client-card.online{border-color:color-mix(in srgb,var(--ok) 25%,var(--border))}.client-card span{min-width:0}.client-card strong,.client-card small{display:block;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}.client-card strong{font-size:11px}.client-card small{margin-top:3px;color:var(--text-faint);font:8px var(--mono)}.client-card b{color:var(--warn);font:7px var(--mono)}.mini-db{position:relative;width:29px;height:27px;border:1px solid var(--text-faint);border-radius:50%/20%}.mini-db:before,.mini-db i{content:'';position:absolute;left:-1px;width:29px;height:8px;border:1px solid var(--text-faint);border-radius:50%}.mini-db:before{top:-1px}.mini-db i:first-child{top:8px}.mini-db i:last-child{top:17px}.flow{position:absolute;z-index:1;left:31%;right:34%;height:2px;border-top:1px dashed color-mix(in srgb,var(--text-faint) 65%,transparent)}.flow.push{top:43%;color:var(--warn)}.flow.pull{top:58%;color:var(--l2)}.flow span{position:absolute;left:50%;transform:translate(-50%,-18px);white-space:nowrap;color:currentColor;font:8px var(--mono)}.flow i{position:absolute;top:-3px;width:6px;height:6px;border-radius:50%;background:currentColor;box-shadow:0 0 8px currentColor;animation:travel 2.8s linear infinite}.flow i:nth-of-type(2){animation-delay:-.95s}.flow i:nth-of-type(3){animation-delay:-1.9s}.flow.push i{animation-direction:reverse}.truth{min-height:43px;padding:8px 14px;border-top:1px solid var(--border);display:flex;align-items:center;gap:9px;color:var(--text-muted);font-size:10px;line-height:1.4}.truth i{width:7px;height:7px;flex:0 0 7px;border-radius:50%;background:var(--warn)}.stages{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:9px;margin-top:12px}.stages article{min-height:68px;padding:10px;display:grid;grid-template-columns:25px 1fr auto;gap:8px;align-items:center}.stages b{color:var(--l2);font:10px var(--mono)}.stages strong,.stages small{display:block}.stages strong{font-size:10px}.stages small{margin-top:3px;color:var(--text-faint);font-size:8px;line-height:1.35}.stages em{color:var(--warn);font:7px var(--mono);font-style:normal}.stages article:not(:last-child):after{content:'›';position:absolute}.stages article{position:relative}@keyframes travel{from{left:0}to{left:100%}}@media(prefers-reduced-motion:reduce){.flow i{animation:none}.flow i:nth-of-type(1){left:18%}.flow i:nth-of-type(2){left:50%}.flow i:nth-of-type(3){left:82%}}@media(max-width:1000px){.summary,.stages{grid-template-columns:1fr 1fr}.canvas{grid-template-columns:240px 1fr 260px;padding:22px}.flow{left:31%;right:32%}}@media(max-width:720px){.summary,.stages{grid-template-columns:1fr}.canvas{display:flex;flex-direction:column;align-items:stretch;gap:22px;padding:18px}.flow{display:none}.topology{min-height:0}.screen-head{align-items:flex-start}.head-actions .live{display:none}}
</style>
