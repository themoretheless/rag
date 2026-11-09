import { formatMessage, setActiveLocale, type Locale, type MessageKey } from '@/lib/i18n'

export type Theme = 'dark' | 'light' | 'system'

const THEME_KEY = 'rag-theme'
const LOCALE_KEY = 'rag-locale'

/** Colour theme and interface language, persisted in localStorage. */
export class Appearance {
  theme = $state<Theme>('dark')
  locale = $state<Locale>('ru')

  get localeLabel(): string {
    return this.locale === 'en' ? 'EN' : 'RU'
  }

  /** Translate a shell/wiki UI string for the active locale. */
  t(key: MessageKey, params?: Record<string, string | number>): string {
    return formatMessage(this.locale, key, params)
  }

  /** Restore the saved theme and locale and apply them to the document. */
  hydrate() {
    const saved = localStorage.getItem(THEME_KEY) as Theme | null
    if (saved) this.theme = saved
    this.applyTheme()
    this.hydrateLocale()
  }

  hydrateLocale() {
    const saved = localStorage.getItem(LOCALE_KEY)
    if (saved === 'en' || saved === 'ru') this.locale = saved
    setActiveLocale(this.locale)
  }

  setLocale(next: Locale) {
    this.locale = next
    setActiveLocale(next)
    localStorage.setItem(LOCALE_KEY, next)
  }

  toggleLocale() {
    this.setLocale(this.locale === 'en' ? 'ru' : 'en')
  }

  setTheme(next: Theme) {
    this.theme = next
    localStorage.setItem(THEME_KEY, next)
    this.applyTheme()
  }

  /** Write the effective mode (`system` resolved through the OS preference) to `<html data-theme>`. */
  applyTheme() {
    const mode =
      this.theme === 'system'
        ? window.matchMedia('(prefers-color-scheme: dark)').matches
          ? 'dark'
          : 'light'
        : this.theme
    document.documentElement.dataset.theme = mode
  }
}
