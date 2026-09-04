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
  import ConsoleView from '@/views/ConsoleView.svelte'
  import CorpusView from '@/views/CorpusView.svelte'
  import AgentsView from '@/views/AgentsView.svelte'
  import SyncView from '@/views/SyncView.svelte'
  import EvaluationView from '@/views/EvaluationView.svelte'
  import ModelsView from '@/views/ModelsView.svelte'
  import { wiki } from '@/lib/state/wiki.svelte'

  onMount(() => {
    ui.hydrateTheme()
    startRouter()
    void ui.checkHealth()
    void wiki.loadCatalog()
    // Refresh the health badge periodically so offline recovery shows up.
    const timer = setInterval(() => void ui.checkHealth(), 15_000)
    return () => clearInterval(timer)
  })
</script>

<AppShell>
  {#if route.name === 'console'}
    <ConsoleView />
  {:else if route.name === 'corpus'}
    <CorpusView />
  {:else if route.name === 'wiki'}
    <WikiView />
  {:else if route.name === 'graph'}
    <GraphView />
  {:else if route.name === 'search'}
    <SearchView />
  {:else if route.name === 'agents'}
    <AgentsView />
  {:else if route.name === 'sync'}
    <SyncView />
  {:else if route.name === 'evaluation'}
    <EvaluationView />
  {:else}
    <ModelsView />
  {/if}
</AppShell>
<CommandPalette />
<ToastHost />
