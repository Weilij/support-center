// Browser realtime client (CRD §8.3): connect via the mcss_access HttpOnly
// cookie (sent automatically on the WS handshake — same-origin), route pushed
// events to the state containers, reconnect with capped backoff, and
// re-handshake whenever credentials change.

import { session, authChanged } from '../auth/session'
import { applyIncomingMessage } from '../stores/conversations'

type Handler = (payload: Record<string, unknown>) => void

const MAX_BACKOFF_MS = 30_000

let socket: WebSocket | null = null
let backoff = 1000
let closedByUs = false
let openedOnce = false
const handlers = new Map<string, Set<Handler>>()
// Conversations the UI wants live updates for. Subscribe frames sent before the
// socket reaches OPEN are dropped (sendFrame has no queue), so we remember the
// desired set and (re)send it on every successful handshake — this fixes both
// the initial-load race (Inbox mounts and subscribes while the WS is still
// CONNECTING) and re-subscription after a reconnect.
const desiredConversations = new Set<string>()

export function onEvent(event: string, fn: Handler): () => void {
  if (!handlers.has(event)) handlers.set(event, new Set())
  handlers.get(event)!.add(fn)
  return () => handlers.get(event)?.delete(fn)
}

function route(event: string, payload: Record<string, unknown>) {
  handlers.get(event)?.forEach((fn) => fn(payload))
}

export interface IncomingMessage {
  conversationId: string
  id: string
  content: string
  senderType: string
  senderId: string
  timestamp: string
  /// True when this is an agent message sent by the current user — the sender
  /// already rendered it optimistically, so handlers skip it (avoids duplicate
  /// bubbles and a wrong unread bump on one's own messages).
  isOwn: boolean
  messageType: string
  media?: Record<string, unknown>
}

// The server uses two `new_message` payload shapes: inbound (webhook) nests the
// fields under `message`, while outbound (agent send) is flat with `messageId`.
// Read both so every event renders with real content and the right sender side.
export function readMessageEvent(payload: Record<string, unknown>): IncomingMessage {
  const nested = (payload.message ?? {}) as Record<string, unknown>
  const senderId = String(nested.senderId ?? payload.senderId ?? '')
  const senderType = String(nested.senderType ?? payload.senderType ?? 'customer')
  const me = session.identity()?.id
  const messageType = String(nested.type ?? payload.messageType ?? nested.messageType ?? 'text')
  let media = nested.media as Record<string, unknown> | undefined
  if (!media && nested.metadata != null) {
    let meta: Record<string, unknown> | undefined
    if (typeof nested.metadata === 'string') {
      try { meta = JSON.parse(nested.metadata) as Record<string, unknown> } catch { meta = undefined }
    } else {
      meta = nested.metadata as Record<string, unknown>
    }
    // REST nests under metadata.media; the realtime inbound payload puts the
    // media object directly in metadata — accept either.
    media = (meta?.media as Record<string, unknown> | undefined) ?? meta
  }
  return {
    conversationId: String(payload.conversationId ?? ''),
    id: String(nested.id ?? payload.messageId ?? ''),
    content: String(nested.content ?? payload.content ?? ''),
    senderType,
    senderId,
    timestamp: String(nested.timestamp ?? payload.timestamp ?? ''),
    isOwn: senderType === 'agent' && me != null && senderId === String(me),
    messageType,
    media,
  }
}

// Built-in routing: pushed new-message events reconcile the conversation list.
onEvent('new_message', (payload) => {
  const m = readMessageEvent(payload)
  if (!m.conversationId || m.isOwn) return
  applyIncomingMessage(m.conversationId, m.content, m.timestamp || new Date().toISOString())
})

export function connectRealtime(): void {
  // Gate on being authenticated (identity cached from login / /me).
  // The mcss_access HttpOnly cookie is sent automatically on the WS handshake
  // — no ?token= query param needed.
  if (!session.identity() || socket) return
  closedByUs = false
  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws'
  const url = `${scheme}://${window.location.host}/api/websocket/connect`
  const ws = new WebSocket(url)
  socket = ws

  ws.onopen = () => {
    const reconnected = openedOnce
    openedOnce = true
    backoff = 1000 // reset after a successful handshake
    // Flush every desired subscription now that the socket is OPEN — covers the
    // initial-load race and re-establishes subscriptions after a reconnect.
    desiredConversations.forEach((id) => sendFrame({ type: 'subscribe', conversationId: id }))
    if (reconnected) {
      route('realtime_reconnected', { subscribedConversationIds: [...desiredConversations] })
    }
  }
  ws.onmessage = (raw) => {
    try {
      const frame = JSON.parse(String(raw.data)) as {
        type?: string
        payload?: Record<string, unknown>
      } & Record<string, unknown>
      // The server wraps events as { type, payload, timestamp }. Hand the inner
      // payload to handlers — they read fields like conversationId/message off
      // it directly. Raw frames without a payload wrapper fall back to the frame.
      if (frame.type) route(frame.type, (frame.payload ?? frame) as Record<string, unknown>)
    } catch {
      /* non-JSON frames are ignored */
    }
  }
  ws.onclose = () => {
    socket = null
    if (closedByUs) return
    // Capped-backoff reconnection (CRD §8.3).
    const delay = backoff
    backoff = Math.min(backoff * 2, MAX_BACKOFF_MS)
    setTimeout(() => connectRealtime(), delay)
  }
  ws.onerror = () => ws.close()
}

export function disconnectRealtime(): void {
  closedByUs = true
  openedOnce = false
  socket?.close()
  socket = null
}

export function sendFrame(frame: Record<string, unknown>): boolean {
  if (socket?.readyState === WebSocket.OPEN) {
    socket.send(JSON.stringify(frame))
    return true
  }
  return false
}

export function subscribeConversation(conversationId: string) {
  // Remember the intent so a reconnect (or a not-yet-OPEN socket) re-sends it.
  desiredConversations.add(conversationId)
  sendFrame({ type: 'subscribe', conversationId })
}

export function unsubscribeConversation(conversationId: string) {
  desiredConversations.delete(conversationId)
  sendFrame({ type: 'unsubscribe', conversationId })
}

// Auth changes force a fresh handshake (CRD §8.1 renew-credential behavior).
// Gate on identity presence — the cookie is sent automatically by the browser.
authChanged.on(() => {
  disconnectRealtime()
  if (session.identity()) connectRealtime()
})
