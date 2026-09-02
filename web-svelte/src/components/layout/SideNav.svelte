<script lang="ts">
  import { route, go, goGraph, goSearch, goWiki, type RouteName } from '@/lib/router.svelte'
  const items: { name: RouteName; label: string; icon: string; layer?: string }[] = [
    { name: 'console', label: 'Пульт', icon: '⌘' }, { name: 'corpus', label: 'Корпус', icon: '◫', layer: 'l0' },
    { name: 'search', label: 'Поиск', icon: '⌕', layer: 'l1' }, { name: 'graph', label: 'Граф', icon: '⌘', layer: 'l2' },
    { name: 'wiki', label: 'Вики', icon: '◇', layer: 'l3' }, { name: 'agents', label: 'Агенты', icon: '◎', layer: 'l4' },
    { name: 'evaluation', label: 'Оценка', icon: '▥' },
  ]
  function open(name: RouteName) {
    if (name === 'wiki') goWiki(); else if (name === 'graph') goGraph(); else if (name === 'search') goSearch(); else go(name)
  }
</script>
<aside class="rail" aria-label="Основная навигация">
  <button class="mark" onclick={() => go('console')} title="RAG Console">R</button>
  <nav>{#each items as item}<button class:active={route.name === item.name} class={item.layer ?? ''} onclick={() => open(item.name)} title={item.label}><span class="icon">{item.icon}</span><span>{item.label}</span></button>{/each}</nav>
  <button class:active={route.name === 'models'} class="models" onclick={() => go('models')} title="Модели"><span class="icon">⌁</span><span>Модели</span></button>
</aside>
<style>
  .rail{width:64px;flex:0 0 64px;background:#090b10;border-right:1px solid var(--border);display:flex;flex-direction:column;align-items:center;padding:8px 0;z-index:10}.mark{width:34px;height:34px;margin:1px 0 10px;border:0;border-radius:10px;background:linear-gradient(135deg,var(--l3),var(--l0));color:#080a0f;font:800 15px var(--mono);cursor:pointer}nav{display:flex;flex-direction:column;width:100%;gap:2px}.models{margin-top:auto}nav button,.models{position:relative;width:100%;height:53px;border:0;background:transparent;color:var(--text-faint);display:flex;flex-direction:column;align-items:center;justify-content:center;gap:3px;font-size:9px;cursor:pointer}nav button:hover,.models:hover{color:var(--text);background:var(--bg-hover)}nav button.active,.models.active{color:var(--text);background:linear-gradient(90deg,color-mix(in srgb,var(--layer,var(--accent)) 16%,transparent),transparent);box-shadow:inset 2px 0 var(--layer,var(--accent))}.icon{font:600 18px/1 var(--mono)}.l0{--layer:var(--l0)}.l1{--layer:var(--l1)}.l2{--layer:var(--l2)}.l3{--layer:var(--l3)}.l4{--layer:var(--l4)}
</style>
