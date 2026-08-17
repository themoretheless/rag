<script setup lang="ts">
import { ref } from 'vue'
import { useRouter } from 'vue-router'
import { useWikiStore } from '@/stores/wiki'
import { api } from '@/api/client'

const wiki = useWikiStore()
const router = useRouter()
const q = ref('')
const busy = ref(false)
const err = ref<string | null>(null)
const hits = ref<{ kind: string; title: string; id: string; detail?: string }[]>([])

async function run() {
  const query = q.value.trim()
  if (!query) return
  busy.value = true
  err.value = null
  hits.value = []
  try {
    if (!wiki.pages.length) await wiki.loadCatalog()
    const ql = query.toLowerCase()
    for (const p of wiki.pages) {
      if (`${p.title} ${p.slug} ${p.summary ?? ''}`.toLowerCase().includes(ql)) {
        hits.value.push({
          kind: 'wiki',
          title: p.title || p.slug,
          id: p.id,
          detail: p.summary || p.slug,
        })
      }
    }
    try {
      const node = (await api.findNode(query)) as {
        id?: string
        label?: string
        kind?: string
        document_id?: string | null
      }
      if (node?.id) {
        hits.value.unshift({
          kind: node.kind || 'node',
          title: node.label || node.id,
          id: node.document_id || node.id,
          detail: node.id,
        })
      }
    } catch {
      /* no graph hit */
    }
  } catch (e) {
    err.value = e instanceof Error ? e.message : String(e)
  } finally {
    busy.value = false
  }
}

function open(id: string) {
  void router.push({ name: 'wiki', params: { id } })
}
</script>

<template>
  <div class="search">
    <h1>Search</h1>
    <p class="sub">Notion-style jump across wiki catalog + graph find.</p>
    <form class="row" @submit.prevent="run">
      <input v-model="q" type="search" placeholder="Query…" autofocus />
      <button type="submit" :disabled="busy">{{ busy ? '…' : 'Search' }}</button>
    </form>
    <p v-if="err" class="err">{{ err }}</p>
    <ul>
      <li v-for="(h, i) in hits" :key="i" @click="open(h.id)">
        <span class="k">{{ h.kind }}</span>
        <div>
          <div class="t">{{ h.title }}</div>
          <div class="d">{{ h.detail }}</div>
        </div>
      </li>
      <li v-if="!hits.length && !busy" class="empty">No results yet</li>
    </ul>
  </div>
</template>

<style scoped>
.search {
  max-width: 720px;
  margin: 0 auto;
  padding: 32px 20px;
}
h1 {
  margin: 0 0 4px;
  letter-spacing: -0.03em;
}
.sub {
  color: var(--text-muted);
  margin: 0 0 20px;
}
.row {
  display: flex;
  gap: 8px;
}
input {
  flex: 1;
  border: 1px solid var(--border);
  background: var(--bg-elevated);
  border-radius: 10px;
  padding: 12px 14px;
  outline: none;
}
button {
  border: none;
  background: var(--accent);
  color: white;
  border-radius: 10px;
  padding: 0 16px;
  cursor: pointer;
  font-weight: 600;
}
ul {
  list-style: none;
  margin: 20px 0 0;
  padding: 0;
}
li {
  display: flex;
  gap: 12px;
  padding: 12px;
  border-radius: 10px;
  cursor: pointer;
  border: 1px solid transparent;
}
li:hover {
  background: var(--bg-hover);
  border-color: var(--border);
}
.k {
  font-size: 11px;
  text-transform: uppercase;
  color: var(--text-faint);
  min-width: 48px;
  padding-top: 3px;
}
.t {
  font-weight: 600;
}
.d {
  font-size: 13px;
  color: var(--text-muted);
}
.err {
  color: var(--danger);
}
.empty {
  color: var(--text-muted);
  justify-content: center;
}
</style>
