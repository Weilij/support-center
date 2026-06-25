// Conversation list container (CRD §8.1 List Conversations + mark-as-read
// with optimistic unread clearing).

import { get, put, post } from '../api/client'
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
function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function finiteNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function listPayload(raw: unknown): unknown[] {
  if (Array.isArray(raw)) return raw
  if (!isRecord(raw)) return []
  if (Array.isArray(raw.items)) return raw.items
  if (Array.isArray(raw.conversations)) return raw.conversations
  return []
}

function lastMessagePreview(value: unknown): string | undefined {
  if (typeof value === 'string') return value
  if (!isRecord(value)) return undefined
  const content = value.content
  return content === undefined || content === null ? '' : String(content)
}

function normalizeConversationRow(c: Record<string, unknown>): Conversation | null {
  if (typeof c.id !== 'string' || typeof c.status !== 'string' || typeof c.priority !== 'string') {
    return null
  }
  return {
    ...c,
    id: c.id,
    status: c.status,
    priority: c.priority,
    lastMessage: lastMessagePreview(c.lastMessage),
  }
}

export function normalizeConversations(raw: unknown): Conversation[] {
  return listPayload(raw)
    .filter(isRecord)
    .map(normalizeConversationRow)
    .filter((c): c is Conversation => c !== null)
}

function responseTotal(resp: unknown, fallback: number): number {
  if (!isRecord(resp)) return fallback
  const pagination = isRecord(resp.pagination) ? resp.pagination : undefined
  return finiteNumber(pagination?.total) ?? finiteNumber(resp.total) ?? fallback
}

export async function loadConversations(page = 1, force = false): Promise<void> {
  const current = conversationsStore.get()
  if (!force && page === current.page && conversationsStore.isFresh(FRESH_MS)) return
  conversationsStore.update((s) => ({ ...s, busy: true, error: null }))
  const resp = await get<unknown>(`/api/conversations?page=${page}`)
  if (resp.success && resp.data !== undefined) {
    const items = normalizeConversations(resp.data)
    const total = responseTotal(resp, items.length)
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

/// Assign a conversation to a team. Optimistically reflects the new team and
/// 'assigned' status, reverting if the server refuses (CRD §3.2 routing).
export function assignConversation(id: string, teamId: number, reason?: string): Promise<boolean> {
  return conversationsStore.optimistic(
    (s) => ({
      ...s,
      items: s.items.map((c) =>
        c.id === id ? { ...c, teamId, status: 'assigned' } : c,
      ),
    }),
    () => post(`/api/conversations/${id}/assign`, { teamId, reason }),
  )
}

/// Deprecated compatibility guard: conversations are routed only to teams.
/// Keep this local rejection so old UI/client code cannot silently reintroduce
/// individual-agent assignment semantics.
export async function assignConversationToAgent(_id: string, _agentId: string): Promise<boolean> {
  return false
}

/// Transfer a conversation from its current team to another.
export function transferConversation(
  id: string,
  toTeamId: number,
  fromTeamId?: number | null,
  reason?: string,
): Promise<boolean> {
  return conversationsStore.optimistic(
    (s) => ({
      ...s,
      items: s.items.map((c) => (c.id === id ? { ...c, teamId: toTeamId, status: 'active' } : c)),
    }),
    () => post(`/api/conversations/${id}/transfer`, { toTeamId, fromTeamId, reason }),
  )
}

/// Remove the team assignment, returning the conversation to the unassigned pool.
export function unassignConversation(id: string, reason?: string): Promise<boolean> {
  return conversationsStore.optimistic(
    (s) => ({
      ...s,
      items: s.items.map((c) => (c.id === id ? { ...c, teamId: null, status: 'active' } : c)),
    }),
    () => post(`/api/conversations/${id}/unassign`, { reason }),
  )
}

export type BulkOperation = 'assign' | 'set_priority' | 'add_tags' | 'remove_tags'

/// Apply a bulk operation to many conversations, then refresh the list from the
/// server (the response is a summary, not the updated rows).
export async function bulkConversations(
  conversationIds: string[],
  operation: BulkOperation,
  data: Record<string, unknown>,
): Promise<boolean> {
  const resp = await post('/api/conversations/bulk', { operation, conversationIds, data })
  if (resp.success) await loadConversations(conversationsStore.get().page, true)
  return resp.success
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
