import { beforeEach, describe, expect, it, vi } from 'vitest'

import { post } from '../api/client'
import {
  assignConversation,
  assignConversationToAgent,
  conversationsStore,
  transferConversation,
} from '../stores/conversations'

vi.mock('../api/client', () => ({
  get: vi.fn(),
  put: vi.fn(),
  post: vi.fn(),
}))

const postMock = vi.mocked(post)

describe('conversation routing store', () => {
  beforeEach(() => {
    postMock.mockReset()
    postMock.mockResolvedValue({ success: true })
    conversationsStore.set({
      items: [{ id: 'c1', status: 'active', priority: 'normal' }],
      total: 1,
      page: 1,
      busy: false,
      error: null,
    })
  })

  it('assigns a conversation with teamId only', async () => {
    await expect(assignConversation('c1', 42, 'vip handoff')).resolves.toBe(true)

    expect(postMock).toHaveBeenCalledWith('/api/conversations/c1/assign', {
      teamId: 42,
      reason: 'vip handoff',
    })
    expect(postMock.mock.calls[0][1]).not.toHaveProperty('agentId')
    expect(postMock.mock.calls[0][1]).not.toHaveProperty('assigneeId')
  })

  it('transfers a conversation with team ids only', async () => {
    await expect(transferConversation('c1', 7, 3, 'team coverage')).resolves.toBe(true)

    expect(postMock).toHaveBeenCalledWith('/api/conversations/c1/transfer', {
      toTeamId: 7,
      fromTeamId: 3,
      reason: 'team coverage',
    })
    expect(postMock.mock.calls[0][1]).not.toHaveProperty('agentId')
    expect(postMock.mock.calls[0][1]).not.toHaveProperty('assigneeId')
  })

  it('rejects deprecated individual-agent assignment locally', async () => {
    await expect(assignConversationToAgent('c1', 'agent-1')).resolves.toBe(false)

    expect(postMock).not.toHaveBeenCalled()
    expect(conversationsStore.get().items[0]).toMatchObject({
      id: 'c1',
      status: 'active',
    })
    expect(conversationsStore.get().items[0]).not.toHaveProperty('teamId')
    expect(conversationsStore.get().items[0]).not.toHaveProperty('agentId')
    expect(conversationsStore.get().items[0]).not.toHaveProperty('assigneeId')
  })
})
