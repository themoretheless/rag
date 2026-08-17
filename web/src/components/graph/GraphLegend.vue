<script setup lang="ts">
import { computed } from 'vue'
import { useGraphStore } from '@/stores/graph'
import { legendEntries } from '@/lib/forceLayout'

const graph = useGraphStore()

const entries = computed(() => legendEntries(graph.nodes))
</script>

<template>
  <div class="legend">
    <div v-for="e in entries" :key="`${e.source}:${e.label}`">
      <i :style="{ background: e.color, color: e.color }" />
      {{ e.label }}
    </div>
    <div class="edge">- wikilink</div>
  </div>
</template>

<style scoped>
.legend {
  position: absolute;
  left: 12px;
  bottom: 12px;
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 10px 12px;
  border-radius: 12px;
  background: rgba(8, 12, 22, 0.78);
  border: 1px solid rgba(255, 255, 255, 0.08);
  font-size: 12px;
  color: #cbd5e1;
  backdrop-filter: blur(8px);
  pointer-events: none;
  max-height: min(40vh, 280px);
  overflow-y: auto;
}
i {
  display: inline-block;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  margin-right: 8px;
  box-shadow: 0 0 10px currentColor;
  vertical-align: middle;
}
.edge {
  color: #a78bfa;
  opacity: 0.9;
}
</style>
