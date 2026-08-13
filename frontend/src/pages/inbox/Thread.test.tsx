import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

// --- API client -----------------------------------------------------------
const apiMock = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('../../api/client', () => apiMock)

// --- session --------------------------------------------------------------
vi.mock('../../auth/session', () => ({
  session: {
    identity: () => ({ id: 'u1', displayName: 'Me' }),
    currentTeam: () => null,
    teamOptions: () => [],
  },
}))

// --- realtime client (spy subscribe/unsubscribe/onEvent) ------------------
const rt = vi.hoisted(() => ({
  onEvent: vi.fn(() => vi.fn()),
  readMessageEvent: vi.fn((p: Record<string, unknown>) => p),
  subscribeConversation: vi.fn(),
  unsubscribeConversation: vi.fn(),
  getConnectionState: vi.fn(() => 'connected'),
  onConnectionChange: vi.fn(() => vi.fn()),
}))
vi.mock('../../realtime/client', () => rt)

// --- stores ---------------------------------------------------------------
const filesMock = vi.hoisted(() => ({
  loadConversationFiles: vi.fn().mockResolvedValue([]),
  uploadConversationFile: vi.fn().mockResolvedValue({
    attachment: { id: 'att1', filename: 'a.png', contentType: 'image/png' },
  }),
}))
vi.mock('../../stores/files', () => filesMock)
vi.mock('../../stores/delayedMessages', () => ({
  loadPendingDelayed: vi.fn().mockResolvedValue([]),
  scheduleDelayed: vi.fn().mockResolvedValue({ ok: true }),
}))
vi.mock('../../stores/conversations', () => ({
  assignConversation: vi.fn().mockResolvedValue(true),
  conversationsStore: { get: () => ({ items: [] }) },
}))

// --- child components: minimal stand-ins that expose what we drive/assert -
vi.mock('./ThreadHeader', () => ({ ThreadHeader: () => null }))
vi.mock('./FilesDrawer', () => ({ FilesDrawer: () => null }))
vi.mock('./ScheduleDrawer', () => ({ ScheduleDrawer: () => null }))
vi.mock('../../components/ConversationAssign', () => ({ AssignMenu: () => null }))
vi.mock('../../components/ui', () => ({ Toast: () => null }))
vi.mock('./MessageList', () => ({
  MessageList: ({ messages, error }: { messages: Array<{ id: string; content?: string; pending?: boolean; deliveryStatus?: string; rejectCode?: string; rejectMessage?: string }>; error: string | null }) => (
    <div>
      {error && <p role="alert">{error}</p>}
      {messages.map((m) => (
        <div key={m.id} data-testid="msg" data-pending={m.pending ? '1' : '0'} data-status={m.deliveryStatus} data-reject-code={m.rejectCode}>
          {m.content}
          {m.rejectMessage}
        </div>
      ))}
    </div>
  ),
}))
vi.mock('./MessageComposer', () => ({
  MessageComposer: ({
    draft,
    setDraft,
    onSubmit,
    onAddFiles,
  }: {
    draft: string
    setDraft: (v: string) => void
    onSubmit: (e: React.FormEvent) => void
    onAddFiles: (files: File[]) => void
  }) => (
    <form onSubmit={onSubmit}>
      <input aria-label="訊息" value={draft} onChange={(e) => setDraft(e.target.value)} />
      <button type="submit">送出</button>
      <button type="button" onClick={() => onAddFiles([new File(['x'], 'a.png', { type: 'image/png' })])}>
        加檔
      </button>
    </form>
  ),
}))

import { Thread } from './Thread'

function renderThread() {
  return render(
    <Thread convId="c1" meta={{ platform: 'line', customerName: 'Alice' }} onMetaLoaded={vi.fn()} />,
  )
}

beforeEach(() => {
  vi.clearAllMocks()
  apiMock.get.mockImplementation((url: string) => {
    if (url === '/api/conversations/c1/messages') {
      return Promise.resolve({
        success: true,
        data: {
          items: [
            { id: 'm1', content: 'first', senderType: 'customer' },
            { id: 'm2', content: 'second', senderType: 'agent' },
          ],
        },
      })
    }
    return Promise.resolve({ success: true, data: { customerName: 'Alice' } })
  })
  apiMock.post.mockResolvedValue({ success: true, data: { id: 'real1' } })
  rt.onEvent.mockImplementation(() => vi.fn())
})

afterEach(() => cleanup())

