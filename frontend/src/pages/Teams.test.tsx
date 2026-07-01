import { render, screen, fireEvent, waitFor, within, cleanup } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const apiMock = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  del: vi.fn(),
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
    cleanup()
    vi.clearAllMocks()
    apiMock.get.mockImplementation((url: string) => {
      if (url === '/api/teams') {
        return Promise.resolve({
          success: true,
          data: [{ id: 1, name: '客服一隊', memberCount: 1 }],
        })
      }
      if (url === '/api/teams/1/members') {
        // Real backend returns a BARE ARRAY (team_member_list), not { members }.
        return Promise.resolve({
          success: true,
          data: [{ id: 'm1', displayName: '小明', role: 'member', isActive: true }],
        })
      }
      return Promise.resolve({ success: true, data: {} })
    })
    apiMock.post.mockResolvedValue({ success: true })
    apiMock.put.mockResolvedValue({ success: true })
    apiMock.del.mockResolvedValue({ success: true })
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

  it('removes a member FROM THE TEAM (leave), never purging the account', async () => {
    render(<Teams />)
    fireEvent.click(await screen.findByText('客服一隊'))
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams/1/members'))

    // Pick the member, click 移出團隊, confirm.
    fireEvent.click(await screen.findByRole('checkbox'))
    fireEvent.click(screen.getByRole('button', { name: /移出團隊/ }))
    fireEvent.click(await screen.findByRole('button', { name: '移出團隊' }))

    // Calls the team-scoped leave endpoint (removes team_members row, keeps account)...
    await waitFor(() =>
      expect(apiMock.del).toHaveBeenCalledWith('/api/teams/agent-teams/m1/leave/1'),
    )
    // ...and NEVER the account-purging bulk-delete endpoint.
    expect(apiMock.post).not.toHaveBeenCalledWith(
      '/api/teams/members/bulk-delete',
      expect.anything(),
    )
  })
})
