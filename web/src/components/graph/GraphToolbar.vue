<script setup lang="ts">
import { useGraphStore } from '@/stores/graph'

const graph = useGraphStore()

const emit = defineEmits<{
  expand: []
  full: []
}>()
</script>

<template>
  <div class="tb">
    <div class="left">
      <strong>Graph</strong>
      <span class="chip">{{ graph.nodes.length }} nodes</span>
      <span class="chip">{{ graph.edges.length }} edges</span>
      <span class="chip">{{ graph.mode }}</span>
    </div>
    <div class="mid">
      <input v-model="graph.seed" type="text" placeholder="Seed id / label / document_id" />
      <label>
        depth
        <input v-model.number="graph.depth" type="number" min="1" max="3" />
      </label>
      <label>
        max
        <input v-model.number="graph.maxNodes" type="number" min="20" max="2000" step="20" />
      </label>
      <label class="check">
        <input v-model="graph.includeTags" type="checkbox" />
        tags
      </label>
    </div>
    <div class="right">
      <button type="button" @click="emit('full')">Full graph</button>
      <button type="button" class="accent" @click="emit('expand')">Expand seed</button>
    </div>
  </div>
</template>

<style scoped>
.tb {
  display: flex;
  flex-wrap: wrap;
  gap: 10px;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px;
  border-bottom: 1px solid var(--graph-panel-border);
  background: var(--graph-panel-bg);
  color: var(--graph-panel-text);
  backdrop-filter: blur(8px);
}
.left,
.mid,
.right {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.chip {
  font-size: 11px;
  padding: 3px 8px;
  border-radius: 999px;
  background: var(--graph-panel-chip);
  color: var(--graph-panel-muted);
}
input[type='text'],
input[type='number'] {
  background: var(--graph-panel-input);
  border: 1px solid var(--graph-panel-border);
  border-radius: 8px;
  padding: 6px 10px;
  color: var(--graph-panel-text);
  outline: none;
}
input[type='text'] {
  min-width: 220px;
}
input[type='number'] {
  width: 64px;
}
label {
  font-size: 12px;
  color: var(--graph-panel-muted);
  display: flex;
  align-items: center;
  gap: 6px;
}
button {
  border: 1px solid var(--graph-panel-border);
  background: var(--graph-panel-chip);
  color: var(--graph-panel-text);
  border-radius: 8px;
  padding: 7px 12px;
  cursor: pointer;
}
button.accent {
  background: linear-gradient(135deg, var(--graph-node-wiki), var(--accent));
  border: none;
  color: #fff;
  font-weight: 600;
}
button:hover {
  filter: brightness(1.08);
}
</style>
