<script lang="ts">
  import { onMount, type Component } from 'svelte'
  import AppShell from '@/components/layout/AppShell.svelte'
  import ToastHost from '@/components/layout/ToastHost.svelte'
  import CommandPalette from '@/components/notion/CommandPalette.svelte'
  import { createRequestGate, errorMessage } from '@/lib/async'
  import { usePoll } from '@/lib/poll'
  import { route, startRouter } from '@/lib/router.svelte'
  import { routeByName } from '@/lib/routes'
  import { ui } from '@/lib/state/ui.svelte'
  import { wiki } from '@/lib/state/wiki.svelte'

  // Resolve the location before the first render so only the requested screen's
  // chunk is fetched (the route store starts on a placeholder until then).
  startRouter()

  /** The mounted screen. It is swapped only once the next screen's chunk has arrived. */
  let View = $state.raw<Component | null>(null)
  const screenGate = createRequestGate()

  $effect(() => {
    const ticket = screenGate.next()
    routeByName(route.name)
      .load()
      .then(
        (module) => {
          if (ticket.current) View = module.default
        },
        (cause: unknown) => {
          if (ticket.current) ui.toast(errorMessage(cause), 'error')
        },
      )
  })

  onMount(() => {
    ui.hydrateTheme()
    void ui.checkHealth()
    void wiki.loadCatalog()
  })
  // Refresh the health badge periodically so offline recovery shows up.
  usePoll(() => void ui.checkHealth(10_000), 15_000)
</script>

<AppShell>
  {#if View}<View />{/if}
</AppShell>
<CommandPalette />
<ToastHost />
