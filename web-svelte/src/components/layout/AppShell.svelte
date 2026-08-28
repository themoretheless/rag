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
    if (meta && e.key.toLowerCase() === 'b') {
      e.preventDefault()
      ui.toggleSidebar()
    }
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="shell" class:collapsed={ui.sidebarCollapsed}>
  <TopBar />
  <div class="body">
    <SideNav />
    <main class="main">
      {@render children()}
    </main>
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
