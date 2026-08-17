<script setup lang="ts">
import { useUiStore } from '@/stores/ui'
const ui = useUiStore()
</script>

<template>
  <div class="toasts">
    <div v-for="t in ui.toasts" :key="t.id" class="toast" :class="t.kind">
      {{ t.text }}
    </div>
  </div>
</template>

<style scoped>
.toasts {
  position: fixed;
  right: 16px;
  bottom: 16px;
  display: flex;
  flex-direction: column;
  gap: 8px;
  z-index: 1000;
  max-width: min(420px, 92vw);
}
.toast {
  padding: 10px 14px;
  border-radius: 10px;
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  box-shadow: var(--shadow);
  font-size: 13px;
}
.toast.ok {
  border-color: color-mix(in srgb, var(--ok) 40%, var(--border));
}
.toast.error {
  border-color: color-mix(in srgb, var(--danger) 50%, var(--border));
  color: var(--danger);
}
</style>
