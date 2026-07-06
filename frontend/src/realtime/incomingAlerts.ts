// Incoming-message alerts (Phase 2.3): a short sound cue and, when the tab is
// backgrounded, a desktop notification that deep-links to the conversation.
// Fires only for other people's messages in conversations the agent is not
// actively looking at.

import { onEvent, readMessageEvent } from './client'

const SOUND_KEY = 'mcss.notifySound'

export function soundEnabled(): boolean {
  return localStorage.getItem(SOUND_KEY) !== '0' // default on
}

export function setSoundEnabled(on: boolean): void {
  localStorage.setItem(SOUND_KEY, on ? '1' : '0')
}

export function notificationPermission(): NotificationPermission | 'unsupported' {
  if (typeof Notification === 'undefined') return 'unsupported'
  return Notification.permission
}

/// Ask for desktop-notification permission (must be called from a user gesture).
export async function requestNotificationPermission(): Promise<NotificationPermission | 'unsupported'> {
  if (typeof Notification === 'undefined') return 'unsupported'
  if (Notification.permission !== 'default') return Notification.permission
  return Notification.requestPermission()
}

// ── sound ───────────────────────────────────────────────────────────────────
let audioCtx: AudioContext | null = null

function playChime(): void {
  try {
    const Ctx = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
    if (!Ctx) return
    audioCtx = audioCtx ?? new Ctx()
    if (audioCtx.state === 'suspended') void audioCtx.resume()
    const now = audioCtx.currentTime
    const osc = audioCtx.createOscillator()
    const gain = audioCtx.createGain()
    osc.type = 'sine'
    osc.frequency.setValueAtTime(880, now)
    osc.frequency.setValueAtTime(1174, now + 0.09)
    gain.gain.setValueAtTime(0.0001, now)
    gain.gain.exponentialRampToValueAtTime(0.14, now + 0.02)
    gain.gain.exponentialRampToValueAtTime(0.0001, now + 0.28)
    osc.connect(gain)
    gain.connect(audioCtx.destination)
    osc.start(now)
    osc.stop(now + 0.3)
  } catch {
    /* audio unavailable — ignore */
  }
}

// ── active conversation (from the SPA route) ─────────────────────────────────
function activeConversationId(): string | null {
  const match = window.location.pathname.match(/^\/conversations\/(.+)$/)
  return match ? decodeURIComponent(match[1]) : null
}

let navigateTo: ((path: string) => void) | null = null

function showDesktopNotification(conversationId: string, title: string, body: string): void {
  if (typeof Notification === 'undefined' || Notification.permission !== 'granted') return
  try {
    const notification = new Notification(title, { body, tag: conversationId })
    notification.onclick = () => {
      window.focus()
      navigateTo?.(`/conversations/${conversationId}`)
      notification.close()
    }
  } catch {
    /* notification construction can throw on some platforms — ignore */
  }
}

/// Wire the new-message handler. `navigate` is used for notification click-through.
/// Returns an unsubscribe function.
export function initIncomingAlerts(navigate: (path: string) => void): () => void {
  navigateTo = navigate
  return onEvent('new_message', (payload) => {
    const message = readMessageEvent(payload)
    if (!message.conversationId || message.isOwn) return

    const foreground = document.visibilityState === 'visible'
    const looking = foreground && activeConversationId() === message.conversationId
    // Skip only the thread the agent is actively watching.
    if (looking) return

    if (soundEnabled()) playChime()
    if (!foreground) {
      const preview = message.content?.trim() || '你有一則新訊息'
      showDesktopNotification(message.conversationId, '新訊息', preview.slice(0, 120))
    }
  })
}
