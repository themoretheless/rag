<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { goWiki, goGraph } from '@/lib/router.svelte'

  let historyOpen = $state(false)
  let historyBtn: HTMLElement | null = $state(null)

  /** Catalog row for the open page (category, title fallback). */
  const catalogPage = $derived.by(() => {
    const id = wiki.current?.id
    if (!id) return null
    return wiki.pages.find((p) => p.id === id) ?? null
  })

  const category = $derived.by(() => {
    const c = catalogPage?.category
    return c && c.trim() ? c.trim() : null
  })

  const pageTitle = $derived.by(() => {
    if (wiki.editing && wiki.draftTitle.trim()) return wiki.draftTitle.trim()
    return wiki.current?.title || catalogPage?.title || catalogPage?.slug || ui.t('untitled')
  })

  type Crumb = {
    key: string
    label: string
    link?: boolean
  }

  const crumbs = $derived.by((): Crumb[] => {
    const out: Crumb[] = [{ key: 'wiki', label: ui.t('wiki'), link: true }]
    if (!wiki.current) return out
    if (category) {
      out.push({ key: 'cat', label: category })
    }
    out.push({ key: 'title', label: pageTitle })
    return out
  })

  /** History stack (oldest first in store); show newest first in the menu. */
  const historyEntries = $derived.by(() => {
    const ids = [...wiki.history].reverse()
    return ids.map((id) => {
      const p = wiki.pages.find((x) => x.id === id)
      return {
        id,
        title: p?.title || p?.slug || id.slice(0, 8),
        category: p?.category ?? null,
      }
    })
  })

  function goWikiRoot() {
    historyOpen = false
    goWiki()
  }

  /** Pop history and keep the route in sync via replace (no extra pushHistory). */
  function goBack() {
    const prev = wiki.history[wiki.history.length - 1]
    if (!prev) return
    historyOpen = false
    wiki.goBack()
    goWiki(prev, { replace: true })
  }

  /**
   * Jump to a stack entry: truncate after it, goBack once (opens without re-push),
   * then replace the URL so the route matches current.
   */
  function openHistoryId(id: string) {
    historyOpen = false
    const stack = wiki.history
    const idx = stack.lastIndexOf(id)
    if (idx < 0) {
      goWiki(id)
      return
    }
    stack.splice(idx + 1)
    wiki.goBack()
    goWiki(id, { replace: true })
  }

  function toggleHistory() {
    if (!wiki.history.length) return
    historyOpen = !historyOpen
  }

  function onDocClick(e: MouseEvent) {
    if (!historyOpen) return
    const t = e.target as Node | null
    if (historyBtn?.contains(t)) return
    historyOpen = false
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') historyOpen = false
  }

  function showInGraph() {
    const id = wiki.current?.id
    if (!id) return
    goGraph(id)
  }

  async function onSave() {
    try {
      await wiki.save()
    } catch {
      ui.toast(ui.t('saveFailed'), 'error')
    }
  }
</script>

<svelte:document onclick={onDocClick} onkeydown={onKey} />

