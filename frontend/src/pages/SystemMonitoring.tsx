// System monitoring (Phase 4, admin): read-only ops dashboards consolidated
// into tabs — service monitoring, health, queues, and the security overview.
// Each tab fetches its stats endpoint and renders via the generic MetricsView.

import { useEffect, useState } from 'react'

import { get } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { MetricsView } from '../components/MetricsView'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'

type Tab = 'monitoring' | 'health' | 'queues' | 'security'

const TABS: { key: Tab; label: string; endpoint: string }[] = [
  { key: 'monitoring', label: '監控', endpoint: '/api/monitoring/dashboard' },
  { key: 'health', label: '健康', endpoint: '/api/health/status' },
  { key: 'queues', label: '佇列', endpoint: '/api/queues/stats' },
  { key: 'security', label: '安全', endpoint: '/api/security/dashboard/summary' },
]

export default function SystemMonitoring() {
  const [tab, setTab] = useState<Tab>('monitoring')
  const [data, setData] = useState<unknown>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

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

  if (!can(session.position(), 'system')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const refreshBtn = (
    <button onClick={() => void load()} disabled={busy}>
      {busy ? '更新中…' : '重新整理'}
    </button>
  )

  return (
    <div style={{ maxWidth: 980, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="系統監控" actions={refreshBtn} />

      <Card>
        <div style={{ display: 'flex', gap: 8, borderBottom: '1px solid var(--hairline)', marginBottom: 'var(--sp-4)' }}>
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

        {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
        {!busy && <MetricsView data={data} />}
      </Card>
    </div>
  )
}
