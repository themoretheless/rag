<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useWikiStore } from '@/stores/wiki'
import { useUiStore } from '@/stores/ui'
import { renderWikiHtml } from '@/lib/markdown'
import { pageIcon } from '@/lib/pageIcon'
import Outline from '@/components/wiki/Outline.vue'

const wiki = useWikiStore()
const ui = useUiStore()
const router = useRouter()
const articleEl = ref<HTMLElement | null>(null)

/** Page id whose scroll we last restored / are tracking (skip save during restore). */
let trackedPageId: string | null = null
let restoring = false
let scrollRaf = 0

const known = computed(() => {
  const titles = new Set<string>()
  const slugs = new Set<string>()
  for (const p of wiki.pages) {
    if (p.title) titles.add(p.title)
    if (p.slug) slugs.add(p.slug)
  }
  return { titles, slugs }
})

const html = computed(() => {
  if (!wiki.current) return ''
  return renderWikiHtml(wiki.current.content, known.value.titles, known.value.slugs)
})

/** Catalog slug preferred (same key as SideNav), then wiki:// uri tail, then id. */
const pageSlug = computed(() => {
  const cur = wiki.current
  if (!cur) return ''
  const meta = wiki.pages.find((p) => p.id === cur.id)
  if (meta?.slug) return meta.slug
  const fromUri = cur.uri?.replace(/^wiki:\/\//, '').trim()
  if (fromUri) return fromUri
  return cur.id
})

const currentIcon = computed(() => pageIcon(pageSlug.value))

function persistScroll() {
  const el = articleEl.value
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
  const el = articleEl.value
  if (!el) return
  const y = wiki.getScrollPosition(pageId)
  restoring = true
  el.scrollTop = y
  // Content may reflow after fonts/images; re-apply once after paint.
  await nextTick()
  requestAnimationFrame(() => {
    if (articleEl.value && trackedPageId === pageId) {
      articleEl.value.scrollTop = y
    }
    restoring = false
  })
}

watch(
  () => wiki.current?.id,
  async (id, prevId) => {
    if (prevId && articleEl.value && !restoring) {
      wiki.saveScrollPosition(prevId, articleEl.value.scrollTop)
    }
    trackedPageId = id ?? null
    if (!id) return
    // Wait for page body to render before setting scrollTop.
    await nextTick()
    await restoreScroll(id)
  },
  { immediate: true },
)

// After html recompute (catalog known-set changes), keep position for same page.
watch(html, async () => {
  const id = wiki.current?.id
  if (!id || id !== trackedPageId) return
  await nextTick()
  if (!restoring && articleEl.value) {
    const y = wiki.getScrollPosition(id)
    if (Math.abs(articleEl.value.scrollTop - y) > 2) {
      await restoreScroll(id)
    }
  }
})

onBeforeUnmount(() => {
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
  if (page) void router.push({ name: 'wiki', params: { id: page.id } })
}
</script>

<template>
  <article ref="articleEl" class="article" @scroll.passive="onScroll">
    <div v-if="!wiki.current && !wiki.loading" class="empty">
      <h2>{{ ui.t('selectPage') }}</h2>
      <p>{{ ui.t('selectPageHint') }}</p>
      <p class="hint">{{ ui.t('linksHint') }}</p>
    </div>
    <div v-else-if="wiki.loading && !wiki.current" class="empty">{{ ui.t('loading') }}</div>
    <div v-else-if="wiki.current" class="page-row">
      <div class="main-col">
        <header>
          <div class="title-row">
            <span class="page-icon" aria-hidden="true" :title="pageSlug || undefined">{{
              currentIcon
            }}</span>
            <h1>{{ wiki.current.title }}</h1>
          </div>
          <div class="meta">
            <span>{{ wiki.current.layer }}</span>
            <span>{{ wiki.current.kind }}</span>
            <span class="uri">{{ wiki.current.uri }}</span>
          </div>
        </header>
        <div class="body prose" v-html="html" @click="onClick" />
      </div>
      <Outline />
    </div>
  </article>
</template>

<style scoped>
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
.hint {
  font-size: 13px;
  color: var(--text-faint);
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
:deep(.prose h1),
:deep(.prose h2),
:deep(.prose h3) {
  letter-spacing: -0.02em;
  margin: 1.4em 0 0.5em;
  scroll-margin-top: 16px;
}
:deep(.prose a.wikilink) {
  color: var(--link);
  text-decoration: underline;
  text-underline-offset: 2px;
  cursor: pointer;
}
:deep(.prose a.wikilink.missing) {
  color: var(--link-missing);
  text-decoration-style: dashed;
}
:deep(.prose code) {
  font-family: var(--mono);
  font-size: 0.9em;
  background: var(--bg-hover);
  padding: 0.1em 0.35em;
  border-radius: 4px;
}
:deep(.prose pre) {
  background: var(--bg-sidebar);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 12px 14px;
  overflow: auto;
}
:deep(.prose pre code) {
  background: none;
  padding: 0;
}
:deep(.prose blockquote) {
  margin: 0.8em 0;
  padding-left: 14px;
  border-left: 3px solid var(--border);
  color: var(--text-muted);
}
:deep(.prose .tag) {
  color: var(--ok);
  font-weight: 500;
}
</style>
