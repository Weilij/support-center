// Tags container (CRD §8.1): shared tag list cache plus create/delete actions
// for the Tags screen and conversation-tag controls.

import { del, get, post } from '../api/client'
import { Store } from './store'

export interface Tag {
  id: number
  name: string
  color?: string
  isActive?: boolean
  [key: string]: unknown
}

interface TagsState {
  items: Tag[]
  busy: boolean
  error: string | null
}

const FRESH_MS = 60_000

export const tagsStore = new Store<TagsState>({
  items: [],
  busy: false,
  error: null,
})

function readTags(data: unknown): Tag[] {
  if (Array.isArray(data)) return data as Tag[]
  if (typeof data !== 'object' || data === null) return []
  const record = data as { items?: unknown; tags?: unknown }
  if (Array.isArray(record.items)) return record.items as Tag[]
  if (Array.isArray(record.tags)) return record.tags as Tag[]
  return []
}

export async function loadTags(force = false): Promise<void> {
  if (!force && tagsStore.isFresh(FRESH_MS) && tagsStore.get().items.length > 0) return
  tagsStore.update((s) => ({ ...s, busy: true, error: null }))
  const resp = await get<unknown>('/api/tags')
  if (resp.success && resp.data !== undefined) {
    tagsStore.set({ items: readTags(resp.data), busy: false, error: null })
    tagsStore.markFresh()
  } else {
    tagsStore.update((s) => ({ ...s, busy: false, error: resp.message ?? '載入失敗' }))
  }
}

export async function createTag(name: string): Promise<boolean> {
  const trimmed = name.trim()
  if (!trimmed) {
    tagsStore.update((s) => ({ ...s, error: 'Tag name is required' }))
    return false
  }
  const resp = await post('/api/tags', { name: trimmed })
  if (!resp.success) {
    tagsStore.update((s) => ({ ...s, error: resp.message ?? '新增失敗' }))
    return false
  }
  await loadTags(true)
  return true
}

export async function deleteTag(id: number): Promise<boolean> {
  const resp = await del(`/api/tags/${id}`)
  if (!resp.success) {
    tagsStore.update((s) => ({ ...s, error: resp.message ?? '刪除失敗' }))
    return false
  }
  await loadTags(true)
  return true
}
