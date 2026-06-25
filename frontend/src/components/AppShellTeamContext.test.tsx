import { fireEvent, render, screen } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { MemoryRouter } from 'react-router-dom'

function stubMatchMedia() {
  vi.stubGlobal('matchMedia', (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    addListener: vi.fn(),
    removeListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }))
}

describe('AppShell team context switcher', () => {
  beforeEach(() => {
    vi.resetModules()
    vi.unstubAllGlobals()
    localStorage.clear()
    stubMatchMedia()
  })

  it('renders available teams and switches the active team context', async () => {
    const { session } = await import('../auth/session')
    const AppShell = (await import('./AppShell')).default

    session.storeLogin('session-1', {
      id: 'agent-1',
      role: 'agent',
      displayName: 'Agent One',
      teams: [
        { id: 10, name: 'North', isPrimary: true },
        { id: 20, name: 'South' },
      ],
    })

    render(
      <MemoryRouter>
        <AppShell title="Dashboard">
          <div>content</div>
        </AppShell>
      </MemoryRouter>,
    )

    const select = screen.getByLabelText('切換團隊') as HTMLSelectElement
    expect(select.value).toBe('10')

    fireEvent.change(select, { target: { value: '20' } })

    expect(session.contextTeamId()).toBe('20')
    expect(select.value).toBe('20')
  })
})
