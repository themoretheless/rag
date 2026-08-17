<script setup lang="ts">
import { onMounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useUiStore } from '@/stores/ui'
import { useWikiStore } from '@/stores/wiki'
import { pageIcon } from '@/lib/pageIcon'

const ui = useUiStore()
const wiki = useWikiStore()
const route = useRoute()
const router = useRouter()

onMounted(() => {
  void wiki.loadCatalog()
})

watch(
  () => route.params.id,
  (id) => {
    if (typeof id === 'string' && id) void wiki.openPage(id)
  },
  { immediate: true },
)

function open(id: string) {
  void router.push({ name: 'wiki', params: { id } })
}

async function onNewPage() {
  try {
    const id = await wiki.createPage()
    if (!id) return
    if (route.params.id === id) {
      await wiki.openPage(id, false)
    } else {
      await router.push({ name: 'wiki', params: { id } })
    }
  } catch {
    // createPage already toasted the error
  }
}

function iconFor(p: { slug: string; id: string }) {
  return pageIcon(p.slug || p.id)
}

function onPin(e: Event, id: string) {
  e.stopPropagation()
  wiki.toggleFavorite(id)
}

function pageLabel(p: { title?: string | null; slug?: string | null }) {
  return p.title || p.slug || ''
}

function selectAll() {
  wiki.setFacet({ type: 'all' })
}

function selectWikiKind() {
  wiki.setFacet({ type: 'kind', value: 'wiki' })
}

function selectCategory(cat: string) {
  wiki.setFacet({ type: 'category', value: cat })
}
</script>

<template>
  <aside class="side" :class="{ hidden: ui.sidebarCollapsed }">
    <div v-if="wiki.favoritePages.length" class="section">
      <div class="head">
        <strong>{{ ui.t('favorites') }}</strong>
        <span class="count">{{ wiki.favoritePages.length }}</span>
      </div>
      <ul class="list favorites">
        <li
          v-for="p in wiki.favoritePages"
          :key="'fav-' + p.id"
          :class="{ active: wiki.current?.id === p.id }"
          @click="open(p.id)"
        >
          <div class="row">
            <span class="icon" aria-hidden="true">{{ iconFor(p) }}</span>
            <div class="text">
              <div class="title">{{ pageLabel(p) }}</div>
              <div class="meta">
                <span v-if="p.category">{{ p.category }}</span>
                <span v-if="p.revision != null">r{{ p.revision }}</span>
              </div>
            </div>
            <button
              type="button"
              class="pin on"
              :title="ui.t('unpinFavorite')"
              :aria-label="ui.t('unpinFavorite')"
              @click="onPin($event, p.id)"
            >
              ★
            </button>
          </div>
        </li>
      </ul>
    </div>

    <div v-if="wiki.recent.length" class="section recent-section">
      <div class="head">
        <strong>Recent</strong>
        <span class="count">{{ wiki.recent.length }}</span>
      </div>
      <ul class="list recent">
        <li
          v-for="p in wiki.recent"
          :key="'recent-' + p.id"
          :class="{ active: wiki.current?.id === p.id }"
          :title="p.slug"
          @click="open(p.id)"
        >
          <div class="row">
            <span class="icon" aria-hidden="true">{{ iconFor(p) }}</span>
            <div class="text">
              <div class="title">{{ pageLabel(p) }}</div>
            </div>
          </div>
        </li>
      </ul>
    </div>

    <div class="head">
      <strong>{{ ui.t('pages') }}</strong>
      <div class="head-actions">
        <button
          type="button"
          class="ghost new-page"
          :title="ui.t('newPage')"
          :disabled="wiki.creating"
          @click="onNewPage"
        >
          +
        </button>
        <button type="button" class="ghost" :title="ui.t('refresh')" @click="wiki.loadCatalog()">↻</button>
      </div>
    </div>

    <div class="chips" role="toolbar" :aria-label="ui.t('filterFacets')">
      <button
        type="button"
        class="chip"
        :class="{ active: wiki.facetIsAll() }"
        :aria-pressed="wiki.facetIsAll()"
        @click="selectAll"
      >
        {{ ui.t('facetAll') }}
      </button>
      <button
        type="button"
        class="chip"
        :class="{ active: wiki.facetIsKind('wiki') }"
        :aria-pressed="wiki.facetIsKind('wiki')"
        title="kind=wiki"
        @click="selectWikiKind"
      >
        {{ ui.t('wiki') }}
      </button>
      <button
        v-for="cat in wiki.categories"
        :key="'cat-' + cat"
        type="button"
        class="chip"
        :class="{ active: wiki.facetIsCategory(cat) }"
        :aria-pressed="wiki.facetIsCategory(cat)"
        :title="'category=' + cat"
        @click="selectCategory(cat)"
      >
        {{ cat }}
      </button>
    </div>

    <input
      v-model="wiki.filter"
      class="filter"
      type="search"
      :placeholder="ui.t('filterPages')"
      autocomplete="off"
    />
    <div v-if="wiki.loading && !wiki.pages.length" class="muted">{{ ui.t('loading') }}</div>
    <div v-else-if="wiki.error" class="err">{{ wiki.error }}</div>
    <ul v-else class="list">
      <li
        v-for="p in wiki.filtered"
        :key="p.id"
        :class="{ active: wiki.current?.id === p.id }"
        @click="open(p.id)"
      >
        <div class="row">
          <span class="icon" aria-hidden="true">{{ iconFor(p) }}</span>
          <div class="text">
            <div class="title">{{ pageLabel(p) }}</div>
            <div class="meta">
              <span v-if="p.category">{{ p.category }}</span>
              <span v-if="p.kind && p.kind !== 'wiki'">{{ p.kind }}</span>
              <span>r{{ p.revision }}</span>
            </div>
          </div>
          <button
            type="button"
            class="pin"
            :class="{ on: wiki.isFavorite(p.id) }"
            :title="wiki.isFavorite(p.id) ? ui.t('unpinFavorite') : ui.t('pinFavorite')"
            :aria-label="wiki.isFavorite(p.id) ? ui.t('unpinFavorite') : ui.t('pinFavorite')"
            :aria-pressed="wiki.isFavorite(p.id)"
            @click="onPin($event, p.id)"
          >
            {{ wiki.isFavorite(p.id) ? '★' : '☆' }}
          </button>
        </div>
      </li>
      <li v-if="!wiki.filtered.length" class="muted empty">{{ ui.t('noPages') }}</li>
    </ul>
  </aside>
</template>

<style scoped>
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
.section {
  flex-shrink: 0;
  border-bottom: 1px solid var(--border);
}
.section .list {
  flex: 0 1 auto;
  max-height: 40vh;
  overflow: auto;
}
.section.recent-section .list {
  max-height: 220px;
}
.section .list.recent li {
  padding-top: 5px;
  padding-bottom: 5px;
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
  overflow: auto;
  flex: 1;
}
.list li {
  padding: 6px 6px 6px 10px;
  border-radius: 8px;
  cursor: pointer;
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
}
.title {
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
  opacity: 0.55;
}
.list li:hover .pin,
.pin.on {
  opacity: 1;
}
.pin:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
.pin.on {
  color: #e2b340;
}
.muted {
  color: var(--text-muted);
  padding: 12px;
  font-size: 13px;
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