<div class="bar">
  <div class="left">
    <div class="back-group" bind:this={historyBtn}>
      <button
        type="button"
        class="ghost back"
        disabled={!wiki.history.length}
        title={ui.t('back')}
        onclick={goBack}
      >
        {ui.t('back')}
      </button>
      <button
        type="button"
        class="ghost hist-toggle"
        disabled={!wiki.history.length}
        aria-expanded={historyOpen}
        title={ui.t('history')}
        onclick={(e) => {
          e.stopPropagation()
          toggleHistory()
        }}
      >
        ▾
      </button>
      {#if historyOpen && historyEntries.length}
        <div class="hist-menu" role="menu">
          {#each historyEntries as h (h.id)}
            <button
              type="button"
              class="hist-item"
              role="menuitem"
              onclick={() => openHistoryId(h.id)}
            >
              <span class="hist-title">{h.title}</span>
              {#if h.category}<span class="hist-cat">{h.category}</span>{/if}
            </button>
          {/each}
        </div>
      {/if}
    </div>

    <nav class="crumbs" aria-label={ui.t('breadcrumb')}>
      {#each crumbs as c, i (c.key)}
        {#if i > 0}
          <span class="sep" aria-hidden="true">/</span>
        {/if}
        {#if c.link && i < crumbs.length - 1}
          <button type="button" class="crumb link" onclick={goWikiRoot}>
            {c.label}
          </button>
        {:else}
          <span
            class="crumb"
            class:current={i === crumbs.length - 1 && !!wiki.current}
            title={c.label}
          >
            {c.label}
          </span>
        {/if}
      {/each}
    </nav>

    {#if wiki.current?.revision != null}
      <span class="rev">r{wiki.current.revision}</span>
    {/if}
  </div>
  <div class="right">
    {#if wiki.current && !wiki.editing}
      <button
        type="button"
        class="ghost pin"
        class:on={wiki.isFavorite(wiki.current.id)}
        title={wiki.isFavorite(wiki.current.id) ? ui.t('unpinFavorite') : ui.t('pinFavorite')}
        aria-pressed={wiki.isFavorite(wiki.current.id)}
        onclick={() => wiki.toggleFavorite(wiki.current!.id)}
      >
        {wiki.isFavorite(wiki.current.id) ? '★' : '☆'}
      </button>
      <button type="button" class="ghost" onclick={showInGraph}>{ui.t('showInGraph')}</button>
      <button type="button" class="primary" onclick={() => wiki.startEdit()}>
        {ui.t('edit')} <kbd>e</kbd>
      </button>
    {:else if wiki.editing}
      <button type="button" class="ghost" onclick={() => wiki.cancelEdit()}>
        {ui.t('cancel')} <kbd>esc</kbd>
      </button>
      <button type="button" class="primary" disabled={!wiki.dirty} onclick={onSave}>
        {ui.t('save')}
      </button>
    {/if}
  </div>
</div>

<style>
  .bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 10px 18px;
    border-bottom: 1px solid var(--border);
    gap: 12px;
    min-height: 44px;
  }
  .left,
  .right {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .left {
    flex: 1;
  }
  .back-group {
    position: relative;
    display: flex;
    align-items: center;
    flex-shrink: 0;
  }
  .back {
    border-radius: 8px 0 0 8px;
    padding-right: 8px;
  }
  .hist-toggle {
    border-radius: 0 8px 8px 0;
    padding: 7px 8px;
    margin-left: -2px;
    font-size: 11px;
    line-height: 1;
  }
  .hist-menu {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 40;
    min-width: 220px;
    max-width: min(360px, 70vw);
    max-height: 280px;
    overflow: auto;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: var(--shadow);
    padding: 4px;
  }
  .hist-item {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 2px;
    width: 100%;
    border: none;
    background: transparent;
    text-align: left;
    padding: 8px 10px;
    border-radius: 8px;
    cursor: pointer;
    color: var(--text);
  }
  .hist-item:hover {
    background: var(--bg-hover);
  }
  .hist-title {
    font-size: 13px;
    font-weight: 500;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hist-cat {
    font-size: 11px;
    color: var(--text-faint);
  }
  .crumbs {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    flex: 1;
    overflow: hidden;
    font-size: 13px;
  }
  .sep {
    color: var(--text-faint);
    flex-shrink: 0;
    user-select: none;
  }
  .crumb {
    color: var(--text-muted);
    max-width: 28vw;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex-shrink: 1;
    min-width: 0;
  }
  .crumb.current {
    font-weight: 600;
    color: var(--text);
    max-width: 36vw;
    flex-shrink: 0;
  }
  button.crumb.link {
    border: none;
    background: transparent;
    padding: 2px 4px;
    margin: 0 -4px;
    border-radius: 6px;
    cursor: pointer;
    font: inherit;
    color: var(--text-muted);
    flex-shrink: 0;
  }
  button.crumb.link:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  .rev {
    font-size: 12px;
    color: var(--text-faint);
    font-family: var(--mono);
    flex-shrink: 0;
  }
  .ghost,
  .primary {
    border: none;
    border-radius: 8px;
    padding: 7px 12px;
    cursor: pointer;
  }
  .ghost {
    background: transparent;
    color: var(--text-muted);
  }
  .ghost:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text);
  }
  .ghost:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .primary {
    background: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .primary:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .pin.on {
    color: var(--star);
  }
</style>
