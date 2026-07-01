import { beforeEach, describe, expect, it } from 'vitest'
import { session } from './session'

describe('session.isTeamManager', () => {
  beforeEach(() => {
    session.clear()
  })

  it('is true for a global agent who is a team supervisor', () => {
    session.storeLogin('s1', {
      id: 'u1',
      role: 'agent',
      teams: [{ teamId: 1, roleInTeam: 'supervisor', isPrimary: true }],
    })
    expect(session.isTeamManager()).toBe(true)
  })

  it('is true for a global agent who is a team lead', () => {
    session.storeLogin('s1', {
      id: 'u1',
      role: 'agent',
      teams: [{ teamId: 1, roleInTeam: 'lead', isPrimary: true }],
    })
    expect(session.isTeamManager()).toBe(true)
  })

  it('is false for a global agent who is only a plain member', () => {
    session.storeLogin('s1', {
      id: 'u1',
      role: 'agent',
      teams: [{ teamId: 1, roleInTeam: 'member', isPrimary: true }],
    })
    expect(session.isTeamManager()).toBe(false)
  })

  it('is true for a global admin with no teams', () => {
    session.storeLogin('s1', { id: 'u1', role: 'admin', teams: [] })
    expect(session.isTeamManager()).toBe(true)
  })
})