describe('Thread', () => {
  it('loads and renders the conversation messages', async () => {
    renderThread()
    expect(await screen.findByText('first')).toBeTruthy()
    expect(screen.getByText('second')).toBeTruthy()
    expect(apiMock.get).toHaveBeenCalledWith('/api/conversations/c1/messages')
  })

  it('optimistically shows a pending message, then confirms it on success', async () => {
    renderThread()
    await screen.findByText('first')

    fireEvent.change(screen.getByLabelText('訊息'), { target: { value: 'hello' } })
    fireEvent.click(screen.getByRole('button', { name: '送出' }))

    // Optimistic bubble appears immediately.
    expect(screen.getByText('hello')).toBeTruthy()
    // After the POST resolves, it is confirmed (no longer pending).
    await waitFor(() => {
      const msg = screen.getAllByTestId('msg').find((n) => n.textContent === 'hello')
      expect(msg?.getAttribute('data-pending')).toBe('0')
    })
    expect(apiMock.post).toHaveBeenCalledWith(
      '/api/conversations/c1/messages',
      expect.objectContaining({ content: 'hello', senderId: 'u1' }),
    )
  })

  it('rolls back the optimistic message and restores the draft on failure', async () => {
    apiMock.post.mockResolvedValue({ success: false, message: 'boom' })
    renderThread()
    await screen.findByText('first')

    fireEvent.change(screen.getByLabelText('訊息'), { target: { value: 'oops' } })
    fireEvent.click(screen.getByRole('button', { name: '送出' }))

    // The optimistic bubble is removed and the error surfaces.
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe('boom'))
    expect(screen.queryByText('oops')).toBeNull()
    // Draft is restored for retry.
    expect((screen.getByLabelText('訊息') as HTMLInputElement).value).toBe('oops')
  })

  it('subscribes to the conversation on mount and cleans up on unmount', async () => {
    const off = vi.fn()
    rt.onEvent.mockImplementation(() => off)
    const { unmount } = renderThread()
    await waitFor(() => expect(rt.subscribeConversation).toHaveBeenCalledWith('c1'))

    unmount()
    expect(rt.unsubscribeConversation).toHaveBeenCalledWith('c1')
    // all event handlers (new_message + realtime_reconnected + message_updated) are detached
    expect(off).toHaveBeenCalledTimes(3)
  })

  it('uploads a file through the composer entry point', async () => {
    renderThread()
    await screen.findByText('first')
    fireEvent.click(screen.getByRole('button', { name: '加檔' }))
    await waitFor(() =>
      expect(filesMock.uploadConversationFile).toHaveBeenCalledWith('c1', expect.any(File)),
    )
  })

  it('applies message_updated delivery state to the matching message', async () => {
    const handlers = new Map<string, (payload: Record<string, unknown>) => void>()
    rt.onEvent.mockImplementation(((event: string, handler: (payload: Record<string, unknown>) => void) => {
      handlers.set(event, handler)
      return vi.fn()
    }) as never)
    renderThread()
    await screen.findByText('second')

    handlers.get('message_updated')?.({
      messageId: 'm2',
      conversationId: 'c1',
      deliveryStatus: 'failed',
      isSent: false,
      rejectCode: 'meta_window_closed',
      error: '超出 24 小時客服回覆窗',
    })

    await waitFor(() => {
      const message = screen.getAllByTestId('msg').find((node) => node.textContent?.includes('second'))
      expect(message?.getAttribute('data-status')).toBe('failed')
      expect(message?.getAttribute('data-reject-code')).toBe('meta_window_closed')
      expect(message?.textContent).toContain('超出 24 小時客服回覆窗')
    })
  })

  it('keeps a delivery update that arrives before the send response', async () => {
    // A credential-less send fails with no network round-trip, so the outcome
    // can beat the POST that swaps the optimistic temp id for the real one.
    const handlers = new Map<string, (payload: Record<string, unknown>) => void>()
    rt.onEvent.mockImplementation(((event: string, handler: (payload: Record<string, unknown>) => void) => {
      handlers.set(event, handler)
      return vi.fn()
    }) as never)
    let resolvePost: (value: unknown) => void = () => {}
    apiMock.post.mockImplementation(() => new Promise((resolve) => { resolvePost = resolve }))

    renderThread()
    await screen.findByText('first')
    fireEvent.change(screen.getByLabelText('訊息'), { target: { value: 'hello' } })
    fireEvent.click(screen.getByRole('button', { name: '送出' }))
    await screen.findByText('hello')

    handlers.get('message_updated')?.({
      messageId: 'real1',
      conversationId: 'c1',
      deliveryStatus: 'failed',
      isSent: false,
      rejectCode: 'missing_credentials',
      error: 'Facebook：頻道存取權杖已失效或過期，請至頻道管理重新設定憑證',
    })
    resolvePost({ success: true, data: { id: 'real1' } })

    await waitFor(() => {
      const message = screen.getAllByTestId('msg').find((node) => node.textContent?.includes('hello'))
      expect(message?.getAttribute('data-pending')).toBe('0')
      expect(message?.getAttribute('data-status')).toBe('failed')
      expect(message?.getAttribute('data-reject-code')).toBe('missing_credentials')
    })
  })
})
