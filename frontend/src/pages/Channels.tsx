// Channel management screen (CRD §8.2, admin-flagged): connection list with
// status/health, verification trigger.

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { session } from '../auth/session'

interface Channel {
  id: number
  platform: string
  isActive?: boolean
  isVerified?: boolean
  errorCount?: number
}

export default function Channels() {
  const [channels, setChannels] = useState<Channel[]>([])
  const [error, setError] = useState<string | null>(null)
  const [message, setMessage] = useState<string | null>(null)

  const load = async () => {
    const resp = await get<Channel[]>('/api/channels')
    if (resp.success && Array.isArray(resp.data)) setChannels(resp.data)
    else if (resp.success) setChannels([])
    else setError(resp.message ?? null)
  }
  useEffect(() => {
    void load()
  }, [])

  const verify = async (id: number) => {
    setMessage(null); setError(null)
    const resp = await post(`/api/channels/${id}/verify`, {})
    if (resp.success) setMessage('驗證成功')
    else setError(resp.message ?? '驗證失敗')
    void load()
  }

  // Admin gate AFTER all hooks (Rules of Hooks: stable hook order).
  if (!session.isAdmin()) {
    return <main style={{ margin: '10vh auto', maxWidth: 480 }}><p>權限不足</p></main>
  }
  return (
    <main style={{ maxWidth: 720, margin: '5vh auto' }}>
      <h1>頻道管理</h1>
      {message && <p style={{ color: 'seagreen' }}>{message}</p>}
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      {channels.length === 0 && <p>尚未設定任何頻道連接。</p>}
      <ul style={{ listStyle: 'none', padding: 0 }}>
        {channels.map((c) => (
          <li key={c.id} style={{ display: 'flex', gap: 12, padding: 8, alignItems: 'center',
                                  borderBottom: '1px solid #f0f0f0' }}>
            <strong>{c.platform}</strong>
            <small>{c.isActive ? '啟用' : '停用'}</small>
            <small style={{ color: c.isVerified ? 'seagreen' : 'orange' }}>
              {c.isVerified ? '已驗證' : '未驗證'}
            </small>
            {(c.errorCount ?? 0) > 0 && <small style={{ color: 'crimson' }}>錯誤 {c.errorCount}</small>}
            <button onClick={() => void verify(c.id)} style={{ marginLeft: 'auto' }}>驗證</button>
          </li>
        ))}
      </ul>
    </main>
  )
}
