// Activity log screen (CRD §8.2, admin-flagged): filtered audit listing.

import { useEffect, useState } from 'react'

import { get } from '../api/client'
import { session } from '../auth/session'

interface Activity {
  id: number
  agentName?: string
  action?: string
  resourceType?: string
  resourceId?: string
  createdAt?: string
}

export default function ActivityLog() {
  const [items, setItems] = useState<Activity[]>([])
  const [error, setError] = useState<string | null>(null)

  if (!session.isAdmin()) {
    return <main style={{ margin: '10vh auto', maxWidth: 480 }}><p>權限不足</p></main>
  }

  useEffect(() => {
    void get<{ items?: Activity[]; activities?: Activity[] }>('/api/activities').then((resp) => {
      if (resp.success && resp.data) {
        setItems(resp.data.items ?? resp.data.activities ?? [])
      } else {
        setError(resp.message ?? null)
      }
    })
  }, [])

  return (
    <main style={{ maxWidth: 860, margin: '5vh auto' }}>
      <h1>活動日誌</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <table style={{ width: '100%', borderCollapse: 'collapse' }}>
        <thead>
          <tr style={{ textAlign: 'left', borderBottom: '1px solid #ddd' }}>
            <th>時間</th><th>操作者</th><th>動作</th><th>資源</th>
          </tr>
        </thead>
        <tbody>
          {items.map((a) => (
            <tr key={a.id} style={{ borderBottom: '1px solid #f0f0f0' }}>
              <td><small>{a.createdAt}</small></td>
              <td>{a.agentName}</td>
              <td>{a.action}</td>
              <td>{a.resourceType}{a.resourceId ? `#${a.resourceId}` : ''}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </main>
  )
}
