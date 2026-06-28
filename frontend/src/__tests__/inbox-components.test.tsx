import { createRef } from 'react'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { ConversationList } from '../pages/inbox/ConversationList'
import { MessageList } from '../pages/inbox/MessageList'
import { ThreadHeader } from '../pages/inbox/ThreadHeader'
import type { InboxMessage } from '../pages/inbox/types'
import type { Conversation } from '../stores/conversations'

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

describe('Inbox conversation list', () => {
  it('filters by search text and returns the selected conversation id', () => {
    const onSelect = vi.fn()
    render(
      <ConversationList
        items={[
          conversation({ id: 'c-1', customerName: 'Ada Chen', lastMessage: 'Need invoice' }),
          conversation({ id: 'c-2', customerName: 'Ben Wu', lastMessage: 'Shipping update' }),
        ]}
        busy={false}
        selectedId="c-2"
        onSelect={onSelect}
      />,
    )

    fireEvent.change(screen.getByPlaceholderText('搜尋對話…'), { target: { value: 'invoice' } })

    expect(screen.getByText('Ada Chen')).toBeTruthy()
    expect(screen.queryByText('Ben Wu')).toBeNull()

    fireEvent.click(screen.getByText('Ada Chen'))
    expect(onSelect).toHaveBeenCalledWith('c-1')
  })

  it('shows unread and team-assigned conversations in their tabs', () => {
    render(
      <ConversationList
        items={[
          conversation({ id: 'c-1', customerName: 'Unread Customer', unreadCount: 2 }),
          conversation({ id: 'c-2', customerName: 'Team Customer', teamId: 7 }),
          conversation({ id: 'c-3', customerName: 'Plain Customer' }),
        ]}
        busy={false}
        selectedId={undefined}
        onSelect={vi.fn()}
      />,
    )

    fireEvent.click(screen.getByText('未讀'))
    expect(screen.getByText('Unread Customer')).toBeTruthy()
    expect(screen.queryByText('Team Customer')).toBeNull()

    fireEvent.click(screen.getByText('團隊'))
    expect(screen.getByText('Team Customer')).toBeTruthy()
    expect(screen.queryByText('Unread Customer')).toBeNull()
  })
})

describe('Inbox thread header', () => {
  it('renders channel metadata, counters, and action buttons', () => {
    const onFiles = vi.fn()
    const onSchedule = vi.fn()
    const onAssign = vi.fn()
    const onCustomer = vi.fn()

    render(
      <ThreadHeader
        convId="conv-1"
        meta={{ platform: 'line', customerName: 'Lin Customer' }}
        filesCount={3}
        pendingCount={2}
        onToggleFiles={onFiles}
        onToggleSchedule={onSchedule}
        onAssign={onAssign}
        onToggleCustomerPanel={onCustomer}
        showCustomerPanelToggle
      />,
    )

    expect(screen.getByText('Lin Customer')).toBeTruthy()
    expect(screen.getByText(/透過/)).toBeTruthy()
    expect(screen.getByText('3')).toBeTruthy()
    expect(screen.getByText('2')).toBeTruthy()

    fireEvent.click(screen.getByLabelText('檔案'))
    fireEvent.click(screen.getByLabelText('排程'))
    fireEvent.click(screen.getByLabelText('指派團隊'))
    fireEvent.click(screen.getByLabelText('客戶資訊'))

    expect(onFiles).toHaveBeenCalledTimes(1)
    expect(onSchedule).toHaveBeenCalledTimes(1)
    expect(onAssign).toHaveBeenCalledTimes(1)
    expect(onCustomer).toHaveBeenCalledTimes(1)
  })
})

describe('Inbox message list', () => {
  it('renders error state, inbound avatar, agent read state, and attachment previews', () => {
    const messages: InboxMessage[] = [
      {
        id: 'm-1',
        content: 'hello',
        senderType: 'customer',
        createdAt: new Date().toISOString(),
      },
      {
        id: 'm-2',
        content: 'see file',
        senderType: 'agent',
        createdAt: new Date().toISOString(),
        attachments: [
          {
            id: 'a-1',
            filename: 'proof.png',
            mimeType: 'image/png',
            url: '/proof.png',
          },
        ],
      },
    ]

    render(
      <MessageList
        convId="conv-1"
        messages={messages}
        error="載入失敗"
        customerName="Customer"
        customerAvatarUrl="/customer.png"
        bottomRef={createRef<HTMLDivElement>()}
      />,
    )

    expect(screen.getByRole('alert').textContent).toBe('載入失敗')
    expect(screen.getByText('hello')).toBeTruthy()
    expect(screen.getByText('see file')).toBeTruthy()
    expect(screen.getByAltText('Customer')).toBeTruthy()
    expect(screen.getByAltText('proof.png')).toBeTruthy()
    expect(screen.getByText(/已讀/)).toBeTruthy()
  })
})
