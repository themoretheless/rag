import { describe, expect, it } from 'vitest'
import {
  bytes,
  fmtBytes,
  fmtMs,
  initials,
  megabytes,
  milliseconds,
  relativeDate,
  score,
  shortDate,
  shortHash,
  time,
} from './format'

describe('time / dates', () => {
  it('renders a dash for missing timestamps and HH:MM otherwise', () => {
    expect(time()).toBe('—')
    expect(time(null)).toBe('—')
    expect(time('2026-01-02T03:04:05')).toMatch(/^\d{2}:\d{2}$/)
  })

  it('cuts shortDate straight from the ISO string', () => {
    expect(shortDate('2026-01-02T03:04:05Z')).toBe('2026-01-02 03:04')
    expect(shortDate('')).toBe('—')
  })

  it('steps relativeDate through minutes, hours, days and falls back to the date', () => {
    const now = Date.parse('2026-01-10T12:00:00Z')
    expect(relativeDate('2026-01-10T11:58:30Z', now)).toBe('1 мин назад')
    expect(relativeDate('2026-01-10T09:00:00Z', now)).toBe('3 ч назад')
    expect(relativeDate('2026-01-07T12:00:00Z', now)).toBe('3 дн назад')
    expect(relativeDate('2026-01-01T12:00:00Z', now)).toBe('2026-01-01')
    expect(relativeDate('2026-01-11T12:00:00Z', now)).toBe('0 мин назад')
    expect(relativeDate('not a date', now)).toBe('not a date')
  })
})

describe('identifiers', () => {
  it('elides long hashes in the middle only', () => {
    expect(shortHash(null)).toBe('—')
    expect(shortHash('abc')).toBe('abc')
    expect(shortHash('0123456789abcdef0123456789abcdef')).toBe('0123456789ab…abcdef')
  })

  it('builds initials from one or several name parts', () => {
    expect(initials('claude')).toBe('CL')
    expect(initials('claude-code')).toBe('CC')
    expect(initials('my agent_three')).toBe('MA')
    expect(initials('  ')).toBe('?')
  })
})

describe('numbers', () => {
  it('formats scores with precision that depends on magnitude', () => {
    expect(score()).toBe('—')
    expect(score(0.01639)).toBe('0.0164')
    expect(score(12.3456)).toBe('12.35')
  })

  it('formats milliseconds for both absent conventions', () => {
    expect(milliseconds()).toBe('—')
    expect(milliseconds(null)).toBe('—')
    expect(milliseconds(12.6)).toBe('13 мс')
    expect(fmtMs(999)).toBe('999 мс')
    expect(fmtMs(1500)).toBe('1.5 с')
  })

  it('keeps the three byte formats distinct', () => {
    expect(bytes()).toBe('—')
    expect(bytes(512)).toBe('512 Б')
    expect(bytes(2048)).toBe('2 КБ')
    expect(bytes(5 * 1024 ** 2)).toBe('5.0 МБ')
    expect(bytes(3 * 1024 ** 4)).toBe('3072.0 ГБ')

    expect(megabytes()).toBe('—')
    expect(megabytes(5 * 1024 ** 2)).toBe('5 МБ')
    expect(megabytes(1.5 * 1024 ** 3)).toBe('1.5 ГБ')

    expect(fmtBytes(0)).toBe('0')
    expect(fmtBytes(1536)).toBe('1.50 КБ')
    expect(fmtBytes(15 * 1024 ** 2)).toBe('15.0 МБ')
    expect(fmtBytes(150 * 1024 ** 3)).toBe('150 ГБ')
  })
})
