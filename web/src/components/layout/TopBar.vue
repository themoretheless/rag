<script setup lang="ts">
import { computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useUiStore } from '@/stores/ui'

const ui = useUiStore()
const route = useRoute()
const router = useRouter()

const healthText = computed(() => {
  if (ui.healthError) return ui.t('offline')
  if (!ui.health) return '…'
  return ui.t('healthStats', {
    docs: ui.health.documents ?? 0,
    nodes: ui.health.nodes ?? 0,
  })
})
</script>

<template>
  <header class="top">
    <div class="left">
      <button class="icon" type="button" :title="ui.t('toggleSidebar')" @click="ui.toggleSidebar()">
        ☰
      </button>
      <span class="brand">rag-mcp</span>
      <nav class="tabs">
        <button
          type="button"
          :class="{ active: route.name === 'wiki' }"
          @click="router.push('/wiki')"
        >
          {{ ui.t('wiki') }}
        </button>
        <button
          type="button"
          :class="{ active: route.name === 'graph' }"
          @click="router.push('/graph')"
        >
          {{ ui.t('graph') }}
        </button>
        <button
          type="button"
          :class="{ active: route.name === 'search' }"
          @click="router.push('/search')"
        >
          {{ ui.t('search') }}
        </button>
      </nav>
    </div>
    <div class="right">
      <button class="cmd" type="button" @click="ui.openCommand()">
        <span>{{ ui.t('search') }}</span>
        <kbd>⌘K</kbd>
      </button>
      <button
        class="icon lang"
        type="button"
        :title="ui.t('lang')"
        @click="ui.toggleLocale()"
      >
        {{ ui.localeLabel }}
      </button>
      <button
        class="icon"
        type="button"
        :title="ui.t('theme')"
        @click="ui.setTheme(ui.theme === 'dark' ? 'light' : 'dark')"
      >
        {{ ui.theme === 'dark' ? '☾' : '☀' }}
      </button>
      <span
        class="health"
        :class="{ bad: !!ui.healthError, ok: ui.health?.ok }"
        :title="ui.healthError || ui.health?.db_path || ''"
      >
        {{ healthText }}
      </span>
    </div>
  </header>
</template>

<style scoped>
.top {
  height: var(--topbar-h);
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0 12px;
  border-bottom: 1px solid var(--border);
  background: var(--bg-elevated);
  gap: 12px;
}
.left,
.right {
  display: flex;
  align-items: center;
  gap: 10px;
}
.brand {
  font-weight: 600;
  letter-spacing: -0.02em;
}
.tabs {
  display: flex;
  gap: 4px;
  margin-left: 8px;
}
.tabs button,
.icon,
.cmd {
  border: none;
  background: transparent;
  border-radius: 8px;
  padding: 6px 10px;
  cursor: pointer;
  color: var(--text-muted);
}
.tabs button:hover,
.icon:hover,
.cmd:hover {
  background: var(--bg-hover);
  color: var(--text);
}
.tabs button.active {
  background: var(--bg-active);
  color: var(--text);
  font-weight: 600;
}
.cmd {
  display: flex;
  align-items: center;
  gap: 10px;
  border: 1px solid var(--border);
  min-width: 160px;
  justify-content: space-between;
  color: var(--text-muted);
}
.lang {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.04em;
  min-width: 36px;
  font-family: var(--mono);
}
kbd {
  font-family: var(--mono);
  font-size: 11px;
  opacity: 0.7;
}
.health {
  font-size: 12px;
  color: var(--text-faint);
}
.health.ok {
  color: var(--ok);
}
.health.bad {
  color: var(--danger);
}
</style>
