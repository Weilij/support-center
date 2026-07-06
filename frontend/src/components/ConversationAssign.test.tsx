import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { teamsStore } from '../stores/teams'
import {
  assignConversation,
  transferConversation,
  unassignConversation,
} from '../stores/conversations'
import { AssignMenu } from './ConversationAssign'

vi.mock('../stores/conversations', () => ({
  assignConversation: vi.fn(async () => true),
  transferConversation: vi.fn(async () => true),
  unassignConversation: vi.fn(async () => true),
}))

const assignMock = vi.mocked(assignConversation)
const transferMock = vi.mocked(transferConversation)
const unassignMock = vi.mocked(unassignConversation)

function seedTeams() {
  teamsStore.set({
    items: [
      { id: 10, name: 'Support' },
      { id: 20, name: 'Billing' },
    ],
    busy: false,
    error: null,
  })
  teamsStore.markFresh()
}

describe('AssignMenu', () => {
  afterEach(cleanup)
  beforeEach(() => {
    assignMock.mockClear()
    transferMock.mockClear()
    unassignMock.mockClear()
    seedTeams()
  })

  it('assigns immediately when a team is picked and no current team exists', async () => {
    const onResult = vi.fn()
    const { getByLabelText, getByText } = render(
      <AssignMenu conversationId="c1" currentTeamId={null} onResult={onResult} />,
    )

    fireEvent.click(getByLabelText('指派團隊')) // open the dropdown
    fireEvent.click(getByText('Support'))

    await waitFor(() => expect(assignMock).toHaveBeenCalledWith('c1', 10, undefined))
    expect(transferMock).not.toHaveBeenCalled()
    await waitFor(() => expect(onResult).toHaveBeenCalledWith('已指派給「Support」'))
  })

  it('transfers (not assigns) when a current team exists', async () => {
    const { getByLabelText, getByText } = render(
      <AssignMenu conversationId="c1" currentTeamId={10} onResult={vi.fn()} />,
    )

    fireEvent.click(getByLabelText('指派團隊'))
    fireEvent.click(getByText('Billing'))

    await waitFor(() => expect(transferMock).toHaveBeenCalledWith('c1', 20, 10, undefined))
    expect(assignMock).not.toHaveBeenCalled()
  })

  it('passes an optional reason through when provided', async () => {
    const { getByLabelText, getByText } = render(
      <AssignMenu conversationId="c1" currentTeamId={null} onResult={vi.fn()} />,
    )

    fireEvent.click(getByLabelText('指派團隊'))
    fireEvent.click(getByText('填寫原因…'))
    fireEvent.change(getByLabelText('指派原因'), { target: { value: 'VIP handoff' } })
    fireEvent.click(getByText('Support'))

    await waitFor(() => expect(assignMock).toHaveBeenCalledWith('c1', 10, 'VIP handoff'))
  })

  it('unassigns via the 取消指派 action', async () => {
    const { getByLabelText, getByText } = render(
      <AssignMenu conversationId="c1" currentTeamId={10} onResult={vi.fn()} />,
    )

    fireEvent.click(getByLabelText('指派團隊'))
    fireEvent.click(getByText('取消指派'))

    await waitFor(() => expect(unassignMock).toHaveBeenCalledWith('c1', undefined))
  })

  it('does not resubmit when the current team is re-picked', async () => {
    const { getByLabelText, getByText } = render(
      <AssignMenu conversationId="c1" currentTeamId={10} onResult={vi.fn()} />,
    )

    fireEvent.click(getByLabelText('指派團隊'))
    fireEvent.click(getByText('Support')) // Support is the current team (id 10)

    expect(assignMock).not.toHaveBeenCalled()
    expect(transferMock).not.toHaveBeenCalled()
  })
})
