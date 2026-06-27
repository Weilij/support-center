import { beforeEach, describe, expect, it, vi } from 'vitest'

import { get, put } from '../api/client'
import { onEvent } from '../realtime/client'

vi.mock('../api/client', () => ({
  get: vi.fn(),
  put: vi.fn(),
}))

vi.mock('../realtime/client', () => ({
  onEvent: vi.fn(),
}))

const getMock = vi.mocked(get)
const putMock = vi.mocked(put)
const onEventMock = vi.mocked(onEvent)

async function importStore() {
  return import('../stores/notifications')
}

describe('notifications store', () => {
  beforeEach(() => {
    vi.resetModules()
    getMock.mockReset()
    putMock.mockReset()
    onEventMock.mockReset()
  })

  it('loads notification list and unread count together', async () => {
    getMock
      .mockResolvedValueOnce({
        success: true,
        data: {
          notifications: [
            { id: 'n1', title: 'First', isRead: false },
            { id: 42, title: 'invalid id' },
          ],
        },
      } as never)
      .mockResolvedValueOnce({ success: true, data: { count: 4 } } as never)
    const { loadNotifications, notificationsStore } = await importStore()

    await loadNotifications()

    expect(getMock).toHaveBeenNthCalledWith(1, '/api/notifications')
    expect(getMock).toHaveBeenNthCalledWith(2, '/api/notifications/unread-count')
    expect(notificationsStore.get()).toMatchObject({
      items: [{ id: 'n1', title: 'First', isRead: false }],
      unread: 4,
      busy: false,
      error: null,
    })
    expect(notificationsStore.isFresh(1000)).toBe(true)
  })

  it('rolls back optimistic mark-read changes when the backend rejects them', async () => {
    putMock.mockResolvedValue({ success: false, message: 'denied' } as never)
    const { markRead, notificationsStore } = await importStore()
    notificationsStore.set({
      items: [{ id: 'n1', title: 'First', isRead: false }],
      unread: 1,
      busy: false,
      error: null,
    })

    await expect(markRead('n1')).resolves.toBe(false)

    expect(putMock).toHaveBeenCalledWith('/api/notifications/n1/read')
    expect(notificationsStore.get()).toMatchObject({
      items: [{ id: 'n1', title: 'First', isRead: false }],
      unread: 1,
    })
  })

  it('marks all notifications read optimistically on success', async () => {
    putMock.mockResolvedValue({ success: true } as never)
    const { markAllRead, notificationsStore } = await importStore()
    notificationsStore.set({
      items: [
        { id: 'n1', isRead: false },
        { id: 'n2', isRead: false },
      ],
      unread: 2,
      busy: false,
      error: null,
    })

    await expect(markAllRead()).resolves.toBe(true)

    expect(putMock).toHaveBeenCalledWith('/api/notifications/mark-all-read', {})
    expect(notificationsStore.get().items.every((item) => item.isRead)).toBe(true)
    expect(notificationsStore.get().unread).toBe(0)
  })

  it('prepends realtime notifications and increments unread count', async () => {
    const { notificationsStore } = await importStore()
    const callback = onEventMock.mock.calls.find(([event]) => event === 'notification')?.[1]
    expect(callback).toBeTypeOf('function')
    notificationsStore.set({
      items: [{ id: 'old', title: 'Old', isRead: false }],
      unread: 1,
      busy: false,
      error: null,
    })

    callback?.({
      id: 'new',
      type: 'system',
      title: 'New',
      content: 'Body',
      priority: 'high',
      createdAt: '2026-01-01T00:00:00.000Z',
    })

    expect(notificationsStore.get()).toMatchObject({
      unread: 2,
      items: [
        {
          id: 'new',
          type: 'system',
          title: 'New',
          content: 'Body',
          priority: 'high',
          isRead: false,
          createdAt: '2026-01-01T00:00:00.000Z',
        },
        { id: 'old', title: 'Old', isRead: false },
      ],
    })
  })

  it('drops non-string realtime notification fields instead of trusting payload shape', async () => {
    const { notificationsStore } = await importStore()
    const callback = onEventMock.mock.calls.find(([event]) => event === 'notification')?.[1]
    notificationsStore.set({ items: [], unread: 0, busy: false, error: null })

    callback?.({
      id: 'n2',
      type: { nested: true },
      title: 123,
      content: null,
      priority: ['high'],
      createdAt: false,
    })

    expect(notificationsStore.get().items[0]).toEqual({
      id: 'n2',
      type: undefined,
      title: undefined,
      content: undefined,
      priority: undefined,
      isRead: false,
      createdAt: undefined,
    })
  })
})
