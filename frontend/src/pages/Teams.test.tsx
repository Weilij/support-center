import { render, screen, fireEvent, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const apiMock = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
}))

vi.mock('../api/client', () => apiMock)

vi.mock('../auth/permissions', () => ({
  can: () => true,
}))

vi.mock('../auth/session', () => ({
  session: { position: () => 'system_admin' },
}))

vi.mock('../stores/agents', () => ({
  loadAgents: vi.fn().mockResolvedValue({
    items: [
      { id: 'm1', displayName: 'Alice' },
      { id: 'a2', displayName: 'Bob', email: 'bob@x.com' },
    ],
    total: 2,
    page: 1,
  }),
}))

import Teams from './Teams'

describe('Teams member role select', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiMock.get.mockImplementation((url: string) => {
      if (url === '/api/teams') {
        return Promise.resolve({
          success: true,
          data: [{ id: 1, name: '客服一隊', memberCount: 1 }],
        })
      }
      if (url === '/api/teams/1/members') {
        return Promise.resolve({
          success: true,
          data: { members: [{ id: 'm1', displayName: '小明', role: 'member', isActive: true }] },
        })
      }
      return Promise.resolve({ success: true, data: {} })
    })
    apiMock.post.mockResolvedValue({ success: true })
    apiMock.put.mockResolvedValue({ success: true })
  })

  it('uses real team roles and updates a member to supervisor', async () => {
    render(<Teams />)

    // Initial team list load.
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams'))

    // Select the team to load its members.
    fireEvent.click(screen.getByText('客服一隊'))
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams/1/members'))

    // The role select renders the real team-role labels.
    const select = (await screen.findByDisplayValue('成員')) as HTMLSelectElement
    const labels = within(select).getAllByRole('option').map((o) => o.textContent)
    expect(labels).toEqual(['成員', '組長', '主管（團隊管理員）'])

    // Selecting 主管 (supervisor) calls put with the supervisor role.
    fireEvent.change(select, { target: { value: 'supervisor' } })

    await waitFor(() =>
      expect(apiMock.put).toHaveBeenCalledWith('/api/teams/members/m1/role', { role: 'supervisor' }),
    )
  })

  it('adds an existing agent to the team', async () => {
    const { findByText, findByLabelText, getByRole } = render(<Teams />)
    fireEvent.click(await findByText('客服一隊'))
    const picker = (await findByLabelText('新增成員')) as HTMLSelectElement
    expect(within(picker).queryByText('Alice')).toBeNull() // m1 already a member
    expect(within(picker).getByText(/Bob/)).toBeTruthy() // a2 is a candidate
    fireEvent.change(picker, { target: { value: 'a2' } })
    fireEvent.click(getByRole('button', { name: '加入團隊' }))
    await waitFor(() =>
      expect(apiMock.post).toHaveBeenCalledWith('/api/teams/1/members', { agentId: 'a2' }),
    )
  })
})
