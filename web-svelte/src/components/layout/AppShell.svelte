<script lang="ts">
  import { ui } from '@/lib/state/ui.svelte'
  import TopBar from './TopBar.svelte'
  import SideNav from './SideNav.svelte'
  import type { Snippet } from 'svelte'

  let { children }: { children: Snippet } = $props()

  function onKey(e: KeyboardEvent) {
    const meta = e.metaKey || e.ctrlKey
    if (meta && e.key.toLowerCase() === 'k') {
      e.preventDefault()
      ui.openCommand()
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="shell">
  <div class="body">
    <SideNav />
    <div class="workspace"><TopBar /><main class="main">{@render children()}</main></div>
  </div>
</div>

<style>
  .shell {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }
  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }
  .workspace { display:flex; flex:1; min-width:0; min-height:0; flex-direction:column; }
  .main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: var(--bg);
    display: flex;
    flex-direction: column;
  }
</style>
