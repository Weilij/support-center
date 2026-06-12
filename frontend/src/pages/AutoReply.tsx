// Auto-reply configuration screen (CRD §8.2, admin-flagged): rule listing
// with priority order, create keyword/welcome/fallback rules, soft delete.

import { useEffect, useState } from 'react'

import { get, post, del } from '../api/client'
import { session } from '../auth/session'

interface Rule {
  id: number
  name: string
  triggerType: string
  priority: number
  isActive: boolean
}

export default function AutoReply() {
  const [rules, setRules] = useState<Rule[]>([])
  const [name, setName] = useState('')
  const [trigger, setTrigger] = useState('keyword')
  const [keyword, setKeyword] = useState('')
  const [reply, setReply] = useState('')
  const [error, setError] = useState<string | null>(null)

  if (!session.isAdmin()) {
    return <main style={{ margin: '10vh auto', maxWidth: 480 }}><p>權限不足</p></main>
  }

  const load = async () => {
    const resp = await get<{ items?: Rule[] }>('/api/auto-reply/rules')
    if (resp.success && resp.data) setRules(resp.data.items ?? [])
    else setError(resp.message ?? null)
  }
  useEffect(() => {
    void load()
  }, [])

  const create = async (e: React.FormEvent) => {
    e.preventDefault()
    const body: Record<string, unknown> = {
      name,
      triggerType: trigger,
      actions: [{ actionType: 'text', content: JSON.stringify({ text: reply }) }],
    }
    if (trigger === 'keyword' && keyword.trim()) {
      body.conditions = [{ conditionType: 'contains', value: keyword.trim() }]
    }
    const resp = await post('/api/auto-reply/rules', body)
    if (resp.success) {
      setName(''); setKeyword(''); setReply('')
      void load()
    } else {
      setError(resp.message ?? null)
    }
  }

  const remove = async (id: number) => {
    const resp = await del(`/api/auto-reply/rules/${id}`)
    if (resp.success) void load()
    else setError(resp.message ?? null)
  }

  return (
    <main style={{ maxWidth: 720, margin: '5vh auto' }}>
      <h1>自動回覆</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <form onSubmit={create} style={{ display: 'grid', gap: 8, marginBottom: 16 }}>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="規則名稱" required />
        <select value={trigger} onChange={(e) => setTrigger(e.target.value)}>
          <option value="keyword">關鍵字</option>
          <option value="welcome">歡迎訊息</option>
          <option value="off_hours">非營業時間</option>
          <option value="fallback">預設回覆</option>
        </select>
        {trigger === 'keyword' && (
          <input value={keyword} onChange={(e) => setKeyword(e.target.value)} placeholder="關鍵字（包含比對）" />
        )}
        <input value={reply} onChange={(e) => setReply(e.target.value)} placeholder="回覆內容" required />
        <button type="submit">新增規則</button>
      </form>
      <ul style={{ listStyle: 'none', padding: 0 }}>
        {rules.map((r) => (
          <li key={r.id} style={{ display: 'flex', gap: 8, padding: 6, alignItems: 'center',
                                  borderBottom: '1px solid #f0f0f0' }}>
            <span style={{ color: '#999' }}>#{r.priority}</span>
            <strong>{r.name}</strong>
            <small>{r.triggerType}</small>
            {!r.isActive && <small style={{ color: 'orange' }}>停用</small>}
            <button onClick={() => void remove(r.id)} style={{ marginLeft: 'auto' }}>刪除</button>
          </li>
        ))}
      </ul>
    </main>
  )
}
