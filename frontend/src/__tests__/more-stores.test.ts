import { beforeEach, describe, expect, it, vi } from 'vitest'

import { download, get, post, del, upload } from '../api/client'
import { loadAnalyticsOverview } from '../stores/analytics'
import {
  loadCustomerDetail,
  loadCustomerTags,
  loadCustomers,
  customersStore,
} from '../stores/customers'
import { cancelDelayed, loadPendingDelayed, scheduleDelayed } from '../stores/delayedMessages'
import {
  fileDownloadUrl,
  loadConversationFiles,
  uploadConversationFile,
} from '../stores/files'
import { exportMessagesCsv, searchMessages } from '../stores/messages'
import { loadTeams, teamsStore } from '../stores/teams'

vi.mock('../api/client', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../api/client')>()
  return {
    ...actual,
    get: vi.fn(),
    post: vi.fn(),
    del: vi.fn(),
    upload: vi.fn(),
    download: vi.fn(),
  }
})

const getMock = vi.mocked(get)
const postMock = vi.mocked(post)
const delMock = vi.mocked(del)
const uploadMock = vi.mocked(upload)
const downloadMock = vi.mocked(download)

describe('lookup stores', () => {
  beforeEach(() => {
    getMock.mockReset()
    teamsStore.set({ items: [], busy: false, error: null })
    teamsStore.invalidate()
    customersStore.set({ items: [], busy: false, error: null })
    customersStore.invalidate()
  })

  it('loads teams once while the cache is fresh unless forced', async () => {
    getMock.mockResolvedValue({ success: true, data: { items: [{ id: 1, name: 'Support' }] } } as never)

    await loadTeams()
    await loadTeams()
    await loadTeams(true)

    expect(getMock).toHaveBeenCalledTimes(2)
    expect(getMock).toHaveBeenCalledWith('/api/teams')
    expect(teamsStore.get().items).toEqual([{ id: 1, name: 'Support' }])
  })

  it('loads customers, details, and tags from their endpoints', async () => {
    getMock
      .mockResolvedValueOnce({
        success: true,
        data: { customers: [{ id: 10, platform: 'line', platform_user_id: 'U1' }] },
      } as never)
      .mockResolvedValueOnce({
        success: true,
        data: {
          customer: { id: 10, platform: 'line', platform_user_id: 'U1' },
          conversations: [],
          conversationCount: 0,
        },
      } as never)
      .mockResolvedValueOnce({ success: true, data: [{ id: 1, name: 'VIP' }] } as never)

    await loadCustomers()
    await expect(loadCustomerDetail(10)).resolves.toMatchObject({ conversationCount: 0 })
    await expect(loadCustomerTags(10)).resolves.toEqual([{ id: 1, name: 'VIP' }])

    expect(customersStore.get().items).toEqual([{ id: 10, platform: 'line', platform_user_id: 'U1' }])
    expect(getMock).toHaveBeenNthCalledWith(1, '/api/customers')
    expect(getMock).toHaveBeenNthCalledWith(2, '/api/customers/10')
    expect(getMock).toHaveBeenNthCalledWith(3, '/api/customers/10/tags')
  })
})

