<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useRouter } from 'vue-router'
import { useWikiStore } from '@/stores/wiki'
import { useUiStore } from '@/stores/ui'

const wiki = useWikiStore()
const ui = useUiStore()
const router = useRouter()

const historyOpen = ref(false)
const historyBtn = ref<HTMLElement | null>(null)

/** Catalog row for the open page (category, title fallback). */
const catalogPage = computed(() => {
  const id = wiki.current?.id
  if (!id) return null
  return wiki.pages.find((p) => p.id === id) ?? null
})

const category = computed(() => {
  const c = catalogPage.value?.category
  return c && c.trim() ? c.trim() : null
})

const pageTitle = computed(() => {
  if (wiki.editing && wiki.draftTitle.trim()) return wiki.draftTitle.trim()
  return (
    wiki.current?.title ||
    catalogPage.value?.title ||
    catalogPage.value?.slug ||
    ui.t('untitled')
  )
})

type Crumb = {
  key: string
  label: string
  link?: boolean
}

const crumbs = computed<Crumb[]>(() => {
  const out: Crumb[] = [{ key: 'wiki', label: ui.t('wiki'), link: true }]
  if (!wiki.current) return out
  if (category.value) {
    out.push({ key: 'cat', label: category.value })
  }
  out.push({ key: 'title', label: pageTitle.value })
  return out
})

/** History stack (oldest first in store); show newest first in the menu. */
const historyEntries = computed(() => {
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
  historyOpen.value = false
  void router.push({ name: 'wiki' })
}

/** Pop history and keep the route in sync via replace (no extra pushHistory). */
function goBack() {
  const prev = wiki.history[wiki.history.length - 1]
  if (!prev) return
  historyOpen.value = false
  wiki.goBack()
  void router.replace({ name: 'wiki', params: { id: prev } })
}

/**
 * Jump to a stack entry: truncate after it, goBack once (opens without re-push),
 * then replace the URL so the route matches current.
 */
function openHistoryId(id: string) {
  historyOpen.value = false
  const stack = wiki.history
  const idx = stack.lastIndexOf(id)
  if (idx < 0) {
    void router.push({ name: 'wiki', params: { id } })
    return
  }
  stack.splice(idx + 1)
  wiki.goBack()
  void router.replace({ name: 'wiki', params: { id } })
}

function toggleHistory() {
  if (!wiki.history.length) return
  historyOpen.value = !historyOpen.value
}

function onDocClick(e: MouseEvent) {
  if (!historyOpen.value) return
  const t = e.target as Node | null
  if (historyBtn.value?.contains(t)) return
  historyOpen.value = false
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') historyOpen.value = false
}

function showInGraph() {
  const id = wiki.current?.id
  if (!id) return
  void router.push({ name: 'graph', query: { seed: id } })
}

async function onSave() {
  try {
    await wiki.save()
  } catch {
    ui.toast(ui.t('saveFailed'), 'error')
  }
}

onMounted(() => {
  document.addEventListener('click', onDocClick)
  document.addEventListener('keydown', onKey)
})
onUnmounted(() => {
  document.removeEventListener('click', onDocClick)
  document.removeEventListener('keydown', onKey)
})
</script>

<template>
  <div class="bar">
    <div class="left">
      <div class="back-group" ref="historyBtn">
        <button
          type="button"
          class="ghost back"
          :disabled="!wiki.history.length"
          :title="ui.t('back')"
          @click="goBack"
        >
          {{ ui.t('back') }}
        </button>
        <button
          type="button"
          class="ghost hist-toggle"
          :disabled="!wiki.history.length"
          :aria-expanded="historyOpen"
          :title="ui.t('history')"
          @click.stop="toggleHistory"
        >
          ▾
        </button>
        <div v-if="historyOpen && historyEntries.length" class="hist-menu" role="menu">
          <button
            v-for="h in historyEntries"
            :key="h.id"
            type="button"
            class="hist-item"
            role="menuitem"
            @click="openHistoryId(h.id)"
          >
            <span class="hist-title">{{ h.title }}</span>
            <span v-if="h.category" class="hist-cat">{{ h.category }}</span>
          </button>
        </div>
      </div>

      <nav class="crumbs" :aria-label="ui.t('breadcrumb')">
        <template v-for="(c, i) in crumbs" :key="c.key">
          <span v-if="i > 0" class="sep" aria-hidden="true">/</span>
          <button
            v-if="c.link && i < crumbs.length - 1"
            type="button"
            class="crumb link"
            @click="goWikiRoot"
          >
            {{ c.label }}
          </button>
          <span
            v-else
            class="crumb"
            :class="{ current: i === crumbs.length - 1 && !!wiki.current }"
            :title="c.label"
          >
            {{ c.label }}
          </span>
        </template>
      </nav>

      <span v-if="wiki.current?.revision != null" class="rev">r{{ wiki.current.revision }}</span>
    </div>
    <div class="right">
      <template v-if="wiki.current && !wiki.editing">
        <button
          type="button"
          class="ghost pin"
          :class="{ on: wiki.isFavorite(wiki.current.id) }"
          :title="wiki.isFavorite(wiki.current.id) ? ui.t('unpinFavorite') : ui.t('pinFavorite')"
          :aria-pressed="wiki.isFavorite(wiki.current.id)"
          @click="wiki.toggleFavorite(wiki.current.id)"
        >
          {{ wiki.isFavorite(wiki.current.id) ? '★' : '☆' }}
        </button>
        <button type="button" class="ghost" @click="showInGraph">{{ ui.t('showInGraph') }}</button>
        <button type="button" class="primary" @click="wiki.startEdit()">{{ ui.t('edit') }}</button>
      </template>
      <template v-else-if="wiki.editing">
        <button type="button" class="ghost" @click="wiki.cancelEdit()">{{ ui.t('cancel') }}</button>
        <button type="button" class="primary" :disabled="!wiki.dirty" @click="onSave">
          {{ ui.t('save') }}
        </button>
      </template>
    </div>
  </div>
</template>

<style scoped>
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
  color: #e2b340;
}
</style>
