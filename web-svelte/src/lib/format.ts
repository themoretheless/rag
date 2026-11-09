/**
 * Display formatters shared by the dashboards.
 *
 * Several screens format the "same" quantity with deliberately different precision
 * (a console tile vs. a live resource meter), so those variants stay separate
 * functions here instead of being silently unified.
 */

const DASH = '—'

/** `HH:MM` (24h, local time) of an ISO timestamp; a dash when absent. */
export function time(value?: string | null): string {
  if (!value) return DASH
  return new Date(value).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false })
}

/** `YYYY-MM-DD HH:MM` cut straight from an ISO timestamp (no timezone shift). */
export function shortDate(value: string): string {
  if (!value) return DASH
  return value.replace('T', ' ').slice(0, 16)
}

/** "N мин/ч/дн назад" for the last week, the ISO date after that. */
export function relativeDate(value: string, now: number = Date.now()): string {
  const timestamp = Date.parse(value)
  if (!Number.isFinite(timestamp)) return shortDate(value)
  const minutes = Math.max(0, Math.floor((now - timestamp) / 60_000))
  if (minutes < 60) return `${minutes} мин назад`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} ч назад`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days} дн назад`
  return value.slice(0, 10)
}

/** Middle-elided content hash (`abcdef012345…fedcba`); short values pass through. */
export function shortHash(value?: string | null): string {
  if (!value) return DASH
  return value.length > 24 ? `${value.slice(0, 12)}…${value.slice(-6)}` : value
}

/** Two-letter avatar initials from an agent / client name. */
export function initials(name: string): string {
  const [first, second] = name
    .trim()
    .split(/[\s_-]+/)
    .filter(Boolean)
  if (first === undefined) return '?'
  return (second === undefined ? first.slice(0, 2) : `${first.charAt(0)}${second.charAt(0)}`).toUpperCase()
}

/** Retrieval score: 2 decimals from 1 up, 4 below; a dash when absent. */
export function score(value?: number | null): string {
  return value == null ? DASH : value.toFixed(value >= 1 ? 2 : 4)
}

/** Whole milliseconds with unit; a dash when absent. */
export function milliseconds(value?: number | null): string {
  return value == null ? DASH : `${Math.round(value)} мс`
}

/** Live-meter duration: seconds with one decimal from 1000 ms, raw ms below. */
export function fmtMs(ms: number): string {
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)} с`
  return `${ms} мс`
}

const BYTE_UNITS = ['Б', 'КБ', 'МБ', 'ГБ', 'ТБ'] as const

/** Byte size stepped through Б/КБ/МБ/ГБ; one decimal from МБ up. */
export function bytes(value?: number | null): string {
  if (value == null) return DASH
  let size = value
  let unit = 0
  while (size >= 1024 && unit < 3) {
    size /= 1024
    unit += 1
  }
  return `${size.toFixed(unit > 1 ? 1 : 0)} ${BYTE_UNITS[unit] ?? ''}`
}

/** Coarse storage size for overview tiles: whole МБ, or ГБ with one decimal. */
export function megabytes(value?: number | null): string {
  if (value == null) return DASH
  return value > 1024 ** 3 ? `${(value / 1024 ** 3).toFixed(1)} ГБ` : `${Math.round(value / 1024 ** 2)} МБ`
}

/** Live-meter byte size with three significant digits (Б…ТБ); `0` for zero. */
export function fmtBytes(value: number): string {
  if (!value) return '0'
  const unit = Math.min(BYTE_UNITS.length - 1, Math.floor(Math.log2(value) / 10))
  const scaled = value / 2 ** (10 * unit)
  const digits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2
  return `${scaled.toFixed(digits)} ${BYTE_UNITS[unit] ?? ''}`
}
