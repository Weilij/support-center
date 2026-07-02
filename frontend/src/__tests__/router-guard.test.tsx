import { render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const sessionMock = vi.hoisted(() => ({
  snapshot: vi.fn(),
  lifecycle: vi.fn(),
  init: vi.fn(),
  recordSnapshot: vi.fn(),
  position: vi.fn(),
}))

vi.mock('../auth/session', () => ({
  session: sessionMock,
  authChanged: {
    on: vi.fn(),
  },
}))

vi.mock('../i18n', () => ({
  t: (key: string) => key,
}))

import { Guard } from '../router'

describe('router auth guard', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    sessionMock.position.mockReturnValue('agent')
  })

  it('fails closed for authenticated routes when guard evaluation throws', async () => {
    sessionMock.snapshot.mockImplementation(() => {
      throw new Error('storage unavailable')
    })

    render(
      <MemoryRouter initialEntries={['/protected']}>
        <Routes>
          <Route
            path="/protected"
            element={
              <Guard meta={{ requiresAuth: true, title: 'Protected' }}>
                <div>protected page</div>
              </Guard>
            }
          />
          <Route path="/login" element={<div>login page</div>} />
        </Routes>
      </MemoryRouter>,
    )

    await waitFor(() => expect(screen.getByText('login page')).toBeTruthy())
    expect(screen.queryByText('protected page')).toBeNull()
  })

  it('lets any authenticated position read a daily-area route (e.g. /teams)', async () => {
    sessionMock.snapshot.mockReturnValue(true)
    sessionMock.position.mockReturnValue('agent')

    render(
      <MemoryRouter initialEntries={['/teams']}>
        <Routes>
          <Route
            path="/teams"
            element={
              <Guard meta={{ requiresAuth: true, area: 'daily', title: 'Teams' }}>
                <div>teams page</div>
              </Guard>
            }
          />
          <Route path="/dashboard" element={<div>dashboard page</div>} />
        </Routes>
      </MemoryRouter>,
    )

    await waitFor(() => expect(screen.getByText('teams page')).toBeTruthy())
    expect(screen.queryByText('dashboard page')).toBeNull()
  })

  it('still redirects a plain agent away from an ops-area route to the dashboard', async () => {
    sessionMock.snapshot.mockReturnValue(true)
    sessionMock.position.mockReturnValue('agent')

    render(
      <MemoryRouter initialEntries={['/ops-only']}>
        <Routes>
          <Route
            path="/ops-only"
            element={
              <Guard meta={{ requiresAuth: true, area: 'ops', title: 'Ops' }}>
                <div>ops page</div>
              </Guard>
            }
          />
          <Route path="/dashboard" element={<div>dashboard page</div>} />
        </Routes>
      </MemoryRouter>,
    )

    await waitFor(() => expect(screen.getByText('dashboard page')).toBeTruthy())
    expect(screen.queryByText('ops page')).toBeNull()
  })
})
