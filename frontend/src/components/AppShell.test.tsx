import { cleanup, render, screen } from '@testing-library/react'
import { MemoryRouter } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// Use the REAL permissions (can) so ops-area gating is realistic; only stub the
// session's isTeamManager/position per test. Stub stores/teams so importing AppShell
// has no side effects.
const sessionMock = vi.hoisted(() => ({
  position: vi.fn(() => 'agent'),
  isTeamManager: vi.fn(() => false),
  identity: vi.fn(() => null),
}))
// authChanged is used at module-load time by ../realtime/client (transitively
// imported via notificationsStore); provide a no-op emitter so the import side
// effect doesn't throw.
vi.mock('../auth/session', () => ({
  session: sessionMock,
  authChanged: { on: vi.fn() },
}))
// NavGroups never reads teamsStore (only the AppShell default component does), so a
// stub that just satisfies the module import is enough.
vi.mock('../stores/teams', () => ({ loadTeams: vi.fn(), teamsStore: {} }))

import { NavGroups } from './AppShell'

function renderNav() {
  return render(
    <MemoryRouter>
      <NavGroups pathname="/" pos="agent" unread={0} />
    </MemoryRouter>,
  )
}

describe('團隊 nav visibility', () => {
  beforeEach(() => vi.clearAllMocks())
  // No global test setup file, so RTL's auto-cleanup isn't registered; unmount
  // between tests ourselves to avoid leaking DOM from a prior render.
  afterEach(() => cleanup())

  it('shows 團隊 for a global agent who is an in-team manager', () => {
    sessionMock.isTeamManager.mockReturnValue(true)
    renderNav()
    expect(screen.getByText('團隊')).toBeTruthy()
  })

  it('hides 團隊 for a plain member (agent, not a manager)', () => {
    sessionMock.isTeamManager.mockReturnValue(false)
    renderNav()
    expect(screen.queryByText('團隊')).toBeNull()
  })
})
