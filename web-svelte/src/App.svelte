<script lang="ts">
  import { onMount } from 'svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { route, startRouter } from '@/lib/router.svelte'
  import AppShell from '@/components/layout/AppShell.svelte'
  import CommandPalette from '@/components/notion/CommandPalette.svelte'
  import ToastHost from '@/components/layout/ToastHost.svelte'
  import WikiView from '@/views/WikiView.svelte'
  import GraphView from '@/views/GraphView.svelte'
  import SearchView from '@/views/SearchView.svelte'

  onMount(() => {
    ui.hydrateTheme()
    startRouter()
    void ui.checkHealth()
    // Refresh the health badge periodically so offline recovery shows up.
    const timer = setInterval(() => void ui.checkHealth(), 15_000)
    return () => clearInterval(timer)
  })
</script>

<AppShell>
  {#if route.name === 'wiki'}
    <WikiView />
  {:else if route.name === 'graph'}
    <GraphView />
  {:else}
    <SearchView />
  {/if}
</AppShell>
<CommandPalette />
<ToastHost />
