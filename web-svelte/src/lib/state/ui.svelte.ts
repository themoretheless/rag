import { api } from '@/api/client'
import type { HealthResponse } from '@/api/types'
import { formatMessage, type Locale, type MessageKey } from '@/lib/i18n'

export type Theme = 'dark' | 'light' | 'system'

const THEME_KEY = 'rag-theme'
const LOCALE_KEY = 'rag-locale'

export interface Toast {
  id: number
  text: string
  kind: 'info' | 'error' | 'ok'
}

/** Shell state: theme, locale, health, toasts, sidebar, command palette. */
class UiStore {
  theme = $state<Theme>('dark')
  locale = $state<Locale>('en')
  sidebarCollapsed = $state(false)
  commandOpen = $state(false)
  health = $state<HealthResponse | null>(null)
  healthError = $state<string | null>(null)
  toasts = $state<Toast[]>([])

  private toastSeq = 0

  get localeLabel(): string {
    return this.locale === 'en' ? 'EN' : 'RU'
  }

  /** Translate a shell/wiki UI string for the active locale. */
  t(key: MessageKey, params?: Record<string, string | number>): string {
    return formatMessage(this.locale, key, params)
  }

  hydrateTheme() {
    const saved = localStorage.getItem(THEME_KEY) as Theme | null
    if (saved) this.theme = saved
    this.applyTheme()
    this.hydrateLocale()
  }

  hydrateLocale() {
    const saved = localStorage.getItem(LOCALE_KEY) as Locale | null
    if (saved === 'en' || saved === 'ru') this.locale = saved
  }

  setLocale(l: Locale) {
    this.locale = l
    localStorage.setItem(LOCALE_KEY, l)
  }

  toggleLocale() {
    this.setLocale(this.locale === 'en' ? 'ru' : 'en')
  }

  setTheme(next: Theme) {
    this.theme = next
    localStorage.setItem(THEME_KEY, next)
    this.applyTheme()
  }

  applyTheme() {
    const root = document.documentElement
    let mode = this.theme
    if (mode === 'system') {
      mode = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
    }
    root.dataset.theme = mode
  }

  toggleSidebar() {
    this.sidebarCollapsed = !this.sidebarCollapsed
  }

  openCommand() {
    this.commandOpen = true
  }

  closeCommand() {
    this.commandOpen = false
  }

  async checkHealth() {
    try {
      this.health = await api.health()
      this.healthError = null
    } catch (e) {
      this.healthError = e instanceof Error ? e.message : String(e)
      this.health = null
    }
  }

  toast(text: string, kind: 'info' | 'error' | 'ok' = 'info') {
    const id = ++this.toastSeq
    this.toasts.push({ id, text, kind })
    setTimeout(() => {
      this.toasts = this.toasts.filter((x) => x.id !== id)
    }, 4200)
  }
}

export const ui = new UiStore()
