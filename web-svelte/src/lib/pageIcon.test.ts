import { describe, expect, it } from 'vitest'
import { PAGE_ICONS, hashSlug, pageIcon } from './pageIcon'

describe('hashSlug', () => {
  it('matches FNV-1a 32-bit reference values', () => {
    expect(hashSlug('')).toBe(0x811c9dc5)
    expect(hashSlug('a')).toBe(0xe40c292c)
    expect(hashSlug('foobar')).toBe(0xbf9cf968)
  })
})

describe('pageIcon', () => {
  it('is deterministic and drawn from the curated set', () => {
    expect(pageIcon('architecture')).toBe(pageIcon('architecture'))
    expect(PAGE_ICONS).toContain(pageIcon('architecture'))
    expect(pageIcon('architecture')).toBe(PAGE_ICONS[hashSlug('architecture') % PAGE_ICONS.length])
  })

  it('trims keys and uses the first icon for empty input', () => {
    expect(pageIcon('  slug  ')).toBe(pageIcon('slug'))
    expect(pageIcon('')).toBe(PAGE_ICONS[0])
    expect(pageIcon('   ')).toBe(PAGE_ICONS[0])
    expect(pageIcon(null)).toBe(PAGE_ICONS[0])
    expect(pageIcon(undefined)).toBe(PAGE_ICONS[0])
  })
})
