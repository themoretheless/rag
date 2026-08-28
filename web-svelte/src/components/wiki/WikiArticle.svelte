<script lang="ts">
  import { onDestroy, tick } from 'svelte'
  import { wiki } from '@/lib/state/wiki.svelte'
  import { ui } from '@/lib/state/ui.svelte'
  import { goWiki } from '@/lib/router.svelte'
  import { renderWikiHtml } from '@/lib/markdown'
  import { pageIcon } from '@/lib/pageIcon'
  import Outline from './Outline.svelte'
  import WikiHome from './WikiHome.svelte'

  let articleEl: HTMLElement | null = $state(null)

  /** Page id whose scroll we last restored / are tracking (skip save during restore). */
  let trackedPageId: string | null = null
  let restoring = false
  let scrollRaf = 0

  const known = $derived.by(() => {
    const titles = new Set<string>()
    const slugs = new Set<string>()
    for (const p of wiki.pages) {
      if (p.title) titles.add(p.title)
      if (p.slug) slugs.add(p.slug)
    }
    return { titles, slugs }
  })

  const html = $derived(
    wiki.current ? renderWikiHtml(wiki.current.content, known.titles, known.slugs) : '',
  )

  /** Catalog slug preferred (same key as SideNav), then wiki:// uri tail, then id. */
  const pageSlug = $derived.by(() => {
    const cur = wiki.current
    if (!cur) return ''
    const meta = wiki.pages.find((p) => p.id === cur.id)
    if (meta?.slug) return meta.slug
    const fromUri = cur.uri?.replace(/^wiki:\/\//, '').trim()
    if (fromUri) return fromUri
    return cur.id
  })

  const currentIcon = $derived(pageIcon(pageSlug))

  function persistScroll() {
    const el = articleEl
    const id = trackedPageId
    if (!el || !id || restoring) return
    wiki.saveScrollPosition(id, el.scrollTop)
  }

  function onScroll() {
    if (restoring) return
    if (scrollRaf) cancelAnimationFrame(scrollRaf)
    scrollRaf = requestAnimationFrame(() => {
      scrollRaf = 0
      persistScroll()
    })
  }

  async function restoreScroll(pageId: string) {
    const el = articleEl
    if (!el) return
    const y = wiki.getScrollPosition(pageId)
    restoring = true
    el.scrollTop = y
    // Content may reflow after fonts/images; re-apply once after paint.
    await tick()
    requestAnimationFrame(() => {
      if (articleEl && trackedPageId === pageId) {
        articleEl.scrollTop = y
      }
      restoring = false
    })
  }

  let prevPageId: string | null = null
  $effect(() => {
    const id = wiki.current?.id ?? null
    const prev = prevPageId
    prevPageId = id
    if (prev && articleEl && !restoring && prev !== id) {
      wiki.saveScrollPosition(prev, articleEl.scrollTop)
    }
    trackedPageId = id
    if (!id) return
    // Wait for page body to render before setting scrollTop.
    void tick().then(() => restoreScroll(id))
  })

  // After html recompute (catalog known-set changes), keep position for same page.
  let prevHtml = ''
  $effect(() => {
    const h = html
    const changed = prevHtml !== '' && h !== prevHtml
    prevHtml = h
    if (!changed) return
    const id = wiki.current?.id
    if (!id || id !== trackedPageId) return
    void tick().then(() => {
      if (!restoring && articleEl) {
        const y = wiki.getScrollPosition(id)
        if (Math.abs(articleEl.scrollTop - y) > 2) {
          void restoreScroll(id)
        }
      }
    })
  })

  onDestroy(() => {
    if (scrollRaf) cancelAnimationFrame(scrollRaf)
    persistScroll()
  })

  function onClick(e: MouseEvent) {
    const t = e.target as HTMLElement | null
    const a = t?.closest('a[data-wikilink]') as HTMLElement | null
    if (!a) return
    e.preventDefault()
    persistScroll()
    const key = a.getAttribute('data-wikilink') || ''
    const page = wiki.pages.find(
      (p) =>
        p.title === key ||
        p.slug === key ||
        p.title.toLowerCase() === key.toLowerCase() ||
        p.slug.toLowerCase() === key.toLowerCase(),
    )
    if (page) goWiki(page.id)
  }
</script>

<article bind:this={articleEl} class="article" onscroll={onScroll}>
  {#if !wiki.current && !wiki.loading}
    <WikiHome />
  {:else if wiki.loading && !wiki.current}
    <div class="empty">{ui.t('loading')}</div>
  {:else if wiki.current}
    <div class="page-row">
      <div class="main-col">
        <header>
          <div class="title-row">
            <span class="page-icon" aria-hidden="true" title={pageSlug || undefined}>
              {currentIcon}
            </span>
            <h1>{wiki.current.title}</h1>
          </div>
          <div class="meta">
            <span>{wiki.current.layer}</span>
            <span>{wiki.current.kind}</span>
            <span class="uri">{wiki.current.uri}</span>
          </div>
        </header>
        <!-- markdown is escaped inside renderWikiHtml; wikilinks route via onClick -->
        <div class="body prose" onclick={onClick} role="document">
          {@html html}
        </div>
      </div>
      <Outline />
    </div>
  {/if}
</article>

<style>
  .article {
    flex: 1;
    overflow: auto;
    padding: 28px 16px 80px;
    width: 100%;
  }
  .page-row {
    display: flex;
    align-items: flex-start;
    justify-content: center;
    gap: 4px;
    max-width: 1080px;
    margin: 0 auto;
    width: 100%;
  }
  .main-col {
    flex: 1;
    min-width: 0;
    max-width: 860px;
    padding: 0 32px;
  }
  .empty {
    padding: 80px 24px;
    text-align: center;
    color: var(--text-muted);
  }
  .title-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    margin-bottom: 8px;
  }
  .page-icon {
    flex-shrink: 0;
    font-size: 36px;
    line-height: 1.15;
    user-select: none;
  }
  header h1 {
    margin: 0;
    font-size: 36px;
    letter-spacing: -0.03em;
    line-height: 1.15;
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: 10px;
    color: var(--text-faint);
    font-size: 12px;
    margin-bottom: 24px;
  }
  .uri {
    font-family: var(--mono);
  }
  .body {
    line-height: 1.65;
    font-size: 16px;
  }
  .prose :global(h1),
  .prose :global(h2),
  .prose :global(h3) {
    letter-spacing: -0.02em;
    margin: 1.4em 0 0.5em;
    scroll-margin-top: 16px;
  }
  .prose :global(a.wikilink) {
    color: var(--link);
    text-decoration: underline;
    text-underline-offset: 2px;
    cursor: pointer;
  }
  .prose :global(a.wikilink.missing) {
    color: var(--link-missing);
    text-decoration-style: dashed;
  }
  .prose :global(code) {
    font-family: var(--mono);
    font-size: 0.9em;
    background: var(--bg-hover);
    padding: 0.1em 0.35em;
    border-radius: 4px;
  }
  .prose :global(pre) {
    background: var(--bg-sidebar);
    border: 1px solid var(--border);
    border-radius: 10px;
    padding: 12px 14px;
    overflow: auto;
  }
  .prose :global(pre code) {
    background: none;
    padding: 0;
  }
  .prose :global(blockquote) {
    margin: 0.8em 0;
    padding-left: 14px;
    border-left: 3px solid var(--border);
    color: var(--text-muted);
  }
  .prose :global(.tag) {
    color: var(--ok);
    font-weight: 500;
  }
  .prose :global(table) {
    border-collapse: collapse;
    margin: 0.8em 0;
    font-size: 14px;
  }
  .prose :global(th),
  .prose :global(td) {
    border: 1px solid var(--border);
    padding: 6px 10px;
    text-align: left;
  }
  .prose :global(th) {
    background: var(--bg-elevated);
  }
</style>