describe('message-related stores', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    delMock.mockReset()
    uploadMock.mockReset()
    downloadMock.mockReset()
  })

  it('loads, schedules, and cancels delayed messages', async () => {
    getMock.mockResolvedValue({ success: true, data: { messages: [{ messageId: 'm1' }] } } as never)
    postMock.mockResolvedValue({ success: true, message: 'scheduled' } as never)
    delMock.mockResolvedValue({ success: true } as never)
    const input = {
      conversationId: 'conv-1',
      content: 'later',
      platform: 'line',
      userId: 'U1',
      delaySeconds: 30,
    }

    await expect(loadPendingDelayed('conv-1')).resolves.toEqual([{ messageId: 'm1' }])
    await expect(scheduleDelayed(input)).resolves.toEqual({ ok: true, message: 'scheduled' })
    await expect(cancelDelayed('m1')).resolves.toBe(true)

    expect(getMock).toHaveBeenCalledWith('/api/delayed-messages-v2/pending?conversationId=conv-1')
    expect(postMock).toHaveBeenCalledWith('/api/delayed-messages-v2/send', input)
    expect(delMock).toHaveBeenCalledWith('/api/delayed-messages-v2/cancel/m1')
  })

  it('loads, uploads, and resolves file download urls', async () => {
    const file = new File(['hello'], 'hello.txt', { type: 'text/plain' })
    getMock
      .mockResolvedValueOnce({
        success: true,
        data: [{ id: 'file-1', filename: 'a.txt' }, { id: 42, filename: 'bad.txt' }],
      } as never)
      .mockResolvedValueOnce({ success: true, data: { url: '/signed/file-1' } } as never)
    uploadMock.mockResolvedValue({ success: true, data: { id: 'file-2', filename: 'hello.txt' } } as never)

    await expect(loadConversationFiles('conv-1')).resolves.toEqual([
      {
        id: 'file-1',
        filename: 'a.txt',
        originalName: undefined,
        contentType: undefined,
        size: undefined,
        url: undefined,
        publicUrl: undefined,
        conversationId: undefined,
        uploadStatus: undefined,
        createdAt: undefined,
      },
    ])
    await expect(uploadConversationFile('conv-1', file)).resolves.toEqual({
      attachment: { id: 'file-2', filename: 'hello.txt' },
    })
    await expect(fileDownloadUrl('file-1')).resolves.toBe('/signed/file-1')

    const uploadedForm = uploadMock.mock.calls[0][1] as FormData
    expect(uploadMock).toHaveBeenCalledWith('/api/files/upload/admin', expect.any(FormData))
    expect(uploadedForm.get('conversationId')).toBe('conv-1')
    expect(getMock).toHaveBeenNthCalledWith(1, '/api/files/conversation/conv-1')
    expect(getMock).toHaveBeenNthCalledWith(2, '/api/files/file-1/download-url')
  })

  it('searches and exports messages with query filters', async () => {
    getMock.mockResolvedValue({
      success: true,
      data: { messages: [{ id: 'm1', conversationId: 'conv-1' }], total: 12, pagination: { hasMore: true } },
    } as never)
    downloadMock.mockResolvedValue({ ok: true })

    await expect(searchMessages({ q: 'hello world', limit: 25, offset: 50 })).resolves.toEqual({
      messages: [{ id: 'm1', conversationId: 'conv-1' }],
      total: 12,
      hasMore: true,
    })
    await expect(exportMessagesCsv({ q: 'hello world', limit: 25, offset: 50 })).resolves.toEqual({ ok: true })

    expect(getMock).toHaveBeenCalledWith('/api/messages/search?q=hello+world&limit=25&offset=50')
    expect(downloadMock).toHaveBeenCalledWith(
      'GET',
      '/api/messages/export?q=hello+world&format=csv',
      undefined,
      'messages_export.csv',
    )
  })
})

describe('analytics overview store', () => {
  beforeEach(() => {
    getMock.mockReset()
  })

  it('combines summaries from the four analytics endpoints', async () => {
    getMock
      .mockResolvedValueOnce({ success: true, data: { data: { summary: { open: 3 } } } } as never)
      .mockResolvedValueOnce({ success: true, data: { data: { summary: { sent: 9 } } } } as never)
      .mockResolvedValueOnce({
        success: true,
        data: {
          data: {
            summary: { active: 2 },
            topPerformers: [
              { userId: 'agent-1' },
              null,
              'bad',
              { userId: 123, displayName: 'Missing id', conversationsHandled: Number.NaN },
            ],
          },
        },
      } as never)
      .mockResolvedValueOnce({ success: true, data: { data: { summary: { p95: 120 } } } } as never)

    await expect(loadAnalyticsOverview('30d')).resolves.toEqual({
      conversations: { open: 3 },
      messages: { sent: 9 },
      users: { active: 2 },
      performance: { p95: 120 },
      topPerformers: [
        { userId: 'agent-1', displayName: undefined, conversationsHandled: undefined },
        { userId: undefined, displayName: 'Missing id', conversationsHandled: undefined },
      ],
    })

    expect(getMock).toHaveBeenCalledWith('/api/analytics/conversations?timeRange=30d')
    expect(getMock).toHaveBeenCalledWith('/api/analytics/messages?timeRange=30d')
    expect(getMock).toHaveBeenCalledWith('/api/analytics/users?timeRange=30d')
    expect(getMock).toHaveBeenCalledWith('/api/analytics/performance?timeRange=30d')
  })
})
