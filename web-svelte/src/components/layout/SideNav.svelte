<script lang="ts">
  import { onMount } from 'svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { wiki } from '@/lib/state/wiki.svelte'
  import { route, goWiki } from '@/lib/router.svelte'
  import { pageIcon } from '@/lib/pageIcon'

  onMount(() => {
    void wiki.loadCatalog()
  })

  function open(id: string) {
    goWiki(id)
  }

  async function onNewPage() {
    try {
      const id = await wiki.createPage()
      if (!id) return
      if (route.pageId === id) {
        await wiki.openPage(id, false)
      } else {
        goWiki(id)
      }
    } catch {
      // createPage already toasted the error
    }
  }

  function iconFor(p: { slug?: string | null; id: string }) {
    return pageIcon(p.slug || p.id)
  }

  function onPin(e: MouseEvent, id: string) {
    e.stopPropagation()
    wiki.toggleFavorite(id)
  }

  function pageLabel(p: { title?: string | null; slug?: string | null }) {
    return p.title || p.slug || ''
  }
</script>

<aside class="side" class:hidden={ui.sidebarCollapsed}>
  <div class="scroll">
    {#if wiki.favoritePages.length}
      <div class="section">
        <div class="head">
          <strong>{ui.t('favorites')}</strong>
          <span class="count">{wiki.favoritePages.length}</span>
        </div>
        <ul class="list favorites">
          {#each wiki.favoritePages as p (p.id)}
            <li class:active={wiki.current?.id === p.id}>
              <button type="button" class="row" onclick={() => open(p.id)}>
                <span class="icon" aria-hidden="true">{iconFor(p)}</span>
                <span class="text">
                  <span class="title">{pageLabel(p)}</span>
                  <span class="meta">
                    {#if p.category}<span>{p.category}</span>{/if}
                    {#if p.revision != null}<span>r{p.revision}</span>{/if}
                  </span>
                </span>
              </button>
              <button
                type="button"
                class="pin on"
                title={ui.t('unpinFavorite')}
                aria-label={ui.t('unpinFavorite')}
                onclick={(e) => onPin(e, p.id)}
              >
                ★
              </button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}

    {#if wiki.recent.length}
      <div class="section recent-section">
        <div class="head">
          <strong>{ui.t('recentPages')}</strong>
          <span class="count">{wiki.recent.length}</span>
        </div>
        <ul class="list recent">
          {#each wiki.recent as p (p.id)}
            <li class:active={wiki.current?.id === p.id}>
              <button
                type="button"
                class="row"
                title={p.slug}
                onclick={() => open(p.id)}
              >
                <span class="icon" aria-hidden="true">{iconFor(p)}</span>
                <span class="text">
                  <span class="title">{pageLabel(p)}</span>
                </span>
              </button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}

    <div class="head">
      <strong>{ui.t('pages')}</strong>
      <div class="head-actions">
        <button
          type="button"
          class="ghost new-page"
          title={ui.t('newPage')}
          disabled={wiki.creating}
          onclick={onNewPage}
        >
          +
        </button>
        <button type="button" class="ghost" title={ui.t('refresh')} onclick={() => wiki.loadCatalog()}>
          ↻
        </button>
      </div>
    </div>

    <div class="chips" role="toolbar" aria-label={ui.t('filterFacets')}>
      <button
        type="button"
        class="chip"
        class:active={wiki.facetIsAll()}
        aria-pressed={wiki.facetIsAll()}
        onclick={() => wiki.setFacet({ type: 'all' })}
      >
        {ui.t('facetAll')}
      </button>
      <button
        type="button"
        class="chip"
        class:active={wiki.facetIsKind('wiki')}
        aria-pressed={wiki.facetIsKind('wiki')}
        title="kind=wiki"
        onclick={() => wiki.setFacet({ type: 'kind', value: 'wiki' })}
      >
        {ui.t('wiki')}
      </button>
      {#each wiki.categories as cat (cat)}
        <button
          type="button"
          class="chip"
          class:active={wiki.facetIsCategory(cat)}
          aria-pressed={wiki.facetIsCategory(cat)}
          title={'category=' + cat}
          onclick={() => wiki.setFacet({ type: 'category', value: cat })}
        >
          {cat}
        </button>
      {/each}
    </div>

    <input
      bind:value={wiki.filter}
      class="filter"
      type="search"
      placeholder={ui.t('filterPages')}
      autocomplete="off"
    />
    {#if wiki.loading && !wiki.pages.length}
      <div class="muted">{ui.t('loading')}</div>
    {:else if wiki.error}
      <div class="err">{wiki.error}</div>
    {:else}
      <ul class="list">
        {#each wiki.filtered as p (p.id)}
          <li class:active={wiki.current?.id === p.id}>
            <button type="button" class="row" onclick={() => open(p.id)}>
              <span class="icon" aria-hidden="true">{iconFor(p)}</span>
              <span class="text">
                <span class="title">{pageLabel(p)}</span>
                <span class="meta">
                  {#if p.category}<span>{p.category}</span>{/if}
                  {#if p.kind && p.kind !== 'wiki'}<span>{p.kind}</span>{/if}
                  <span>r{p.revision}</span>
                </span>
              </span>
            </button>
            <button
              type="button"
              class="pin"
              class:on={wiki.isFavorite(p.id)}
              title={wiki.isFavorite(p.id) ? ui.t('unpinFavorite') : ui.t('pinFavorite')}
              aria-label={wiki.isFavorite(p.id) ? ui.t('unpinFavorite') : ui.t('pinFavorite')}
              aria-pressed={wiki.isFavorite(p.id)}
              onclick={(e) => onPin(e, p.id)}
            >
              {wiki.isFavorite(p.id) ? '★' : '☆'}
            </button>
          </li>
        {/each}
        {#if !wiki.filtered.length}
          <li class="muted empty">
            {ui.t('noPages')}
            {#if !wiki.filter.trim()}
              <span class="hint">{ui.t('noPagesHint')}</span>
            {/if}
          </li>
        {/if}
      </ul>
    {/if}
  </div>
</aside>

<style>
  .side {
    width: var(--sidebar-w);
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    background: var(--bg-sidebar);
    display: flex;
    flex-direction: column;
    min-height: 0;
    transition: width 0.15s ease, opacity 0.15s ease;
  }
  .side.hidden {
    width: 0;
    opacity: 0;
    overflow: hidden;
    border: none;
  }
  .scroll {
    display: flex;
    flex-direction: column;
    min-height: 0;
    flex: 1;
    overflow-y: auto;
  }
  .section {
    flex-shrink: 0;
    border-bottom: 1px solid var(--border);
  }
  .section .list {
    flex: 0 1 auto;
    max-height: none;
    overflow: visible;
  }
  .section .list.recent li {
    padding-top: 0;
    padding-bottom: 0;
  }
  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 12px 6px;
  }
  .head-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .count {
    font-size: 11px;
    color: var(--text-faint);
    font-variant-numeric: tabular-nums;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 0 10px 8px;
    flex-shrink: 0;
  }
  .chip {
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    color: var(--text-muted);
    border-radius: 999px;
    padding: 3px 10px;
    font-size: 12px;
    line-height: 1.3;
    cursor: pointer;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chip:hover {
    background: var(--bg-hover);
    color: var(--text);
  }
  .chip.active {
    background: var(--accent-soft);
    border-color: transparent;
    color: var(--accent);
    font-weight: 600;
  }
  .filter {
    margin: 0 10px 8px;
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    border-radius: 8px;
    padding: 8px 10px;
    outline: none;
  }
  .filter:focus {
    border-color: var(--accent);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 4px;
    flex: 1;
  }
  .list li {
    display: flex;
    align-items: flex-start;
    border-radius: 8px;
    padding-right: 2px;
  }
  .list li:hover {
    background: var(--bg-hover);
  }
  .list li.active {
    background: var(--bg-active);
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 8px;
    min-width: 0;
    flex: 1;
    border: none;
    background: transparent;
    text-align: left;
    padding: 6px 0 6px 10px;
    cursor: pointer;
    border-radius: 8px;
    color: inherit;
  }
  .icon {
    flex-shrink: 0;
    width: 1.35em;
    font-size: 15px;
    line-height: 1.3;
    text-align: center;
    user-select: none;
  }
  .text {
    min-width: 0;
    flex: 1;
    display: block;
  }
  .title {
    display: block;
    font-size: 14px;
    line-height: 1.3;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .meta {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: var(--text-faint);
    margin-top: 2px;
  }
  .pin {
    flex-shrink: 0;
    border: none;
    background: transparent;
    cursor: pointer;
    color: var(--text-faint);
    border-radius: 6px;
    padding: 2px 6px;
    font-size: 14px;
    line-height: 1.2;
    opacity: 0;
    margin-top: 4px;
  }
  .list li:hover .pin,
  .list li:focus-within .pin,
  .pin.on {
    opacity: 1;
  }
  .pin:hover {
    background: var(--bg-hover);
    color: var(--accent);
  }
  .pin.on {
    color: var(--star);
  }
  .muted {
    color: var(--text-muted);
    padding: 12px;
    font-size: 13px;
  }
  .empty .hint {
    display: block;
    font-size: 12px;
    color: var(--text-faint);
    margin-top: 4px;
  }
  .err {
    color: var(--danger);
    padding: 12px;
    font-size: 13px;
  }
  .ghost {
    border: none;
    background: transparent;
    cursor: pointer;
    color: var(--text-muted);
    border-radius: 6px;
    padding: 4px 8px;
  }
  .ghost:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .ghost:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .new-page {
    font-size: 18px;
    font-weight: 600;
    line-height: 1;
    color: var(--text);
  }
</style>
