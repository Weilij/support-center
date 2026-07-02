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

const sessionMock = vi.hoisted(() => ({
  position: vi.fn(() => 'system_admin'),
  isAdmin: vi.fn(() => true),
  isTeamManager: vi.fn(() => true),
  identity: vi.fn(() => ({ id: 'admin1' }) as { id: string } | null),
}))
vi.mock('../auth/session', () => ({ session: sessionMock }))

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
    sessionMock.position.mockReturnValue('system_admin')
    sessionMock.isAdmin.mockReturnValue(true)
    sessionMock.isTeamManager.mockReturnValue(true)
    sessionMock.identity.mockReturnValue({ id: 'admin1' })
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
          data: [{ id: 'm1', displayName: '小明', role: 'agent', roleInTeam: 'member', isActive: true }],
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

    // Must hit the TEAM-scoped role endpoint (member/lead/supervisor), NOT the
    // global-role endpoint (which rejects with "role must be one of: admin, agent").
    await waitFor(() =>
      expect(apiMock.put).toHaveBeenCalledWith('/api/teams/agent-teams/m1/role/1', {
        roleInTeam: 'supervisor',
      }),
    )
    expect(apiMock.put).not.toHaveBeenCalledWith(
      '/api/teams/members/m1/role',
      expect.anything(),
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

  it('deletes the whole team via DELETE /api/teams/{id}', async () => {
    render(<Teams />)
    fireEvent.click(await screen.findByText('客服一隊'))
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams/1/members'))

    fireEvent.click(screen.getByRole('button', { name: '刪除團隊…' })) // opens confirm
    fireEvent.click(await screen.findByRole('button', { name: '刪除團隊' })) // confirm

    await waitFor(() => expect(apiMock.del).toHaveBeenCalledWith('/api/teams/1'))
  })

  it('hides admin-only controls for a non-admin team manager', async () => {
    sessionMock.position.mockReturnValue('agent')
    sessionMock.isAdmin.mockReturnValue(false)
    sessionMock.identity.mockReturnValue({ id: 'sup1' })
    // The current user is a supervisor OF THE OPEN TEAM (from the members list) → canModify true.
    apiMock.get.mockImplementation((url: string) => {
      if (url === '/api/teams') {
        return Promise.resolve({ success: true, data: [{ id: 1, name: '客服一隊', memberCount: 1 }] })
      }
      if (url === '/api/teams/1/members') {
        return Promise.resolve({
          success: true,
          data: [{ id: 'sup1', displayName: '主管本人', roleInTeam: 'supervisor', isActive: true }],
        })
      }
      return Promise.resolve({ success: true, data: {} })
    })

    render(<Teams />)
    fireEvent.click(await screen.findByText('客服一隊'))
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams/1/members'))

    // Manager controls remain: add-member picker + editable role dropdown.
    expect(await screen.findByLabelText('新增成員')).toBeTruthy()
    expect(screen.getByDisplayValue('主管（團隊管理員）')).toBeTruthy() // role select is editable

    // Admin-only controls are gone.
    expect(screen.queryByRole('button', { name: '建立' })).toBeNull()
    expect(screen.queryByRole('button', { name: '刪除團隊…' })).toBeNull()
    // Status is a read-only pill, not a toggle button.
    expect(screen.queryByRole('button', { name: /啟用|停用/ })).toBeNull()
  })

  it('is read-only for a plain member (no modify controls)', async () => {
    sessionMock.position.mockReturnValue('agent')
    sessionMock.isAdmin.mockReturnValue(false)
    // current user is the plain member m1 of the open team → canModify false
    sessionMock.identity.mockReturnValue({ id: 'm1' })

    render(<Teams />)
    fireEvent.click(await screen.findByText('客服一隊'))
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/teams/1/members'))
    expect(await screen.findByText('小明')).toBeTruthy() // the member row still renders (read access)

    // Read-only: no editable role dropdown / add-member select (no combobox at all),
    // no selection checkbox, and none of the modify buttons.
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.queryByLabelText('新增成員')).toBeNull()
    expect(screen.queryByRole('checkbox')).toBeNull()
    expect(screen.queryByRole('button', { name: '建立' })).toBeNull()
    expect(screen.queryByRole('button', { name: '刪除團隊…' })).toBeNull()
    expect(screen.queryByRole('button', { name: /啟用|停用/ })).toBeNull()
  })
})
