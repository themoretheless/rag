import { describe, expect, it } from 'vitest'
import { DEFAULT_ROUTE, ROUTES, matchRoute, navRoutes, routeByName, routeCrumb } from './routes'

describe('route table', () => {
  it('has unique names and paths, and paths are /<name>', () => {
    const names = ROUTES.map((entry) => entry.name)
    expect(new Set(names).size).toBe(names.length)
    for (const entry of ROUTES) expect(entry.path).toBe(`/${entry.name}`)
  })

  it('matches exact paths and only parameterised routes accept a trailing segment', () => {
    expect(matchRoute('/graph')).toEqual({ entry: routeByName('graph'), param: null })
    expect(matchRoute('/graph/x')).toBeNull()
    expect(matchRoute('/wiki')).toEqual({ entry: routeByName('wiki'), param: null })
    expect(matchRoute('/wiki/')).toEqual({ entry: routeByName('wiki'), param: null })
    expect(matchRoute('/wiki/a%20b%2Fc')).toEqual({ entry: routeByName('wiki'), param: 'a b/c' })
    expect(matchRoute('/wikipedia')).toBeNull()
    expect(matchRoute('/')).toBeNull()
    expect(matchRoute('/nope')).toBeNull()
  })

  it('lists the rail in table order with models pinned to the foot', () => {
    expect(navRoutes('main').map((entry) => entry.name)).toEqual([
      'console',
      'corpus',
      'search',
      'graph',
      'wiki',
      'agents',
      'sync',
      'evaluation',
      'resources',
    ])
    expect(navRoutes('bottom').map((entry) => entry.name)).toEqual(['models'])
  })

  it('falls back to the default breadcrumb for routes without one', () => {
    expect(DEFAULT_ROUTE.name).toBe('console')
    expect(routeCrumb('wiki').title).toBe('Вики')
    expect(routeCrumb('resources')).toEqual(routeCrumb('console'))
  })
})
