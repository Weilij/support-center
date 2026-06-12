// Browser realtime client (CRD §8.3): connect with the credential as a query
// parameter, route pushed events to the state containers, reconnect with
// capped backoff, and re-handshake whenever credentials change.

import { session, authChanged } from '../auth/session'
import { applyIncomingMessage } from '../stores/conversations'

type Handler = (payload: Record<string, unknown>) => void

const MAX_BACKOFF_MS = 30_000

let socket: WebSocket | null = null
let backoff = 1000
let closedByUs = false
const handlers = new Map<string, Set<Handler>>()

export function onEvent(event: string, fn: Handler): () => void {
  if (!handlers.has(event)) handlers.set(event, new Set())
  handlers.get(event)!.add(fn)
  return () => handlers.get(event)?.delete(fn)
}

function route(event: string, payload: Record<string, unknown>) {
  handlers.get(event)?.forEach((fn) => fn(payload))
}

// Built-in routing: pushed new-message events reconcile the conversation list.
onEvent('new_message', (payload) => {
  const conversationId = String(payload.conversationId ?? '')
  const message = (payload.message ?? {}) as Record<string, unknown>
  if (conversationId) {
    applyIncomingMessage(
      conversationId,
      String(message.content ?? ''),
      String(message.timestamp ?? new Date().toISOString()),
    )
  }
})

export function connectRealtime(): void {
  const token = session.accessToken()
  if (!token || socket) return
  closedByUs = false
  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws'
  const url = `${scheme}://${window.location.host}/api/websocket/connect?token=${encodeURIComponent(token)}`
  const ws = new WebSocket(url)
  socket = ws

  ws.onopen = () => {
    backoff = 1000 // reset after a successful handshake
  }
  ws.onmessage = (raw) => {
    try {
      const frame = JSON.parse(String(raw.data)) as { type?: string } & Record<string, unknown>
      if (frame.type) route(frame.type, frame)
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
  sendFrame({ type: 'subscribe', conversationId })
}

// Credential changes force a fresh handshake with the new token (CRD §8.1
// renew-credential behavior).
authChanged.on(() => {
  disconnectRealtime()
  if (session.accessToken()) connectRealtime()
})
