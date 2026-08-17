/**
 * Deterministic page icon (emoji) from slug hash.
 * Same slug always yields the same glyph; no server field required.
 */

/** Curated, high-contrast emoji set suitable as Notion-style page icons. */
const PAGE_ICONS: readonly string[] = [
  '📄',
  '📝',
  '📚',
  '📖',
  '🗂️',
  '📁',
  '🏷️',
  '📌',
  '🔖',
  '💡',
  '✨',
  '🔍',
  '🧭',
  '🗺️',
  '🧠',
  '🔗',
  '⚙️',
  '🛠️',
  '🧩',
  '📦',
  '🗄️',
  '📊',
  '📈',
  '🧪',
  '🔬',
  '🧬',
  '💻',
  '🖥️',
  '⌨️',
  '🌐',
  '☁️',
  '🔐',
  '🔑',
  '🛡️',
  '🚀',
  '⚡',
  '🔥',
  '🌟',
  '🎯',
  '🏁',
  '🏗️',
  '📐',
  '🎨',
  '🖼️',
  '📷',
  '🎬',
  '🎵',
  '🎮',
  '🏆',
  '💎',
  '🪙',
  '🪐',
  '🌙',
  '☀️',
  '🌊',
  '🌿',
  '🌸',
  '🍀',
  '🦊',
  '🐱',
  '🦉',
  '🐝',
  '🐢',
  '🦋',
]

/**
 * FNV-1a 32-bit hash (stable across JS engines; not cryptographic).
 */
export function hashSlug(input: string): number {
  let h = 0x811c9dc5
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return h >>> 0
}

/**
 * Pick a deterministic emoji for a wiki page from its slug (preferred)
 * or a fallback key such as id/title.
 */
export function pageIcon(slugOrKey: string | null | undefined): string {
  const key = (slugOrKey ?? '').trim()
  if (!key) return PAGE_ICONS[0]!
  const idx = hashSlug(key) % PAGE_ICONS.length
  return PAGE_ICONS[idx]!
}

export { PAGE_ICONS }
