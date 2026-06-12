// Tag management screen (CRD §8.2): list + create + soft delete.

import { useEffect, useState } from 'react'

import { get, post, del } from '../api/client'

interface Tag {
  id: number
  name: string
  color?: string
  isActive?: boolean
}

export default function Tags() {
  const [tags, setTags] = useState<Tag[]>([])
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    const resp = await get<{ items?: Tag[]; tags?: Tag[] }>('/api/tags')
    if (resp.success && resp.data) {
      setTags(resp.data.items ?? resp.data.tags ?? [])
    } else {
      setError(resp.message ?? null)
    }
  }
  useEffect(() => {
    void load()
  }, [])

  const create = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!name.trim()) return
    const resp = await post('/api/tags', { name: name.trim() })
    if (resp.success) {
      setName('')
      void load()
    } else {
      setError(resp.message ?? null)
    }
  }

  const remove = async (id: number) => {
    const resp = await del(`/api/tags/${id}`)
    if (resp.success) void load()
    else setError(resp.message ?? null)
  }

  return (
    <main style={{ maxWidth: 600, margin: '5vh auto' }}>
      <h1>標籤管理</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <form onSubmit={create} style={{ display: 'flex', gap: 8 }}>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="新標籤名稱" />
        <button type="submit">新增</button>
      </form>
      <ul style={{ listStyle: 'none', padding: 0 }}>
        {tags.map((tag) => (
          <li key={tag.id} style={{ display: 'flex', gap: 8, padding: 6, alignItems: 'center' }}>
            <span style={{
              background: tag.color ?? '#3B82F6', color: 'white',
              borderRadius: 8, padding: '2px 10px',
            }}>
              {tag.name}
            </span>
            <button onClick={() => void remove(tag.id)} style={{ marginLeft: 'auto' }}>刪除</button>
          </li>
        ))}
      </ul>
    </main>
  )
}
