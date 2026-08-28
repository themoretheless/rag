<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { route } from '@/lib/router.svelte'
  import WikiArticle from '@/components/wiki/WikiArticle.svelte'
  import WikiEditor from '@/components/wiki/WikiEditor.svelte'
  import BacklinksPanel from '@/components/wiki/BacklinksPanel.svelte'
  import WikiToolbar from '@/components/wiki/WikiToolbar.svelte'

  function isTypedInputTarget(target: EventTarget | null): boolean {
    if (!(target instanceof HTMLElement)) return false
    const tag = target.tagName
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true
    if (target.isContentEditable) return true
    return Boolean(target.closest('input, textarea, select, [contenteditable="true"]'))
  }

  function onKey(e: KeyboardEvent) {
    if (e.metaKey || e.ctrlKey || e.altKey) return
    if (ui.commandOpen) return

    // Esc cancels edit even when the editor fields are focused (leave edit mode).
    if (e.key === 'Escape' && wiki.editing) {
      e.preventDefault()
      wiki.cancelEdit()
      return
    }

    // Single-letter shortcuts stay typed-input-safe.
    if (isTypedInputTarget(e.target)) return
    if (e.key.toLowerCase() === 'e' && !wiki.editing && wiki.current) {
      e.preventDefault()
      wiki.startEdit()
    }
  }

  // Route → store: open the addressed page; /wiki root shows the dashboard.
  let openedId: string | null = null
  $effect(() => {
    if (route.name !== 'wiki') return
    const id = route.pageId
    if (id) {
      if (id === openedId && wiki.current?.id === id) return
      openedId = id
      void wiki.openPage(id)
    } else {
      openedId = null
      if (wiki.current) wiki.closePage()
    }
  })
</script>

<svelte:window onkeydown={onKey} />

<div class="wiki">
  <section class="center">
    <WikiToolbar />
    {#if wiki.editing}
      <WikiEditor />
    {:else}
      <WikiArticle />
    {/if}
  </section>
  <BacklinksPanel />
</div>

<style>
  .wiki {
    display: flex;
    height: 100%;
    min-height: 0;
    flex: 1;
  }
  .center {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
</style>
