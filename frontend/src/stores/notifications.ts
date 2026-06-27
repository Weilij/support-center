// Notification inbox container (CRD §8.1): unread badge, list with
// optimistic read-marking, realtime 'notification' fan-in.

import { get, put } from '../api/client'
import { onEvent } from '../realtime/client'
import { Store } from './store'

export interface Notification {
  id: string
  type?: string
  title?: string
  content?: string
  priority?: string
  isRead?: boolean
  createdAt?: string
  [key: string]: unknown
}

interface NotificationsState {
  items: Notification[]
  unread: number
  busy: boolean
  error: string | null
}

export const notificationsStore = new Store<NotificationsState>({
  items: [],
  unread: 0,
  busy: false,
  error: null,
})

function stringField(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function normalizeNotification(value: unknown): Notification | null {
  if (!isRecord(value) || typeof value.id !== 'string') return null
  return {
    ...value,
    id: value.id,
    type: stringField(value.type),
    title: stringField(value.title),
    content: stringField(value.content),
    priority: stringField(value.priority),
    isRead: typeof value.isRead === 'boolean' ? value.isRead : undefined,
    createdAt: stringField(value.createdAt),
  }
}

function normalizeNotifications(value: unknown): Notification[] {
  return Array.isArray(value)
    ? value.map(normalizeNotification).filter((item): item is Notification => item !== null)
    : []
}

export async function loadNotifications(): Promise<void> {
  notificationsStore.update((s) => ({ ...s, busy: true, error: null }))
  const [list, count] = await Promise.all([
    get<unknown>('/api/notifications'),
    get<{ count?: number }>('/api/notifications/unread-count'),
  ])
  if (list.success && isRecord(list.data)) {
    notificationsStore.set({
      items: normalizeNotifications(list.data.notifications),
      unread: count.success ? (count.data?.count ?? 0) : 0,
      busy: false,
      error: null,
    })
    notificationsStore.markFresh()
  } else {
    notificationsStore.update((s) => ({ ...s, busy: false, error: list.message ?? null }))
  }
}

export function markRead(id: string): Promise<boolean> {
  return notificationsStore.optimistic(
    (s) => ({
      ...s,
      unread: Math.max(0, s.unread - 1),
      items: s.items.map((n) => (n.id === id ? { ...n, isRead: true } : n)),
    }),
    () => put(`/api/notifications/${id}/read`),
  )
}

export function markAllRead(): Promise<boolean> {
  return notificationsStore.optimistic(
    (s) => ({ ...s, unread: 0, items: s.items.map((n) => ({ ...n, isRead: true })) }),
    () => put('/api/notifications/mark-all-read', {}),
  )
}

// Realtime fan-in: pushed notifications land at the top, unread (CRD §8.1).
onEvent('notification', (payload) => {
  notificationsStore.update((s) => ({
    ...s,
    unread: s.unread + 1,
    items: [{
      id: String(payload.id ?? crypto.randomUUID()),
      type: stringField(payload.type),
      title: stringField(payload.title),
      content: stringField(payload.content),
      priority: stringField(payload.priority),
      isRead: false,
      createdAt: stringField(payload.createdAt),
    }, ...s.items],
  }))
})
