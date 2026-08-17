<script setup lang="ts">
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import { useGraphStore } from '@/stores/graph'

const graph = useGraphStore()
const router = useRouter()

const node = computed(() => graph.nodes.find((n) => n.id === graph.selectedId) || null)

const degree = computed(() => {
  if (!node.value) return 0
  const id = node.value.id
  return graph.edges.filter((e) => e.source_id === id || e.target_id === id).length
})

function openWiki() {
  const docId = node.value?.document_id
  if (docId) void router.push({ name: 'wiki', params: { id: docId } })
}
</script>

<template>
  <aside v-if="node" class="insp">
    <button class="x" type="button" @click="graph.select(null)">×</button>
    <h3>{{ node.label }}</h3>
    <dl>
      <dt>kind</dt>
      <dd>{{ node.kind || '—' }}</dd>
      <dt>id</dt>
      <dd class="mono">{{ node.id }}</dd>
      <dt>document</dt>
      <dd class="mono">{{ node.document_id || '—' }}</dd>
      <dt>degree</dt>
      <dd>{{ degree }}</dd>
      <dt>wing/room</dt>
      <dd>{{ node.wing || '—' }} / {{ node.room || '—' }}</dd>
    </dl>
    <button v-if="node.document_id" type="button" class="open" @click="openWiki">
      Open as wiki
    </button>
  </aside>
</template>

<style scoped>
.insp {
  position: absolute;
  top: 12px;
  right: 12px;
  width: 280px;
  padding: 14px;
  border-radius: 14px;
  background: var(--graph-panel-bg);
  border: 1px solid var(--graph-panel-border);
  box-shadow: var(--graph-shadow);
  color: var(--graph-panel-text);
  backdrop-filter: blur(10px);
}
.x {
  position: absolute;
  right: 8px;
  top: 6px;
  border: none;
  background: transparent;
  color: var(--graph-panel-muted);
  font-size: 18px;
  cursor: pointer;
}
h3 {
  margin: 0 28px 10px 0;
  font-size: 16px;
  letter-spacing: -0.02em;
}
dl {
  display: grid;
  grid-template-columns: 72px 1fr;
  gap: 6px 8px;
  margin: 0 0 12px;
  font-size: 12px;
}
dt {
  color: var(--graph-panel-muted);
}
dd {
  margin: 0;
  word-break: break-all;
}
.mono {
  font-family: var(--mono);
  font-size: 11px;
}
.open {
  width: 100%;
  border: none;
  border-radius: 8px;
  padding: 8px;
  background: linear-gradient(135deg, var(--graph-node-wiki), var(--accent));
  color: white;
  font-weight: 600;
  cursor: pointer;
}
</style>
