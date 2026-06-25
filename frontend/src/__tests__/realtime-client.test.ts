import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

class MockWebSocket {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  static instances: MockWebSocket[] = []

  readyState = MockWebSocket.CONNECTING
  sent: string[] = []
  onopen: (() => void) | null = null
  onmessage: ((event: { data: string }) => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null

  constructor(readonly url: string) {
    MockWebSocket.instances.push(this)
  }

  send(data: string) {
    this.sent.push(data)
  }

  close() {
    this.readyState = MockWebSocket.CLOSED
    this.onclose?.()
  }

  open() {
    this.readyState = MockWebSocket.OPEN
    this.onopen?.()
  }

  serverClose() {
    this.readyState = MockWebSocket.CLOSED
    this.onclose?.()
  }

  receive(frame: unknown) {
    this.onmessage?.({ data: JSON.stringify(frame) })
  }
}

describe('realtime client', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.resetModules()
    localStorage.clear()
    MockWebSocket.instances = []
    vi.stubGlobal('WebSocket', MockWebSocket)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('connects only after identity exists and flushes pending subscriptions on open', async () => {
    const { session } = await import('../auth/session')
    const realtime = await import('../realtime/client')

    realtime.connectRealtime()
    expect(MockWebSocket.instances).toHaveLength(0)

    session.storeLogin('session-1', { id: 'agent-1', role: 'agent' })
    realtime.connectRealtime()
    expect(MockWebSocket.instances).toHaveLength(1)
    expect(MockWebSocket.instances[0].url).toBe(
      `ws://${window.location.host}/api/websocket/connect`,
    )

    realtime.subscribeConversation('conv-1')
    expect(MockWebSocket.instances[0].sent).toEqual([])

    MockWebSocket.instances[0].open()
    expect(MockWebSocket.instances[0].sent).toEqual([
      JSON.stringify({ type: 'subscribe', conversationId: 'conv-1' }),
    ])
  })

  it('reconnects with backoff and re-subscribes only desired conversations', async () => {
    const { session } = await import('../auth/session')
    const realtime = await import('../realtime/client')

    session.storeLogin('session-1', { id: 'agent-1', role: 'agent' })
    realtime.connectRealtime()
    const first = MockWebSocket.instances[0]
    first.open()

    realtime.subscribeConversation('conv-1')
    expect(first.sent).toContain(JSON.stringify({ type: 'subscribe', conversationId: 'conv-1' }))

    first.serverClose()
    await vi.advanceTimersByTimeAsync(999)
    expect(MockWebSocket.instances).toHaveLength(1)
    await vi.advanceTimersByTimeAsync(1)

    const second = MockWebSocket.instances[1]
    second.open()
    expect(second.sent).toEqual([
      JSON.stringify({ type: 'subscribe', conversationId: 'conv-1' }),
    ])

    realtime.unsubscribeConversation('conv-1')
    expect(second.sent).toContain(JSON.stringify({ type: 'unsubscribe', conversationId: 'conv-1' }))

    second.serverClose()
    await vi.advanceTimersByTimeAsync(1000)
    const third = MockWebSocket.instances[2]
    third.open()
    expect(third.sent).toEqual([])
  })

  it('emits an internal reconnect event after an unexpected disconnect', async () => {
    const { session } = await import('../auth/session')
    const realtime = await import('../realtime/client')
    const reconnects: Record<string, unknown>[] = []

    realtime.onEvent('realtime_reconnected', (payload) => reconnects.push(payload))
    session.storeLogin('session-1', { id: 'agent-1', role: 'agent' })
    realtime.connectRealtime()
    realtime.subscribeConversation('conv-1')

    MockWebSocket.instances[0].open()
    expect(reconnects).toEqual([])

    MockWebSocket.instances[0].serverClose()
    await vi.advanceTimersByTimeAsync(1000)
    MockWebSocket.instances[1].open()

    expect(reconnects).toEqual([{ subscribedConversationIds: ['conv-1'] }])
  })
})
