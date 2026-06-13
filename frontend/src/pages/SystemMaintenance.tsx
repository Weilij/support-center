// System maintenance (Phase 4, admin): consolidates the data-optimization,
// system-status, and user-experience ops endpoints. Read-only metrics plus a
// few maintenance actions (cleanup, rebuild indexes).

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { MetricsView } from '../components/MetricsView'
import { Toast } from '../components/ui'

type Tab = 'optimization' | 'status' | 'experience'

const TABS: { key: Tab; label: string; endpoint: string }[] = [
  { key: 'optimization', label: '資料優化', endpoint: '/api/data-optimization/stats' },
  { key: 'status', label: '系統狀態', endpoint: '/api/system/api-status' },
  { key: 'experience', label: '使用者體驗', endpoint: '/api/user-experience/report' },
]

export default function SystemMaintenance() {
  const [tab, setTab] = useState<Tab>('optimization')
  const [data, setData] = useState<unknown>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)

  const active = TABS.find((t) => t.key === tab)!

  const load = async () => {
    setBusy(true)
    setError(null)
    const resp = await get<unknown>(active.endpoint)
    if (resp.success) setData(resp.data ?? null)
    else setError(resp.message ?? '載入失敗')
    setBusy(false)
  }

  useEffect(() => {
    void load()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab])

  const runAction = async (endpoint: string, label: string) => {
    const resp = await post(endpoint, {})
    setToast(resp.success ? `${label}完成` : resp.message ?? `${label}失敗`)
    if (resp.success && tab === 'optimization') void load()
  }

  if (!can(session.position(), 'system')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  return (
    <main style={{ maxWidth: 980, margin: '4vh auto', padding: '0 16px' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <h1 style={{ margin: 0 }}>系統維護</h1>
        <button onClick={() => void load()} style={{ marginLeft: 'auto' }} disabled={busy}>
          {busy ? '更新中…' : '重新整理'}
        </button>
      </div>

      <div style={{ display: 'flex', gap: 8, borderBottom: '1px solid #ddd', margin: '12px 0 16px' }}>
        {TABS.map((t) => (
          <button
            key={t.key}
            onClick={() => setTab(t.key)}
            style={{
              border: 'none',
              background: 'none',
              padding: '8px 12px',
              borderBottom: tab === t.key ? '2px solid #3B82F6' : '2px solid transparent',
              fontWeight: tab === t.key ? 700 : 400,
              cursor: 'pointer',
            }}
          >
            {t.label}
          </button>
        ))}
      </div>

      {tab === 'optimization' && (
        <div style={{ display: 'flex', gap: 8, marginBottom: 14 }}>
          <button onClick={() => void runAction('/api/data-optimization/cleanup', '清理')}>執行清理</button>
          <button onClick={() => void runAction('/api/data-optimization/indexes', '索引重建')}>重建索引</button>
        </div>
      )}

      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      {!busy && <MetricsView data={data} />}

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </main>
  )
}
