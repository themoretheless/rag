<script setup lang="ts">
import { useRouter } from 'vue-router'
import { useWikiStore } from '@/stores/wiki'
import { useUiStore } from '@/stores/ui'

const wiki = useWikiStore()
const ui = useUiStore()
const router = useRouter()

function open(id: string, label: string) {
  const byId = wiki.pages.find((p) => p.id === id)
  if (byId) {
    void router.push({ name: 'wiki', params: { id: byId.id } })
    return
  }
  const byLabel = wiki.pages.find(
    (p) => p.title === label || p.title.toLowerCase() === label.toLowerCase(),
  )
  if (byLabel) void router.push({ name: 'wiki', params: { id: byLabel.id } })
}
</script>

<template>
  <aside v-if="wiki.current" class="right">
    <h3>{{ ui.t('backlinks') }}</h3>
    <p class="sub">{{ ui.t('incomingWikilinks') }}</p>
    <ul v-if="wiki.backlinks.length">
      <li v-for="b in wiki.backlinks" :key="b.id + b.label" @click="open(b.id, b.label)">
        {{ b.label }}
      </li>
    </ul>
    <p v-else class="empty">{{ ui.t('noIncoming') }}</p>
    <h3 class="props">{{ ui.t('properties') }}</h3>
    <dl>
      <dt>{{ ui.t('kind') }}</dt>
      <dd>{{ wiki.current.kind }}</dd>
      <dt>{{ ui.t('layer') }}</dt>
      <dd>{{ wiki.current.layer }}</dd>
      <dt>{{ ui.t('revision') }}</dt>
      <dd>r{{ wiki.current.revision ?? '—' }}</dd>
      <dt>{{ ui.t('updated') }}</dt>
      <dd>{{ wiki.current.updated_at || '—' }}</dd>
    </dl>
  </aside>
</template>

<style scoped>
.right {
  width: var(--right-w);
  flex-shrink: 0;
  border-left: 1px solid var(--border);
  background: var(--bg-sidebar);
  padding: 16px 14px;
  overflow: auto;
}
h3 {
  margin: 0 0 4px;
  font-size: 12px;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  color: var(--text-faint);
}
.sub {
  margin: 0 0 10px;
  font-size: 12px;
  color: var(--text-muted);
}
ul {
  list-style: none;
  margin: 0 0 20px;
  padding: 0;
}
li {
  padding: 8px 10px;
  border-radius: 8px;
  cursor: pointer;
  color: var(--link);
}
li:hover {
  background: var(--bg-hover);
}
.empty {
  color: var(--text-faint);
  font-size: 13px;
}
.props {
  margin-top: 8px;
}
dl {
  display: grid;
  grid-template-columns: auto 1fr;
  gap: 6px 10px;
  font-size: 12px;
  margin: 8px 0 0;
}
dt {
  color: var(--text-faint);
}
dd {
  margin: 0;
  word-break: break-all;
}
</style>
