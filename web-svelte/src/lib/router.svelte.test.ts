// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from 'vitest'
import { goGraph, goWiki, graphProjectParam, graphSeedParam, navigate, route, setGraphSeedQuery, startRouter } from './router.svelte'

const here = () => window.location.pathname + window.location.search

describe('router', () => {
  beforeEach(() => {
    window.history.replaceState({}, '', '/')
  })

  it('redirects the root to the console on start', () => {
    startRouter()
    expect(here()).toBe('/console')
    expect(route.name).toBe('console')
  })

  it('parses wiki page ids and encodes them on navigation', () => {
    goWiki('a b/c')
    expect(window.location.pathname).toBe('/wiki/a%20b%2Fc')
    expect(route.name).toBe('wiki')
    expect(route.pageId).toBe('a b/c')
    goWiki()
    expect(route.pageId).toBeNull()
  })

  it('redirects unknown paths to the console', () => {
    navigate('/nope')
    expect(here()).toBe('/console')
    expect(route.name).toBe('console')
  })

  it('exposes graph seed/project params only on the graph route', () => {
    goGraph('  seed-1 ', { project: 'rag' })
    expect(route.name).toBe('graph')
    expect(graphSeedParam()).toBe('seed-1')
    expect(graphProjectParam()).toBe('rag')
    goGraph(null)
    expect(graphSeedParam()).toBeNull()
    expect(graphProjectParam()).toBeNull()
    navigate('/search')
    expect(graphSeedParam()).toBeNull()
  })

  it('updates graph query in place with replace semantics', () => {
    goGraph('x')
    const length = window.history.length
    setGraphSeedQuery('y', '')
    expect(window.history.length).toBe(length)
    expect(route.query.get('seed')).toBe('y')
    expect(graphProjectParam()).toBe('')
    setGraphSeedQuery(null, null)
    expect(here()).toBe('/graph')
  })
})
