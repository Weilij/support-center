// Conversation list container (CRD §8.1 List Conversations + mark-as-read
// with optimistic unread clearing).

import { get, put } from '../api/client'
import { Store } from './store'

export interface Conversation {
  id: string
  customerName?: string
  status: string
  priority: string
  teamId?: number | null
  lastMessage?: string
  lastMessageAt?: string
  unreadCount?: number
  [key: string]: unknown
}

interface ConversationsState {
  items: Conversation[]
  total: number
  page: number
  busy: boolean
  error: string | null
}

const FRESH_MS = 30_000

export const conversationsStore = new Store<ConversationsState>({
  items: [],
  total: 0,
  page: 1,
  busy: false,
  error: null,
})

export async function loadConversations(page = 1, force = false): Promise<void> {
  const current = conversationsStore.get()
  if (!force && page === current.page && conversationsStore.isFresh(FRESH_MS)) return
  conversationsStore.update((s) => ({ ...s, busy: true, error: null }))
  const resp = await get<{ items?: Conversation[]; conversations?: Conversation[]; total?: number }>(
    `/api/conversations?page=${page}`,
  )
  if (resp.success && resp.data) {
    const items = resp.data.items ?? resp.data.conversations ?? []
    conversationsStore.set({
      items,
      total: resp.data.total ?? items.length,
      page,
      busy: false,
      error: null,
    })
    conversationsStore.markFresh()
  } else {
    conversationsStore.update((s) => ({
      ...s,
      busy: false,
      error: resp.message ?? 'load failed',
    }))
  }
}

/// Optimistically clear the unread badge, reverting if the server refuses.
export function markConversationRead(id: string): Promise<boolean> {
  return conversationsStore.optimistic(
    (s) => ({
      ...s,
      items: s.items.map((c) => (c.id === id ? { ...c, unreadCount: 0 } : c)),
    }),
    () => put(`/api/conversations/${id}/read`),
  )
}

/// Real-time reconciliation: a pushed new-message event bumps the affected
/// conversation to the top with an incremented unread badge.
export function applyIncomingMessage(conversationId: string, preview: string, at: string) {
  conversationsStore.update((s) => {
    const existing = s.items.find((c) => c.id === conversationId)
    if (!existing) {
      conversationsStore.invalidate()
      return s
    }
    const bumped: Conversation = {
      ...existing,
      lastMessage: preview,
      lastMessageAt: at,
      unreadCount: (existing.unreadCount ?? 0) + 1,
    }
    return { ...s, items: [bumped, ...s.items.filter((c) => c.id !== conversationId)] }
  })
}
