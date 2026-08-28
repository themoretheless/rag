<script lang="ts">
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { route, goWiki, goGraph } from '@/lib/router.svelte'
  import { pageIcon } from '@/lib/pageIcon'
  import type { WikiPageMeta } from '@/api/types'

  const offline = $derived(Boolean(ui.healthError || wiki.error))

  /** Catalog loaded successfully but contains no pages. */
  const emptyCatalog = $derived(!offline && !wiki.loading && !wiki.pages.length)

  interface PageGroup {
    category: string | null
    pages: WikiPageMeta[]
  }

  /** Pages grouped by category (uncategorized last), sorted by title. */
  const groups = $derived.by((): PageGroup[] => {
    const byCat = new Map<string, WikiPageMeta[]>()
    const rest: WikiPageMeta[] = []
    for (const p of wiki.filtered) {
      const c = p.category?.trim()
      if (c) {
        const list = byCat.get(c) ?? []
        list.push(p)
        byCat.set(c, list)
      } else {
        rest.push(p)
      }
    }
    const byTitle = (a: WikiPageMeta, b: WikiPageMeta) =>
      (a.title || a.slug).localeCompare(b.title || b.slug)
    const out: PageGroup[] = Array.from(byCat.entries())
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([category, pages]) => ({ category, pages: pages.sort(byTitle) }))
    if (rest.length) out.push({ category: null, pages: rest.sort(byTitle) })
    return out
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

  function retry() {
    void ui.checkHealth()
    void wiki.loadCatalog()
  }

  function iconFor(p: { slug?: string | null; id: string }) {
    return pageIcon(p.slug || p.id)
  }
</script>

<div class="home">
  {#if offline}
    <div class="state">
      <div class="state-icon" aria-hidden="true">⛔</div>
      <h2>{ui.t('offlineTitle')}</h2>
      <p class="err-text">{ui.healthError || wiki.error}</p>
      <p class="muted-text">{ui.t('offlineHint')}</p>
      <code class="cmd-line">RAG_HTTP_BIND=127.0.0.1:7432 RAG_HTTP_ONLY=true ./target/release/rag-mcp</code>
      <button type="button" class="primary" onclick={retry}>{ui.t('retry')}</button>
    </div>
  {:else if emptyCatalog}
    <div class="state">
      <div class="state-icon" aria-hidden="true">📝</div>
      <h2>{ui.t('emptyWikiTitle')}</h2>
      <p class="muted-text">{ui.t('emptyWikiHint')}</p>
      <button type="button" class="primary" disabled={wiki.creating} onclick={onNewPage}>
        {ui.t('createFirstPage')}
      </button>
    </div>
  {:else}
    <header class="hero">
      <h1>{ui.t('homeGreeting')}</h1>
      <p class="muted-text">{ui.t('homeSubtitle')}</p>
      <div class="actions">
        <button type="button" class="primary" disabled={wiki.creating} onclick={onNewPage}>
          + {ui.t('newPage')}
        </button>
        <button type="button" class="ghost-btn" onclick={() => ui.openCommand()}>
          {ui.t('openSearch')} <kbd>⌘K</kbd>
        </button>
        <button type="button" class="ghost-btn" onclick={() => goGraph()}>
          {ui.t('openGraph')}
        </button>
      </div>
    </header>

    {#if wiki.favoritePages.length}
      <section class="section">
        <h2>{ui.t('favorites')}</h2>
        <div class="cards">
          {#each wiki.favoritePages as p (p.id)}
            <button type="button" class="card" onclick={() => open(p.id)}>
              <span class="card-icon" aria-hidden="true">{iconFor(p)}</span>
              <span class="card-title">{p.title || p.slug}</span>
              {#if p.category}<span class="card-meta">{p.category}</span>{/if}
            </button>
          {/each}
        </div>
      </section>
    {/if}

    {#if wiki.recent.length}
      <section class="section">
        <h2>{ui.t('recentPages')}</h2>
        <div class="cards">
          {#each wiki.recent as p (p.id)}
            <button type="button" class="card" onclick={() => open(p.id)}>
              <span class="card-icon" aria-hidden="true">{iconFor(p)}</span>
              <span class="card-title">{p.title || p.slug}</span>
            </button>
          {/each}
        </div>
      </section>
    {/if}

    <section class="section">
      <h2>{ui.t('allPages')}</h2>
      {#each groups as g (g.category ?? '_')}
        <div class="group">
          {#if g.category}
            <h3 class="group-title">{g.category}</h3>
          {/if}
          <div class="cards">
            {#each g.pages as p (p.id)}
              <button type="button" class="card wide" onclick={() => open(p.id)}>
                <span class="card-icon" aria-hidden="true">{iconFor(p)}</span>
                <span class="card-body">
                  <span class="card-title">{p.title || p.slug}</span>
                  {#if p.summary}<span class="card-summary">{p.summary}</span>{/if}
                  <span class="card-meta">
                    {#if p.kind && p.kind !== 'wiki'}<span>{p.kind}</span>{/if}
                    <span>r{p.revision}</span>
                  </span>
                </span>
              </button>
            {/each}
          </div>
        </div>
      {/each}
    </section>
  {/if}
</div>

<style>
  .home {
    max-width: 900px;
    margin: 0 auto;
    padding: 40px 32px 80px;
  }
  .hero h1 {
    margin: 0 0 6px;
    font-size: 30px;
    letter-spacing: -0.03em;
  }
  .muted-text {
    color: var(--text-muted);
    margin: 0 0 16px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin: 18px 0 8px;
  }
  .primary,
  .ghost-btn {
    border: none;
    border-radius: 8px;
    padding: 8px 14px;
    cursor: pointer;
    font-size: 14px;
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
  .ghost-btn {
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    color: var(--text);
  }
  .ghost-btn:hover {
    background: var(--bg-hover);
  }
  .section {
    margin-top: 28px;
  }
  .section h2 {
    font-size: 13px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-faint);
    margin: 0 0 10px;
  }
  .group-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text-muted);
    margin: 14px 0 8px;
  }
  .cards {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
    gap: 8px;
  }
  .card {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    text-align: left;
    border: 1px solid var(--border);
    background: var(--bg-elevated);
    border-radius: 10px;
    padding: 12px;
    cursor: pointer;
    color: var(--text);
    transition: border-color 0.1s ease, background 0.1s ease;
  }
  .card:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
  }
  .card.wide {
    grid-column: span 2;
  }
  @media (max-width: 640px) {
    .card.wide {
      grid-column: span 1;
    }
  }
  .card-icon {
    font-size: 20px;
    line-height: 1.2;
    flex-shrink: 0;
  }
  .card-body {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .card-title {
    font-size: 14px;
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .card-summary {
    font-size: 12px;
    color: var(--text-muted);
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .card-meta {
    display: flex;
    gap: 8px;
    font-size: 11px;
    color: var(--text-faint);
  }
  .state {
    text-align: center;
    padding: 60px 24px;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
  }
  .state-icon {
    font-size: 40px;
  }
  .state h2 {
    margin: 0;
  }
  .err-text {
    color: var(--danger);
    font-size: 13px;
    margin: 0;
    max-width: 60ch;
    overflow-wrap: anywhere;
  }
  .cmd-line {
    font-family: var(--mono);
    font-size: 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 8px 12px;
    user-select: all;
  }
</style>
