import { beforeEach, describe, expect, it, vi } from 'vitest'

function jsonResponse(body: unknown, init?: ResponseInit) {
  return new Response(JSON.stringify(body), {
    headers: { 'Content-Type': 'application/json' },
    ...init,
  })
}

describe('api client auth and csrf behavior', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.unstubAllGlobals()
    vi.useRealTimers()
    localStorage.clear()
    document.cookie = 'mcss_csrf=; Max-Age=0; path=/'
    Object.defineProperty(window, 'location', {
      configurable: true,
      value: { assign: vi.fn() },
    })
  })

  it('adds the csrf header only for mutations', async () => {
    document.cookie = 'mcss_csrf=csrf-123; path=/'
    const fetchMock = vi.fn(async () => jsonResponse({ success: true, data: {} }))
    vi.stubGlobal('fetch', fetchMock)
    const { get, post } = await import('../api/client')

    await get('/api/conversations')
    await post('/api/conversations', { subject: 'hello' })

    expect(fetchMock).toHaveBeenNthCalledWith(
      1,
      '/api/conversations',
      expect.objectContaining({
        method: 'GET',
        headers: expect.not.objectContaining({ 'X-CSRF-Token': expect.any(String) }),
      }),
    )
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      '/api/conversations',
      expect.objectContaining({
        method: 'POST',
        headers: expect.objectContaining({ 'X-CSRF-Token': 'csrf-123' }),
        body: JSON.stringify({ subject: 'hello' }),
      }),
    )
  })

  it('refreshes credentials once and retries a 401 request without redirecting', async () => {
    document.cookie = 'mcss_csrf=csrf-456; path=/'
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ success: false, error: 'expired' }, { status: 401 }))
      .mockResolvedValueOnce(jsonResponse({ success: true }))
      .mockResolvedValueOnce(jsonResponse({ success: true, data: { id: 'conv-1' } }))
    vi.stubGlobal('fetch', fetchMock)
    const { get } = await import('../api/client')

    const result = await get<{ id: string }>('/api/conversations/conv-1')

    expect(result).toEqual({ success: true, data: { id: 'conv-1' } })
    expect(fetchMock).toHaveBeenCalledTimes(3)
    expect(fetchMock).toHaveBeenNthCalledWith(
      2,
      '/api/auth/refresh',
      expect.objectContaining({
        method: 'POST',
        credentials: 'include',
        headers: expect.objectContaining({ 'X-CSRF-Token': 'csrf-456' }),
        body: '{}',
      }),
    )
    expect(fetchMock).toHaveBeenNthCalledWith(
      3,
      '/api/conversations/conv-1',
      expect.objectContaining({ method: 'GET', credentials: 'include' }),
    )
    expect(window.location.assign).not.toHaveBeenCalled()
  })

  it('redirects once when refresh fails after a 401', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce(jsonResponse({ success: false }, { status: 401 }))
      .mockResolvedValueOnce(jsonResponse({ success: false }, { status: 401 }))
    vi.stubGlobal('fetch', fetchMock)
    const { get } = await import('../api/client')

    const result = await get('/api/private')

    expect(result.success).toBe(false)
    expect(result.status).toBe(401)
    expect(window.location.assign).toHaveBeenCalledTimes(1)
    expect(window.location.assign).toHaveBeenCalledWith('/login')
  })
})
