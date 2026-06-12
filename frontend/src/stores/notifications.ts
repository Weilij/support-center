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

export async function loadNotifications(): Promise<void> {
  notificationsStore.update((s) => ({ ...s, busy: true, error: null }))
  const [list, count] = await Promise.all([
    get<{ notifications?: Notification[] }>('/api/notifications'),
    get<{ count?: number }>('/api/notifications/unread-count'),
  ])
  if (list.success && list.data) {
    notificationsStore.set({
      items: list.data.notifications ?? [],
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
      type: payload.type as string | undefined,
      title: payload.title as string | undefined,
      content: payload.content as string | undefined,
      priority: payload.priority as string | undefined,
      isRead: false,
      createdAt: payload.createdAt as string | undefined,
    }, ...s.items],
  }))
})
