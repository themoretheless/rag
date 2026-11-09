import type { AgentInfo, HealthResponse } from '@/api/types'
import type { Locale, MessageKey } from '@/lib/i18n'
import { Appearance, type Theme } from './appearance.svelte'
import { ShellHealth } from './health.svelte'
import { Toasts, type Toast, type ToastKind } from './toasts.svelte'

export type { Theme, Toast }

/**
 * Application-shell state, composed of three stores:
 *
 * - `appearance`  — theme and locale (`appearance.svelte.ts`)
 * - `toastStore`  — transient notifications (`toasts.svelte.ts`)
 * - `shellHealth` — gateway health badge and agent avatars (`health.svelte.ts`)
 *
 * plus the command-palette flag. The flat accessors keep the `ui.t(...)` /
 * `ui.toast(...)` call sites that every screen uses.
 */
class UiStore {
  readonly appearance = new Appearance()
  readonly toastStore = new Toasts()
  readonly shellHealth = new ShellHealth()

  sidebarCollapsed = $state(false)
  commandOpen = $state(false)

  toggleSidebar() {
    this.sidebarCollapsed = !this.sidebarCollapsed
  }

  openCommand() {
    this.commandOpen = true
  }

  closeCommand() {
    this.commandOpen = false
  }

  /* ---------- appearance ---------- */

  get theme(): Theme {
    return this.appearance.theme
  }
  get locale(): Locale {
    return this.appearance.locale
  }
  set locale(value: Locale) {
    this.appearance.locale = value
  }
  get localeLabel(): string {
    return this.appearance.localeLabel
  }
  /** Translate a shell/wiki UI string for the active locale. */
  t(key: MessageKey, params?: Record<string, string | number>): string {
    return this.appearance.t(key, params)
  }
  hydrateTheme() {
    this.appearance.hydrate()
  }
  hydrateLocale() {
    this.appearance.hydrateLocale()
  }
  setLocale(next: Locale) {
    this.appearance.setLocale(next)
  }
  toggleLocale() {
    this.appearance.toggleLocale()
  }
  setTheme(next: Theme) {
    this.appearance.setTheme(next)
  }

  /* ---------- toasts ---------- */

  get toasts(): Toast[] {
    return this.toastStore.items
  }
  toast(text: string, kind: ToastKind = 'info') {
    this.toastStore.push(text, kind)
  }

  /* ---------- gateway health ---------- */

  get health(): HealthResponse | null {
    return this.shellHealth.health
  }
  get healthError(): string | null {
    return this.shellHealth.healthError
  }
  get shellAgents(): AgentInfo[] {
    return this.shellHealth.agents
  }
  get shellAgentsError(): string | null {
    return this.shellHealth.agentsError
  }
  checkHealth(agentsMaxAgeMs = 0): Promise<void> {
    return this.shellHealth.check(agentsMaxAgeMs)
  }
  /** Check health now and keep polling; returns the stop function. */
  startHealthPolling(): () => void {
    return this.shellHealth.start()
  }
}

export const ui = new UiStore()
