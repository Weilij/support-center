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

/// The list endpoint returns the conversations as a bare array in `data`;
/// lastMessage is an object whose preview lives at .content.
function normalize(raw: unknown): Conversation[] {
  const list = Array.isArray(raw)
    ? raw
    : ((raw as { items?: unknown[]; conversations?: unknown[] })?.items ??
       (raw as { conversations?: unknown[] })?.conversations ??
       [])
  return (list as Record<string, unknown>[]).map((c) => ({
    ...(c as Conversation),
    lastMessage:
      typeof c.lastMessage === 'object' && c.lastMessage !== null
        ? String((c.lastMessage as { content?: unknown }).content ?? '')
        : (c.lastMessage as string | undefined),
  }))
}

export async function loadConversations(page = 1, force = false): Promise<void> {
  const current = conversationsStore.get()
  if (!force && page === current.page && conversationsStore.isFresh(FRESH_MS)) return
  conversationsStore.update((s) => ({ ...s, busy: true, error: null }))
  const resp = await get<unknown>(`/api/conversations?page=${page}`)
  if (resp.success && resp.data !== undefined) {
    const items = normalize(resp.data)
    const total = (resp as { pagination?: { total?: number } }).pagination?.total ?? items.length
    conversationsStore.set({
      items,
      total,
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
