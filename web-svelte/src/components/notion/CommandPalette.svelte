<script lang="ts">
  import { ui } from '@/lib/state/ui.svelte'
  import { wiki } from '@/lib/state/wiki.svelte'
  import { route, goWiki, goGraph, goSearch } from '@/lib/router.svelte'

  interface ActionItem {
    kind: 'action'
    id: string
    icon: string
    title: string
    hint: string
    run: () => void | Promise<void>
  }
  interface PageItem {
    kind: 'page'
    id: string
    icon: string
    title: string
    hint: string
    pageId: string
  }
  type Item = ActionItem | PageItem

  let q = $state('')
  let active = $state(0)
  let inputEl: HTMLInputElement | null = $state(null)

  const actions = $derived.by((): ActionItem[] => [
    {
      kind: 'action',
      id: 'act-new',
      icon: '＋',
      title: ui.t('paletteActionNew'),
      hint: '',
      run: async () => {
        try {
          const id = await wiki.createPage()
          if (!id) return
          if (route.pageId === id) await wiki.openPage(id, false)
          else goWiki(id)
        } catch {
          /* createPage toasted */
        }
      },
    },
    {
      kind: 'action',
      id: 'act-graph',
      icon: '🕸',
      title: ui.t('paletteActionGraph'),
      hint: '',
      run: () => goGraph(),
    },
    {
      kind: 'action',
      id: 'act-search',
      icon: '🔍',
      title: ui.t('paletteActionSearch'),
      hint: '',
      run: () => goSearch(),
    },
  ])

  const results = $derived.by((): Item[] => {
    const query = q.trim().toLowerCase()
    const acts = actions.filter(
      (a) => !query || a.title.toLowerCase().includes(query),
    )
    const pages: PageItem[] = (
      !query
        ? wiki.pages.slice(0, 12)
        : wiki.pages
            .filter((p) =>
              `${p.title} ${p.slug} ${p.summary ?? ''}`.toLowerCase().includes(query),
            )
            .slice(0, 12)
    ).map((p) => ({
      kind: 'page',
      id: 'page-' + p.id,
      icon: '',
      title: p.title || p.slug,
      hint: p.slug,
      pageId: p.id,
    }))
    return [...acts, ...pages]
  })

  $effect(() => {
    if (ui.commandOpen) {
      q = ''
      active = 0
      if (!wiki.pages.length) void wiki.loadCatalog()
      // Focus after the overlay renders.
      requestAnimationFrame(() => inputEl?.focus())
    }
  })

  // Keep the active row in range when results shrink.
  $effect(() => {
    if (active > results.length - 1) active = Math.max(0, results.length - 1)
  })

  function close() {
    ui.closeCommand()
  }

  function pick(item: Item) {
    close()
    if (item.kind === 'page') {
      goWiki(item.pageId)
    } else {
      void item.run()
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      close()
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      active = Math.min(active + 1, results.length - 1)
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      active = Math.max(active - 1, 0)
    }
    if (e.key === 'Enter') {
      const item = results[active]
      if (item) {
        e.preventDefault()
        pick(item)
      }
    }
  }
</script>

{#if ui.commandOpen}
  <div
    class="overlay"
    onclick={(e) => e.target === e.currentTarget && close()}
    onkeydown={onKey}
    role="presentation"
  >
    <div class="panel" role="dialog" aria-modal="true">
      <input
        bind:this={inputEl}
        bind:value={q}
        class="input"
        type="search"
        placeholder={ui.t('palettePlaceholder')}
      />
      <ul>
        {#each results as item, i (item.id)}
          <li class:active={i === active}>
            <button
              type="button"
              class="item"
              onmouseenter={() => (active = i)}
              onclick={() => pick(item)}
            >
              {#if item.kind === 'action'}
                <span class="lead" aria-hidden="true">{item.icon}</span>
              {/if}
              <span class="t">{item.title}</span>
              <span class="s">{item.hint}</span>
            </button>
          </li>
        {/each}
        {#if !results.length}
          <li class="empty">{ui.t('paletteNoMatches')}</li>
        {/if}
      </ul>
      <div class="hint">
        <span>{ui.t('paletteNav')}</span>
        <span>{ui.t('paletteOpen')}</span>
        <span>{ui.t('paletteClose')}</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
    display: grid;
    place-items: start center;
    padding-top: 12vh;
    z-index: 900;
  }
  .panel {
    width: min(560px, 92vw);
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 14px;
    box-shadow: var(--shadow);
    overflow: hidden;
  }
  .input {
    width: 100%;
    border: none;
    border-bottom: 1px solid var(--border);
    padding: 14px 16px;
    background: transparent;
    outline: none;
    font-size: 16px;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 6px;
    max-height: 360px;
    overflow: auto;
  }
  li {
    border-radius: 8px;
  }
  li.active {
    background: var(--bg-active);
  }
  .item {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
    width: 100%;
    border: none;
    background: transparent;
    padding: 10px 12px;
    cursor: pointer;
    color: inherit;
    text-align: left;
    border-radius: 8px;
  }
  .lead {
    flex-shrink: 0;
    width: 1.3em;
    color: var(--text-muted);
  }
  .t {
    font-weight: 500;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .s {
    color: var(--text-faint);
    font-size: 12px;
    font-family: var(--mono);
  }
  .empty {
    color: var(--text-muted);
    text-align: center;
    padding: 10px 12px;
  }
  .hint {
    display: flex;
    gap: 14px;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
    color: var(--text-faint);
    font-size: 11px;
  }
</style>
