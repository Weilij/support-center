// Activity log (admin): audit listing plus an overview panel (totals, action
// breakdown, most-active users) and per-entry restore for soft-deleted
// resources (Phase 3.5).

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { DataTable } from '../components/DataTable'
import { StatCard, Toast } from '../components/ui'
import type { Column } from '../components/DataTable'

interface Activity {
  id: number
  agentName?: string
  action?: string
  resourceType?: string
  resourceId?: string
  createdAt?: string
}

interface Overview {
  totalActivities?: number
  actionBreakdown?: Record<string, number>
  topUsers?: { userName?: string; userRole?: string; count?: number }[]
}

export default function ActivityLog() {
  const [items, setItems] = useState<Activity[]>([])
  const [overview, setOverview] = useState<Overview>({})
  const [error, setError] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const loadList = async () => {
    setBusy(true)
    const resp = await get<{ items?: Activity[]; activities?: Activity[] }>('/api/activities')
    if (resp.success && resp.data) setItems(resp.data.items ?? resp.data.activities ?? [])
    else setError(resp.message ?? null)
    setBusy(false)
  }

  useEffect(() => {
    void loadList()
    void get<Overview>('/api/activities/overview').then((r) => r.success && r.data && setOverview(r.data))
  }, [])

  const restore = async (id: number) => {
    const resp = await post(`/api/activities/${id}/restore`, {})
    setToast(resp.success ? '已還原' : resp.message ?? '無法還原')
    if (resp.success) void loadList()
  }

  if (!can(session.position(), 'analytics')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const actionEntries = Object.entries(overview.actionBreakdown ?? {})
    .sort((a, b) => b[1] - a[1])
    .slice(0, 6)

  const columns: Column<Activity>[] = [
    { key: 'createdAt', header: '時間', width: 160, render: (a) => (a.createdAt ? new Date(a.createdAt).toLocaleString() : '—') },
    { key: 'agentName', header: '操作者', render: (a) => a.agentName || '—' },
    { key: 'action', header: '動作', render: (a) => a.action || '—' },
    {
      key: 'resource',
      header: '資源',
      render: (a) => `${a.resourceType ?? ''}${a.resourceId ? `#${a.resourceId}` : ''}` || '—',
    },
    {
      key: 'restore',
      header: '',
      width: 70,
      render: (a) =>
        a.action?.includes('delete') ? <button onClick={() => void restore(a.id)}>還原</button> : null,
    },
  ]

  return (
    <main style={{ maxWidth: 920, margin: '4vh auto', padding: '0 16px' }}>
      <h1>活動日誌</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}

      <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap', margin: '12px 0' }}>
        <StatCard label="總活動數" value={overview.totalActivities ?? 0} />
        {actionEntries.map(([action, count]) => (
          <StatCard key={action} label={action} value={count} />
        ))}
      </div>

      {(overview.topUsers ?? []).length > 0 && (
        <p style={{ fontSize: 14, color: '#555' }}>
          最活躍：
          {(overview.topUsers ?? [])
            .slice(0, 5)
            .map((u) => `${u.userName ?? '?'} (${u.count ?? 0})`)
            .join('、')}
        </p>
      )}

      <DataTable columns={columns} rows={items} rowKey={(a) => a.id} busy={busy} empty="沒有活動紀錄" />

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </main>
  )
}
