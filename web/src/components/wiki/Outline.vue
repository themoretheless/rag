<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useWikiStore } from '@/stores/wiki'
import { useUiStore } from '@/stores/ui'
import { extractHeadings } from '@/lib/markdown'

const wiki = useWikiStore()
const ui = useUiStore()
const activeId = ref<string | null>(null)
const collapsed = ref(false)

const headings = computed(() => {
  if (!wiki.current || wiki.editing) return []
  return extractHeadings(wiki.current.content)
})

const visible = computed(() => headings.value.length > 0)

const minLevel = computed(() => {
  if (!headings.value.length) return 1
  return Math.min(...headings.value.map((h) => h.level))
})

function scrollTo(id: string) {
  const el = document.getElementById(id)
  if (!el) return
  el.scrollIntoView({ behavior: 'smooth', block: 'start' })
  activeId.value = id
}

let observer: IntersectionObserver | null = null

function disconnectObserver() {
  observer?.disconnect()
  observer = null
}

function bindObserver() {
  disconnectObserver()
  if (!headings.value.length || typeof IntersectionObserver === 'undefined') return

  const root = document.querySelector('.article') as HTMLElement | null
  observer = new IntersectionObserver(
    (entries) => {
      const visibleEntries = entries
        .filter((e) => e.isIntersecting)
        .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)
      const top = visibleEntries[0]
      if (top?.target instanceof HTMLElement && top.target.id) {
        activeId.value = top.target.id
      }
    },
    {
      root: root ?? null,
      rootMargin: '0px 0px -55% 0px',
      threshold: [0, 0.1, 1],
    },
  )

  for (const h of headings.value) {
    const el = document.getElementById(h.id)
    if (el) observer.observe(el)
  }
}

watch(
  () => [wiki.current?.id, wiki.current?.content, wiki.editing] as const,
  async () => {
    activeId.value = null
    await nextTick()
    bindObserver()
  },
)

onMounted(async () => {
  await nextTick()
  bindObserver()
})

onUnmounted(() => {
  disconnectObserver()
})
</script>

<template>
  <nav v-if="visible" class="outline" :aria-label="ui.t('outline')">
    <button
      type="button"
      class="head"
      :aria-expanded="!collapsed"
      @click="collapsed = !collapsed"
    >
      <span>{{ ui.t('outline') }}</span>
      <span class="chev" :class="{ open: !collapsed }">▾</span>
    </button>
    <ul v-show="!collapsed">
      <li
        v-for="h in headings"
        :key="h.id"
        :class="[
          `lvl-${h.level}`,
          { active: activeId === h.id },
        ]"
        :style="{ paddingLeft: `${10 + (h.level - minLevel) * 12}px` }"
      >
        <button type="button" class="link" @click="scrollTo(h.id)">
          {{ h.text }}
        </button>
      </li>
    </ul>
  </nav>
</template>

<style scoped>
.outline {
  position: sticky;
  top: 16px;
  align-self: flex-start;
  width: 180px;
  flex-shrink: 0;
  max-height: calc(100vh - var(--topbar-h) - 80px);
  overflow: auto;
  padding: 4px 0 16px;
  margin-right: 8px;
}
.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  border: none;
  background: transparent;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.06em;
  padding: 6px 10px;
  cursor: pointer;
  border-radius: 6px;
}
.head:hover {
  background: var(--bg-hover);
  color: var(--text-muted);
}
.chev {
  font-size: 10px;
  transform: rotate(-90deg);
  transition: transform 0.12s ease;
  opacity: 0.7;
}
.chev.open {
  transform: rotate(0deg);
}
ul {
  list-style: none;
  margin: 4px 0 0;
  padding: 0;
  border-left: 1px solid var(--border);
}
li {
  margin: 0;
}
.link {
  display: block;
  width: 100%;
  text-align: left;
  border: none;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.35;
  padding: 5px 8px 5px 0;
  cursor: pointer;
  border-radius: 0 6px 6px 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.link:hover {
  color: var(--text);
  background: var(--bg-hover);
}
li.active .link {
  color: var(--accent);
  font-weight: 600;
  box-shadow: inset 2px 0 0 var(--accent);
}
.lvl-1 .link {
  font-weight: 500;
}

@media (max-width: 1100px) {
  .outline {
    display: none;
  }
}
</style>
