import { beforeEach, describe, expect, it, vi } from 'vitest'

describe('session team context', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.unstubAllGlobals()
    localStorage.clear()
  })

  it('sets the primary team on login and rejects unauthorized team switches for agents', async () => {
    const { session } = await import('../auth/session')

    session.storeLogin('session-1', {
      id: 'agent-1',
      role: 'agent',
      teams: [
        { id: 10, name: 'North', isPrimary: true },
        { teamId: 20, name: 'South' },
      ],
    })

    expect(session.contextTeamId()).toBe('10')
    expect(session.teamOptions()).toEqual([
      { id: '10', name: 'North', isPrimary: true },
      { id: '20', name: 'South', isPrimary: false },
    ])

    expect(session.switchContextTeam(20)).toBe(true)
    expect(session.contextTeamId()).toBe('20')

    expect(session.switchContextTeam(30)).toBe(false)
    expect(session.contextTeamId()).toBe('20')
  })

  it('lets administrators switch to an explicit team context even without a membership list', async () => {
    const { session } = await import('../auth/session')

    session.storeLogin('session-1', { id: 'admin-1', role: 'admin' })

    expect(session.switchContextTeam(99)).toBe(true)
    expect(session.contextTeamId()).toBe('99')
    expect(session.clearContextTeam()).toBe(true)
    expect(session.contextTeamId()).toBeNull()
  })

  it('sends the active team context header through the shared API client', async () => {
    const fetchMock = vi.fn(async () => new Response(JSON.stringify({ success: true, data: {} })))
    vi.stubGlobal('fetch', fetchMock)
    const { session } = await import('../auth/session')
    const { get } = await import('../api/client')

    session.storeLogin('session-1', {
      id: 'agent-1',
      role: 'agent',
      teams: [
        { id: 10, name: 'North', isPrimary: true },
        { id: 20, name: 'South' },
      ],
    })
    session.switchContextTeam(20)

    await get('/api/conversations')

    expect(fetchMock).toHaveBeenCalledWith(
      '/api/conversations',
      expect.objectContaining({
        headers: expect.objectContaining({ 'X-Context-Team-ID': '20' }),
      }),
    )
  })
})
