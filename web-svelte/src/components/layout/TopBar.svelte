<script lang="ts">
  import { ui } from '@/lib/state/ui.svelte'
  import { route } from '@/lib/router.svelte'

  const labels: Record<string, { title: string; layer: string; color: string }> = {
    console: { title: 'Пульт', layer: 'L0–L4 · обзор', color: 'var(--accent)' },
    corpus: { title: 'Корпус', layer: 'L0 · сырой корпус', color: 'var(--l0)' },
    search: { title: 'Поиск', layer: 'L1 · извлечение', color: 'var(--l1)' },
    graph: { title: 'Граф', layer: 'L2 · граф объектов', color: 'var(--l2)' },
    wiki: { title: 'Вики', layer: 'L3 · знание', color: 'var(--l3)' },
    agents: { title: 'Агенты · Журнал', layer: 'L4 · MCP-клиенты', color: 'var(--l4)' },
    sync: { title: 'Синхронизация БД', layer: 'local ↔ primary', color: 'var(--l2)' },
    evaluation: { title: 'Оценка', layer: 'eval · retrieval', color: 'var(--l1)' },
    models: { title: 'Модели и пайплайн', layer: 'RAG_* env · runtime', color: 'var(--l3)' },
  }
  const current = $derived(labels[route.name] ?? labels.console)
  const healthReady = $derived(Boolean(ui.health?.ok && ui.health?.fts_ready))
  const onlineAgents = $derived(
    ui.shellAgentsError
      ? []
      : ui.shellAgents.filter((agent) => agent.online && (agent.transport === 'stdio' || agent.transport === 'http-mcp')),
  )
  function initials(name: string) {
    const parts = name.trim().split(/[\s_-]+/).filter(Boolean)
    if (!parts.length) return '?'
    return (parts.length === 1 ? parts[0].slice(0, 2) : parts.slice(0, 2).map((part) => part[0]).join('')).toUpperCase()
  }
  const healthLabel = $derived.by(() => {
    if (ui.healthError) return 'offline'
    if (!ui.health) return 'connecting…'
    const bind = ui.health.runtime?.http_bind ?? 'gateway'
    const backend = ui.health.backend ?? 'storage'
    return `${bind} · ${backend} · fts ${ui.health.fts_ready ? '✓' : '!'}`
  })
</script>

<header class="top" style={`--route-color:${current.color}`}>
  <div class="crumb"><span>rag-mcp</span><i>/</i><strong>{current.title}</strong><b>{current.layer}</b></div>
  <button class="command" onclick={() => ui.openCommand()}><span>Страница, документ, узел или команда…</span><kbd>⌘K</kbd></button>
  <div class="right">
    <span class:bad={!!ui.healthError || ui.health?.ok === false} class:ok={healthReady} class="health"><i></i>{healthLabel}</span>
    {#if onlineAgents.length}
      <div class="avatars" aria-label={`${onlineAgents.length} MCP-клиентов онлайн`}>
        {#each onlineAgents.slice(0, 3) as agent (agent.agent)}<span class="online" title={`${agent.agent} · ${agent.transport} · online`}>{initials(agent.agent)}</span>{/each}
        {#if onlineAgents.length > 3}<span class="more" title={`Ещё ${onlineAgents.length - 3} онлайн`}>+{onlineAgents.length - 3}</span>{/if}
      </div>
    {/if}
    <div class="locale"><button class:active={ui.locale === 'ru'} onclick={() => ui.setLocale('ru')}>RU</button><button class:active={ui.locale === 'en'} onclick={() => ui.setLocale('en')}>EN</button></div>
    <button class="theme" onclick={() => ui.setTheme(ui.theme === 'dark' ? 'light' : 'dark')}>{ui.theme === 'dark' ? '☾' : '☀'}</button>
  </div>
</header>

<style>
  .top{height:48px;flex:0 0 48px;display:grid;grid-template-columns:minmax(260px,1fr) minmax(280px,460px) minmax(330px,1fr);align-items:center;padding:0 16px;border-bottom:1px solid var(--border);background:var(--bg);gap:16px}.crumb{display:flex;align-items:center;gap:9px;white-space:nowrap;min-width:0}.crumb span,.crumb i{color:var(--text-faint);font-style:normal}.crumb strong{font-size:13px;overflow:hidden;text-overflow:ellipsis}.crumb b{font:500 10px var(--mono);background:color-mix(in srgb,var(--route-color) 12%,transparent);color:var(--route-color);padding:3px 7px;border-radius:5px}.command{height:30px;display:flex;align-items:center;justify-content:space-between;border:1px solid var(--border);border-radius:8px;background:var(--surface);color:var(--text-faint);padding:0 10px;cursor:pointer;font-size:12px}.command:hover{border-color:var(--border-strong);color:var(--text-muted)}.right{display:flex;align-items:center;justify-content:flex-end;gap:9px}.health{display:flex;align-items:center;gap:6px;font:500 10.5px var(--mono);color:var(--text-muted);white-space:nowrap}.health i{width:7px;height:7px;border-radius:50%;background:var(--warn)}.health.ok i{background:var(--ok);box-shadow:0 0 7px var(--ok)}.health.bad i{background:var(--danger)}.avatars{display:flex}.avatars span{position:relative;width:22px;height:22px;margin-left:-5px;border:2px solid var(--bg);border-radius:50%;display:grid;place-items:center;background:var(--surface-2);color:var(--text-muted);font:600 9px var(--mono)}.avatars span:first-child{margin-left:0}.avatars span.online::after{content:'';position:absolute;right:-1px;bottom:-1px;width:6px;height:6px;border:1px solid var(--bg);border-radius:50%;background:var(--ok)}.avatars span.more{color:var(--l4);font-size:8px}.avatars span.more::after{display:none}.locale{display:flex;border:1px solid var(--border);border-radius:6px;overflow:hidden}.locale button,.theme{height:24px;border:0;background:transparent;color:var(--text-faint);font:600 9.5px var(--mono);cursor:pointer}.locale button{padding:0 6px}.locale button.active{background:color-mix(in srgb,var(--route-color) 13%,transparent);color:var(--route-color)}.theme{width:25px;border:1px solid var(--border);border-radius:6px}@media(max-width:1080px){.top{grid-template-columns:1fr auto}.command{display:none}.avatars{display:none}}@media(max-width:760px){.top{grid-template-columns:1fr auto}.crumb b,.health{display:none}}
</style>
