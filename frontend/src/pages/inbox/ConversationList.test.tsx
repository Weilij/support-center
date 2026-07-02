import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ConversationList } from './ConversationList'
import type { Conversation } from '../../stores/conversations'

vi.mock('../../auth/session', () => ({
  session: {
    teamOptions: () => [{ id: '5', name: 'A', isPrimary: true }],
    isAdmin: () => false,
  },
}))

afterEach(() => {
  cleanup()
})

function conversation(overrides: Partial<Conversation> & { id: string }): Conversation {
  return {
    ...overrides,
    id: overrides.id,
    status: 'active',
    priority: 'normal',
    customerName: overrides.customerName ?? overrides.id,
    lastMessage: overrides.lastMessage ?? '',
    lastMessageAt: overrides.lastMessageAt ?? new Date().toISOString(),
    unreadCount: overrides.unreadCount ?? 0,
    teamId: overrides.teamId,
  }
}

describe('ConversationList 我的團隊 tab', () => {
  it('shows only conversations whose team is one of the agent own teams', () => {
    render(
      <ConversationList
        items={[
          conversation({ id: 'a', teamId: 5, customerName: 'Conv X' }),
          conversation({ id: 'b', teamId: 9, customerName: 'Conv Y' }),
          conversation({ id: 'c', teamId: null, customerName: 'Conv Z' }),
        ]}
        busy={false}
        selectedId={undefined}
        onSelect={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByText('我的團隊'))

    expect(screen.getByText('Conv X')).toBeTruthy()
    expect(screen.queryByText('Conv Y')).toBeNull()
    expect(screen.queryByText('Conv Z')).toBeNull()
  })

  it('offers 全部/未讀/我的團隊 tabs and no 待跟進', () => {
    render(
      <ConversationList
        items={[conversation({ id: 'a', teamId: 5, customerName: 'Conv X' })]}
        busy={false}
        selectedId={undefined}
        onSelect={vi.fn()}
      />,
    )
    expect(screen.getByText('全部')).toBeTruthy()
    expect(screen.getByText('未讀')).toBeTruthy()
    expect(screen.getByText('我的團隊')).toBeTruthy()
    expect(screen.queryByText('待跟進')).toBeNull()
  })
})
