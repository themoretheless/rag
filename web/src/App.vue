<script setup lang="ts">
import { onMounted } from 'vue'
import { useUiStore } from '@/stores/ui'
import AppShell from '@/components/layout/AppShell.vue'
import CommandPalette from '@/components/notion/CommandPalette.vue'
import ToastHost from '@/components/layout/ToastHost.vue'

const ui = useUiStore()
onMounted(() => {
  ui.hydrateTheme()
  ui.checkHealth()
})
</script>

<template>
  <AppShell>
    <router-view />
  </AppShell>
  <CommandPalette />
  <ToastHost />
</template>
