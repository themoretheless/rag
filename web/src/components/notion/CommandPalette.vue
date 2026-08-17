<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { useRouter } from 'vue-router'
import { useUiStore } from '@/stores/ui'
import { useWikiStore } from '@/stores/wiki'

const ui = useUiStore()
const wiki = useWikiStore()
const router = useRouter()
const q = ref('')
const active = ref(0)

const results = computed(() => {
  const query = q.value.trim().toLowerCase()
  const pages = wiki.pages
  const filtered = !query
    ? pages.slice(0, 12)
    : pages
        .filter((p) => `${p.title} ${p.slug} ${p.summary ?? ''}`.toLowerCase().includes(query))
        .slice(0, 12)
  return filtered
})

watch(
  () => ui.commandOpen,
  (open) => {
    if (open) {
      q.value = ''
      active.value = 0
      if (!wiki.pages.length) void wiki.loadCatalog()
    }
  },
)

function close() {
  ui.closeCommand()
}

function go(id: string) {
  close()
  void router.push({ name: 'wiki', params: { id } })
}

function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape') close()
  if (e.key === 'ArrowDown') {
    e.preventDefault()
    active.value = Math.min(active.value + 1, results.value.length - 1)
  }
  if (e.key === 'ArrowUp') {
    e.preventDefault()
    active.value = Math.max(active.value - 1, 0)
  }
  if (e.key === 'Enter' && results.value[active.value]) {
    go(results.value[active.value].id)
  }
}
</script>

<template>
  <div v-if="ui.commandOpen" class="overlay" @click.self="close" @keydown="onKey">
    <div class="panel" role="dialog" aria-modal="true">
      <input
        v-model="q"
        class="input"
        type="search"
        placeholder="Jump to page…"
        autofocus
        @keydown="onKey"
      />
      <ul>
        <li
          v-for="(p, i) in results"
          :key="p.id"
          :class="{ active: i === active }"
          @mouseenter="active = i"
          @click="go(p.id)"
        >
          <span class="t">{{ p.title || p.slug }}</span>
          <span class="s">{{ p.slug }}</span>
        </li>
        <li v-if="!results.length" class="empty">No matches</li>
      </ul>
      <div class="hint">
        <span>↑↓ navigate</span>
        <span>↵ open</span>
        <span>esc close</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
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
  display: flex;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border-radius: 8px;
  cursor: pointer;
}
li.active,
li:hover {
  background: var(--bg-active);
}
.t {
  font-weight: 500;
}
.s {
  color: var(--text-faint);
  font-size: 12px;
  font-family: var(--mono);
}
.empty {
  color: var(--text-muted);
  justify-content: center;
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
