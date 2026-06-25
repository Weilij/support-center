import { beforeEach, describe, expect, it, vi } from 'vitest'

import { get, post } from '../api/client'
import {
  assignConversation,
  assignConversationToAgent,
  conversationsStore,
  loadConversations,
  normalizeConversations,
  transferConversation,
} from '../stores/conversations'

vi.mock('../api/client', () => ({
  get: vi.fn(),
  put: vi.fn(),
  post: vi.fn(),
}))

const postMock = vi.mocked(post)
const getMock = vi.mocked(get)

describe('conversation routing store', () => {
  beforeEach(() => {
    getMock.mockReset()
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

  it('loads only validated conversations and ignores malformed pagination', async () => {
    getMock.mockResolvedValue({
      success: true,
      data: {
        items: [
          { id: 'bad' },
          { id: 'c1', status: 'active', priority: 'normal', lastMessage: { content: 'hi' } },
        ],
      },
      pagination: { total: 'wrong' },
    } as never)

    await loadConversations(2, true)

    expect(conversationsStore.get()).toMatchObject({
      items: [{ id: 'c1', status: 'active', priority: 'normal', lastMessage: 'hi' }],
      total: 1,
      page: 2,
      busy: false,
      error: null,
    })
  })
})

describe('conversation response normalization', () => {
  it('accepts supported list containers and extracts last-message content', () => {
    expect(
      normalizeConversations({
        conversations: [
          {
            id: 'c1',
            status: 'active',
            priority: 'normal',
            lastMessage: { content: 'hello' },
          },
        ],
      }),
    ).toEqual([
      {
        id: 'c1',
        status: 'active',
        priority: 'normal',
        lastMessage: 'hello',
      },
    ])
  })

  it('drops malformed rows instead of casting them into conversations', () => {
    expect(
      normalizeConversations({
        items: [
          'bad-row',
          null,
          { id: 'missing-required-fields' },
          {
            id: 'c2',
            status: 'active',
            priority: 'normal',
            lastMessage: { content: 123 },
          },
        ],
      }),
    ).toEqual([
      {
        id: 'c2',
        status: 'active',
        priority: 'normal',
        lastMessage: '123',
      },
    ])
  })
})
