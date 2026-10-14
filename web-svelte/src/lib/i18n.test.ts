import { afterEach, describe, expect, it } from 'vitest'
import { formatMessage, getActiveLocale, messages, setActiveLocale, t, type MessageKey } from './i18n'

describe('i18n catalog', () => {
  it('defines every key in both locales', () => {
    const en = Object.keys(messages.en).sort()
    expect(Object.keys(messages.ru).sort()).toEqual(en)
    for (const key of en as MessageKey[]) {
      expect(messages.en[key]).not.toBe('')
      expect(messages.ru[key]).not.toBe('')
    }
  })

  it('keeps placeholders consistent across locales', () => {
    const placeholders = (text: string) => (text.match(/\{\w+\}/g) ?? []).sort()
    for (const key of Object.keys(messages.en) as MessageKey[]) {
      expect(placeholders(messages.ru[key]), key).toEqual(placeholders(messages.en[key]))
    }
  })
})

describe('formatMessage', () => {
  it('substitutes every occurrence of each parameter', () => {
    expect(formatMessage('en', 'graphNodes', { count: 3 })).toBe('3 nodes')
    expect(formatMessage('ru', 'healthStats', { docs: 2, nodes: 5 })).toBe('2 док. · 5 узлов')
  })

  it('leaves unknown placeholders untouched when params are missing', () => {
    expect(formatMessage('en', 'graphEdges')).toBe('{count} edges')
  })
})

describe('active locale', () => {
  afterEach(() => setActiveLocale('ru'))

  it('defaults to ru and is switchable for non-component modules', () => {
    expect(getActiveLocale()).toBe('ru')
    expect(t('retry')).toBe('Повторить')
    setActiveLocale('en')
    expect(getActiveLocale()).toBe('en')
    expect(t('apiHttpError', { status: 500, detail: 'boom' })).toBe('HTTP 500: boom')
  })
})
