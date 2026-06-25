import { beforeEach, describe, expect, it, vi } from 'vitest'

import { del, get, post } from '../api/client'
import { createTag, deleteTag, loadTags, tagsStore } from '../stores/tags'

vi.mock('../api/client', () => ({
  get: vi.fn(),
  post: vi.fn(),
  del: vi.fn(),
}))

const getMock = vi.mocked(get)
const postMock = vi.mocked(post)
const delMock = vi.mocked(del)

describe('tags store', () => {
  beforeEach(() => {
    getMock.mockReset()
    postMock.mockReset()
    delMock.mockReset()
    tagsStore.set({ items: [], busy: false, error: null })
    tagsStore.invalidate()
  })

  it('loads tags from supported response containers', async () => {
    getMock.mockResolvedValue({
      success: true,
      data: { tags: [{ id: 1, name: 'VIP', color: '#0ea5e9' }] },
    } as never)

    await loadTags()

    expect(tagsStore.get()).toMatchObject({
      items: [{ id: 1, name: 'VIP', color: '#0ea5e9' }],
      busy: false,
      error: null,
    })
  })

  it('creates and deletes tags through the backend then refreshes the list', async () => {
    getMock
      .mockResolvedValueOnce({ success: true, data: { items: [] } } as never)
      .mockResolvedValueOnce({ success: true, data: { items: [{ id: 2, name: 'New' }] } } as never)
      .mockResolvedValueOnce({ success: true, data: { items: [] } } as never)
    postMock.mockResolvedValue({ success: true } as never)
    delMock.mockResolvedValue({ success: true } as never)

    await loadTags()
    await expect(createTag(' New ')).resolves.toBe(true)
    await expect(deleteTag(2)).resolves.toBe(true)

    expect(postMock).toHaveBeenCalledWith('/api/tags', { name: 'New' })
    expect(delMock).toHaveBeenCalledWith('/api/tags/2')
    expect(getMock).toHaveBeenCalledTimes(3)
  })

  it('rejects blank tag names locally', async () => {
    await expect(createTag('   ')).resolves.toBe(false)
    expect(postMock).not.toHaveBeenCalled()
    expect(tagsStore.get().error).toBe('Tag name is required')
  })
})
