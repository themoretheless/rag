// @vitest-environment jsdom

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { api, hasGatewayToken, setGatewayToken } from './client'

const secret = 'test-access-token'

describe('gateway Bearer authentication', () => {
  beforeEach(() => {
    sessionStorage.clear()
    // Node 26 exposes its own unavailable localStorage over jsdom's property.
    vi.stubGlobal('localStorage', { setItem: vi.fn(), getItem: vi.fn(), removeItem: vi.fn() })
    vi.stubEnv('VITE_API_BASE', '')
    vi.stubGlobal('fetch', vi.fn(async () => new Response('{}', {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })))
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.unstubAllEnvs()
  })

  it('allows an unauthenticated loopback server without a token', async () => {
    await api.health()
    const [, init] = vi.mocked(fetch).mock.calls[0]
    expect(new Headers(init?.headers).has('Authorization')).toBe(false)
    expect(hasGatewayToken()).toBe(false)
  })

  it('sends create-only wiki writes via POST while updates continue using PUT', async () => {
    const body = { slug: 'new-page', title: 'New page', content: '# New page' }
    await api.createWiki(body)
    await api.putWiki({ ...body, if_match_revision: 1 })
    const calls = vi.mocked(fetch).mock.calls
    expect(calls[0][0]).toBe('/v1/wiki')
    expect(calls[0][1]?.method).toBe('POST')
    expect(calls[1][1]?.method).toBe('PUT')
  })

  it('authenticates reads, writes, and source downloads without putting the token in URLs', async () => {
    setGatewayToken(secret)
    await api.health()
    await api.get('/v1/status')
    await api.post('/v1/search', { query: 'example' })
    await api.sourceFile('document with spaces')

    expect(fetch).toHaveBeenCalledTimes(4)
    for (const [url, init] of vi.mocked(fetch).mock.calls) {
      const headers = new Headers(init?.headers)
      expect(headers.get('Authorization')).toBe(`Bearer ${secret}`)
      expect(String(url)).not.toContain(secret)
      expect(init?.redirect).toBe('error')
    }
    const [, post] = vi.mocked(fetch).mock.calls[2]
    expect(new Headers(post?.headers).get('Content-Type')).toBe('application/json')
    expect(post?.body).toBe('{"query":"example"}')
    expect(vi.mocked(fetch).mock.calls[3][0]).toBe('/v1/source-file?document_id=document%20with%20spaces')
  })

  it('stores the token only in tab session storage and restores it after module reload', async () => {
    setGatewayToken(secret)
    expect(localStorage.setItem).not.toHaveBeenCalled()
    expect(sessionStorage.length).toBe(1)
    vi.resetModules()
    const reloaded = await import('./client')
    expect(reloaded.hasGatewayToken()).toBe(true)
    await reloaded.api.health()
    expect(new Headers(vi.mocked(fetch).mock.calls[0][1]?.headers).get('Authorization')).toBe(`Bearer ${secret}`)
  })

  it('clears credentials and stops sending them on subsequent requests', async () => {
    setGatewayToken(secret)
    setGatewayToken('')
    expect(sessionStorage.length).toBe(0)
    expect(hasGatewayToken()).toBe(false)
    await api.health()
    expect(new Headers(vi.mocked(fetch).mock.calls[0][1]?.headers).has('Authorization')).toBe(false)
  })

  it('does not reuse a token when the configured API base changes', async () => {
    setGatewayToken(secret)
    vi.stubEnv('VITE_API_BASE', 'https://another-gateway.example')
    await api.health()
    expect(hasGatewayToken()).toBe(false)
    expect(new Headers(vi.mocked(fetch).mock.calls[0][1]?.headers).has('Authorization')).toBe(false)
  })

  it('rejects malformed credentials without exposing or saving them', () => {
    const invalid = 'private-token\r\nInjected: value'
    expect(() => setGatewayToken(invalid)).toThrow('допустимые символы')
    try { setGatewayToken(invalid) } catch (cause) { expect(String(cause)).not.toContain(invalid) }
    expect(sessionStorage.length).toBe(0)
  })

  it('rejects external or protocol-relative request paths before sending credentials', async () => {
    setGatewayToken(secret)
    for (const path of ['https://external.example/data', '//external.example/data', '/\\external.example/data', '/\t/external.example/data']) {
      await expect(api.get(path)).rejects.toThrow('absolute API path')
    }
    expect(fetch).not.toHaveBeenCalled()
  })

  it.each([401, 403])('reports HTTP %i without echoing an authentication response', async (status) => {
    setGatewayToken(secret)
    vi.mocked(fetch).mockResolvedValueOnce(new Response(`Rejected ${secret}`, { status }))
    const error = await api.health().catch((cause: Error) => cause)
    expect(String(error)).toContain(`HTTP ${status}`)
    expect(String(error)).not.toContain(secret)
  })
})
