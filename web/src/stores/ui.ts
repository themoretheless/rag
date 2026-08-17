import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { api } from '@/api/client'
import type { HealthResponse } from '@/api/types'
import {
  formatMessage,
  type Locale,
  type MessageKey,
} from '@/i18n/messages'

export type Theme = 'dark' | 'light' | 'system'

const LOCALE_KEY = 'rag-locale'

export const useUiStore = defineStore('ui', () => {
  const theme = ref<Theme>('dark')
  const locale = ref<Locale>('en')
  const sidebarCollapsed = ref(false)
  const commandOpen = ref(false)
  const health = ref<HealthResponse | null>(null)
  const healthError = ref<string | null>(null)
  const toasts = ref<{ id: number; text: string; kind: 'info' | 'error' | 'ok' }[]>([])
  let toastSeq = 0

  function hydrateTheme() {
    const saved = localStorage.getItem('rag-theme') as Theme | null
    if (saved) theme.value = saved
    applyTheme()
    hydrateLocale()
  }

  function hydrateLocale() {
    const saved = localStorage.getItem(LOCALE_KEY) as Locale | null
    if (saved === 'en' || saved === 'ru') locale.value = saved
  }

  function setLocale(l: Locale) {
    locale.value = l
    localStorage.setItem(LOCALE_KEY, l)
  }

  function toggleLocale() {
    setLocale(locale.value === 'en' ? 'ru' : 'en')
  }

  /** Translate a shell/wiki UI string for the active locale. */
  function t(key: MessageKey, params?: Record<string, string | number>): string {
    return formatMessage(locale.value, key, params)
  }

  const localeLabel = computed(() => (locale.value === 'en' ? 'EN' : 'RU'))

  function setTheme(next: Theme) {
    theme.value = next
    localStorage.setItem('rag-theme', next)
    applyTheme()
  }

  function applyTheme() {
    const root = document.documentElement
    let mode = theme.value
    if (mode === 'system') {
      mode = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
    }
    root.dataset.theme = mode
  }

  function toggleSidebar() {
    sidebarCollapsed.value = !sidebarCollapsed.value
  }

  function openCommand() {
    commandOpen.value = true
  }

  function closeCommand() {
    commandOpen.value = false
  }

  async function checkHealth() {
    try {
      health.value = await api.health()
      healthError.value = null
    } catch (e) {
      healthError.value = e instanceof Error ? e.message : String(e)
      health.value = null
    }
  }

  function toast(text: string, kind: 'info' | 'error' | 'ok' = 'info') {
    const id = ++toastSeq
    toasts.value.push({ id, text, kind })
    setTimeout(() => {
      toasts.value = toasts.value.filter((x) => x.id !== id)
    }, 4200)
  }

  return {
    theme,
    locale,
    localeLabel,
    sidebarCollapsed,
    commandOpen,
    health,
    healthError,
    toasts,
    hydrateTheme,
    hydrateLocale,
    setLocale,
    toggleLocale,
    t,
    setTheme,
    applyTheme,
    toggleSidebar,
    openCommand,
    closeCommand,
    checkHealth,
    toast,
  }
})
