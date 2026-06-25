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
})
