<script lang="ts">
  import { route, go } from '@/lib/router.svelte'
  import { navRoutes, preloadRoute } from '@/lib/routes'
  import NavIcon from './NavIcon.svelte'

  const mainItems = navRoutes('main')
  const bottomItems = navRoutes('bottom')
</script>
<aside class="rail" aria-label="Основная навигация">
  <button class="mark" onclick={() => go('console')} title="RAG Console"><NavIcon name="layers" size={16} /></button>
  <nav>{#each mainItems as item (item.name)}<button class:active={route.name === item.name} class={item.nav.layer ?? ''} onclick={() => go(item.name)} onpointerenter={() => preloadRoute(item.name)} title={item.nav.label}><span class="icon"><NavIcon name={item.nav.icon} /></span><span>{item.nav.label}</span></button>{/each}</nav>
  {#each bottomItems as item (item.name)}<button class:active={route.name === item.name} class="models" onclick={() => go(item.name)} onpointerenter={() => preloadRoute(item.name)} title={item.nav.label}><span class="icon"><NavIcon name={item.nav.icon} /></span><span>{item.nav.label}</span></button>{/each}
</aside>
<style>
  .rail{width:56px;flex:0 0 56px;background:#090b10;border-right:1px solid var(--border);display:flex;flex-direction:column;align-items:center;padding:8px 0;z-index:10}.mark{width:30px;height:30px;display:grid;place-items:center;margin:4px 0 10px;border:0;border-radius:9px;background:linear-gradient(135deg,var(--l3),var(--l0));color:#080a0f;cursor:pointer}nav{display:flex;flex-direction:column;width:100%;gap:2px}.models{margin-top:auto}nav button,.models{position:relative;width:100%;height:53px;border:0;background:transparent;color:var(--text-faint);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:4px;font-size:9px;cursor:pointer}nav button:hover,.models:hover{color:var(--text);background:var(--bg-hover)}nav button.active,.models.active{color:var(--text);background:linear-gradient(90deg,color-mix(in srgb,var(--layer,var(--accent)) 16%,transparent),transparent);box-shadow:inset 2px 0 var(--layer,var(--accent))}.icon{height:18px;display:grid;place-items:center}.l0{--layer:var(--l0)}.l1{--layer:var(--l1)}.l2{--layer:var(--l2)}.l3{--layer:var(--l3)}.l4{--layer:var(--l4)}
</style>
