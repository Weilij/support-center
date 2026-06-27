import { beforeEach, describe, expect, it, vi } from 'vitest'

import { del, get, post, put } from '../api/client'
import {
  batchTransferAgents,
  createAgent,
  deleteAgent,
  loadAgents,
  loadStatusStatistics,
  setAgentPosition,
} from '../stores/agents'
import {
  closeSession,
  loadSessionStats,
  loadSessions,
  reopenSession,
  updateSessionTopic,
} from '../stores/sessions'

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>()
  return {
    ...actual,
    get: vi.fn(),
    post: vi.fn(),
    put: vi.fn(),
    del: vi.fn(),
  }
})

const getMock = vi.mocked(get)
const postMock = vi.mocked(post)
const putMock = vi.mocked(put)
const delMock = vi.mocked(del)

describe('agents store API wrappers', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    putMock.mockReset()
    delMock.mockReset()
  })

  it('loads paginated agents through shared list unwrapping', async () => {
    getMock.mockResolvedValue({
      success: true,
      data: { items: [{ id: 'agent-1', displayName: 'Ada' }, { id: 42, displayName: 'bad' }] },
      pagination: { total: 7, page: 2 },
    } as never)

    const page = await loadAgents(2, 10)

    expect(getMock).toHaveBeenCalledWith('/api/agents?page=2&limit=10')
    expect(page).toEqual({
      items: [
        {
          id: 'agent-1',
          email: undefined,
          displayName: 'Ada',
          role: undefined,
          position: undefined,
          isActive: undefined,
          teamId: undefined,
          teamName: undefined,
          lastActiveAt: undefined,
          lastLoginAt: undefined,
          createdAt: undefined,
        },
      ],
      total: 7,
      page: 2,
    })
  })

  it('maps agent commands to their backend endpoints', async () => {
    getMock.mockResolvedValue({ success: true, data: { online: 2, offline: 1 } } as never)
    putMock.mockResolvedValue({ success: true, message: 'ok' } as never)
    delMock.mockResolvedValue({ success: false, message: 'locked' } as never)
    postMock.mockResolvedValue({ success: true, message: 'created' } as never)

    await expect(loadStatusStatistics()).resolves.toEqual({ online: 2, offline: 1 })
    await expect(setAgentPosition('agent-1', 'senior')).resolves.toEqual({
      ok: true,
      message: 'ok',
    })
    await expect(batchTransferAgents(['agent-1', 'agent-2'], 42)).resolves.toEqual({
      ok: true,
      message: 'ok',
    })
    await expect(deleteAgent('agent-1')).resolves.toEqual({ ok: false, message: 'locked' })
    await expect(
      createAgent({
        email: 'new@example.com',
        password: 'secret',
        displayName: 'New Agent',
        role: 'agent',
      }),
    ).resolves.toEqual({ ok: true, message: 'created' })

    expect(putMock).toHaveBeenCalledWith('/api/agents/agent-1', { position: 'senior' })
    expect(putMock).toHaveBeenCalledWith('/api/agents/batch/transfer', {
      agentIds: ['agent-1', 'agent-2'],
      toTeamId: 42,
    })
    expect(delMock).toHaveBeenCalledWith('/api/agents/agent-1')
    expect(postMock).toHaveBeenCalledWith('/api/auth/register', {
      email: 'new@example.com',
      password: 'secret',
      displayName: 'New Agent',
      role: 'agent',
    })
  })
})

describe('sessions store API wrappers', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    putMock.mockReset()
  })

  it('loads session pages and falls back cleanly on backend failure', async () => {
    getMock
      .mockResolvedValueOnce({
        success: true,
        data: {
          sessions: [{ id: 'sess-1', conversationId: 'conv-1' }],
          pagination: { total: 9 },
        },
      } as never)
      .mockResolvedValueOnce({ success: false, message: 'nope' } as never)

    await expect(loadSessions(3, 15)).resolves.toEqual({
      sessions: [{ id: 'sess-1', conversationId: 'conv-1' }],
      total: 9,
      page: 3,
      ok: true,
    })
    await expect(loadSessions()).resolves.toEqual({ sessions: [], total: 0, page: 1, ok: false })

    expect(getMock).toHaveBeenNthCalledWith(1, '/api/sessions?page=3&pageSize=15')
  })

  it('loads stats and maps lifecycle commands', async () => {
    getMock.mockResolvedValue({ success: true, data: { total: 3, active: 1 } } as never)
    postMock.mockResolvedValueOnce({ success: true } as never)
    postMock.mockResolvedValueOnce({ success: false } as never)
    putMock.mockResolvedValue({ success: true } as never)

    await expect(loadSessionStats()).resolves.toEqual({ total: 3, active: 1 })
    await expect(closeSession('sess-1')).resolves.toBe(true)
    await expect(reopenSession('sess-1')).resolves.toBe(false)
    await expect(updateSessionTopic('sess-1', 'Billing')).resolves.toBe(true)

    expect(postMock).toHaveBeenCalledWith('/api/sessions/sess-1/close', {})
    expect(postMock).toHaveBeenCalledWith('/api/sessions/sess-1/reopen', {})
    expect(putMock).toHaveBeenCalledWith('/api/sessions/sess-1/topic', { topic: 'Billing' })
  })
})
