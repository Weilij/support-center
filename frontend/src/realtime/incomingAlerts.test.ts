import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

type Handler = (payload: Record<string, unknown>) => void

// Capture the handler registered with onEvent so we can drive it directly.
const onEventMock = vi.hoisted(() =>
  vi.fn((_event: string, _fn: (payload: Record<string, unknown>) => void) => () => {}),
)
vi.mock('./client', () => ({
  onEvent: onEventMock,
  readMessageEvent: (p: Record<string, unknown>) => p,
}))

import { initIncomingAlerts, setSoundEnabled } from './incomingAlerts'

function lastHandler(): Handler {
  return onEventMock.mock.calls[onEventMock.mock.calls.length - 1][1]
}

let created: Array<{ title: string; body?: string }>

function setVisibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { configurable: true, get: () => state })
}

beforeEach(() => {
  created = []
  onEventMock.mockClear()
  setSoundEnabled(false) // avoid touching AudioContext in tests
  class MockNotification {
    static permission: NotificationPermission = 'granted'
    onclick: (() => void) | null = null
    constructor(public title: string, public options?: { body?: string; tag?: string }) {
      created.push({ title, body: options?.body })
    }
    close() {}
  }
  ;(globalThis as unknown as { Notification: unknown }).Notification = MockNotification
  history.pushState({}, '', '/conversations/active-1')
  setVisibility('visible')
})

afterEach(() => {
  history.pushState({}, '', '/')
})

describe('incoming message alerts', () => {
  it('ignores the agent’s own messages', () => {
    initIncomingAlerts(vi.fn())
    setVisibility('hidden')
    lastHandler()({ conversationId: 'other', isOwn: true, content: 'hi' })
    expect(created).toHaveLength(0)
  })

  it('skips the conversation currently open in the foreground', () => {
    initIncomingAlerts(vi.fn())
    lastHandler()({ conversationId: 'active-1', isOwn: false, content: 'hi' })
    expect(created).toHaveLength(0)
  })

  it('raises a desktop notification for another conversation while backgrounded', () => {
    const navigate = vi.fn()
    initIncomingAlerts(navigate)
    setVisibility('hidden')
    lastHandler()({ conversationId: 'other-2', isOwn: false, content: 'ping' })
    expect(created).toEqual([{ title: '新訊息', body: 'ping' }])
  })

  it('does not raise a desktop notification while in the foreground', () => {
    initIncomingAlerts(vi.fn())
    lastHandler()({ conversationId: 'other-3', isOwn: false, content: 'ping' })
    expect(created).toHaveLength(0)
  })
})
