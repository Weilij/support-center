import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { teamsStore } from '../stores/teams'
import {
  assignConversation,
  transferConversation,
  unassignConversation,
} from '../stores/conversations'
import { AssignDialog } from './ConversationAssign'

vi.mock('../stores/conversations', () => ({
  assignConversation: vi.fn(async () => true),
  transferConversation: vi.fn(async () => true),
  unassignConversation: vi.fn(async () => true),
}))

const assignMock = vi.mocked(assignConversation)
const transferMock = vi.mocked(transferConversation)
const unassignMock = vi.mocked(unassignConversation)

describe('AssignDialog', () => {
  afterEach(() => {
    cleanup()
  })

  beforeEach(() => {
    assignMock.mockClear()
    transferMock.mockClear()
    unassignMock.mockClear()
    teamsStore.set({
      items: [
        { id: 10, name: 'Support' },
        { id: 20, name: 'Billing' },
      ],
      busy: false,
      error: null,
    })
    teamsStore.markFresh()
  })

  it('requires and submits a target team for assignment', async () => {
    const { getByText, getByLabelText } = render(
      <AssignDialog open mode="assign" conversationId="c1" onClose={() => {}} />,
    )

    fireEvent.click(getByText('確認'))
    expect(getByText('請選擇團隊')).toBeTruthy()
    expect(assignMock).not.toHaveBeenCalled()

    fireEvent.change(getByLabelText('指派團隊'), { target: { value: '10' } })
    fireEvent.change(getByLabelText('原因（選填，提供後會寫入路由紀錄）'), {
      target: { value: 'handoff' },
    })
    fireEvent.click(getByText('確認'))

    await waitFor(() => expect(assignMock).toHaveBeenCalledWith('c1', 10, 'handoff'))
    expect(assignMock.mock.calls[0]).not.toContain('agent-1')
  })

  it('submits team-to-team transfer without an agent id', async () => {
    const { getByText, getByLabelText, queryByText } = render(
      <AssignDialog
        open
        mode="transfer"
        conversationId="c1"
        currentTeamId={10}
        onClose={() => {}}
      />,
    )

    expect(queryByText('Support')).toBeNull()
    fireEvent.change(getByLabelText('轉接至團隊'), { target: { value: '20' } })
    fireEvent.click(getByText('確認'))

    await waitFor(() => expect(transferMock).toHaveBeenCalledWith('c1', 20, 10, undefined))
    expect(transferMock.mock.calls[0]).not.toContain('agent-1')
  })
})
