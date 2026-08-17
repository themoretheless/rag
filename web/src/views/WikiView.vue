<script setup lang="ts">
import { onMounted, onUnmounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import { useWikiStore } from '@/stores/wiki'
import { useUiStore } from '@/stores/ui'
import WikiArticle from '@/components/wiki/WikiArticle.vue'
import WikiEditor from '@/components/wiki/WikiEditor.vue'
import BacklinksPanel from '@/components/wiki/BacklinksPanel.vue'
import WikiToolbar from '@/components/wiki/WikiToolbar.vue'

const props = defineProps<{ id?: string }>()
const route = useRoute()
const wiki = useWikiStore()
const ui = useUiStore()

function isTypedInputTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false
  const tag = target.tagName
  if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true
  if (target.isContentEditable) return true
  return Boolean(target.closest('input, textarea, select, [contenteditable="true"]'))
}

function onKey(e: KeyboardEvent) {
  if (e.metaKey || e.ctrlKey || e.altKey) return
  if (ui.commandOpen) return

  // Esc cancels edit even when the editor fields are focused (leave edit mode).
  if (e.key === 'Escape' && wiki.editing) {
    e.preventDefault()
    wiki.cancelEdit()
    return
  }

  // Single-letter shortcuts stay typed-input-safe.
  if (isTypedInputTarget(e.target)) return
  if (e.key.toLowerCase() === 'e' && !wiki.editing && wiki.current) {
    e.preventDefault()
    wiki.startEdit()
  }
}

onMounted(() => window.addEventListener('keydown', onKey))
onUnmounted(() => window.removeEventListener('keydown', onKey))

watch(
  () => props.id || (route.params.id as string | undefined),
  (id) => {
    if (id) void wiki.openPage(id)
  },
  { immediate: true },
)
</script>

<template>
  <div class="wiki">
    <section class="center">
      <WikiToolbar />
      <WikiEditor v-if="wiki.editing" />
      <WikiArticle v-else />
    </section>
    <BacklinksPanel />
  </div>
</template>

<style scoped>
.wiki {
  display: flex;
  height: 100%;
  min-height: 0;
}
.center {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
</style>
