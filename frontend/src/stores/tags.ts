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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function normalizeTag(value: unknown): Tag | null {
  if (!isRecord(value)) return null
  const id = value.id
  const name = value.name
  if (typeof id !== 'number' || !Number.isFinite(id) || typeof name !== 'string') return null
  return {
    ...value,
    id,
    name,
    color: typeof value.color === 'string' ? value.color : undefined,
    isActive: typeof value.isActive === 'boolean' ? value.isActive : undefined,
  }
}

function normalizeTags(value: unknown): Tag[] {
  return Array.isArray(value) ? value.map(normalizeTag).filter((tag): tag is Tag => tag !== null) : []
}

function readTags(data: unknown): Tag[] {
  if (Array.isArray(data)) return normalizeTags(data)
  if (!isRecord(data)) return []
  if (Array.isArray(data.items)) return normalizeTags(data.items)
  if (Array.isArray(data.tags)) return normalizeTags(data.tags)
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
