// Analytics overview (Phase 3.1): headline metrics for conversations, messages,
// users and system performance over a selectable time range, plus a top
// performers list.

import { useEffect, useState } from 'react'

import { StatCard } from '../components/ui'
import { DataTable } from '../components/DataTable'
import { PageHeader } from '../components/PageHeader'
import { Card, StatGrid } from '../components/Card'
import { loadAnalyticsOverview, type CoreSummaries, type TopPerformer } from '../stores/analytics'
import type { Column } from '../components/DataTable'
import { can } from '../auth/permissions'
import { session } from '../auth/session'

const RANGES = [
  { value: '7d', label: '近 7 天' },
  { value: '30d', label: '近 30 天' },
  { value: '90d', label: '近 90 天' },
]

function num(v: unknown): string {
  if (v == null) return '—'
  if (typeof v === 'number') return v.toLocaleString()
  return String(v)
}

export default function Analytics() {
  const [range, setRange] = useState('7d')
  const [data, setData] = useState<CoreSummaries>({})
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    setBusy(true)
    void loadAnalyticsOverview(range).then((d) => {
      setData(d)
      setBusy(false)
    })
  }, [range])

  if (!can(session.position(), 'analytics')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const c = data.conversations ?? {}
  const m = data.messages ?? {}
  const u = data.users ?? {}
  const p = data.performance ?? {}

  const performerColumns: Column<TopPerformer>[] = [
    { key: 'displayName', header: '客服', render: (t) => t.displayName || t.userId || '—' },
    { key: 'conversationsHandled', header: '處理對話數', align: 'right', render: (t) => num(t.conversationsHandled) },
  ]

  const rangeSelect = (
    <select
      value={range}
      onChange={(e) => setRange(e.target.value)}
      style={{ padding: '6px 8px', borderRadius: 6, border: '1px solid #ccc' }}
    >
      {RANGES.map((r) => (
        <option key={r.value} value={r.value}>
          {r.label}
        </option>
      ))}
    </select>
  )

  return (
    <div style={{ maxWidth: 1040, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="數據分析" actions={rangeSelect} />
      {busy && <p style={{ color: 'var(--muted)' }}>載入中…</p>}

      <Card title="對話" style={{ marginBottom: 'var(--sp-4)' }}>
        <StatGrid>
          <StatCard label="總對話數" value={num(c.totalConversations)} />
          <StatCard label="進行中" value={num(c.activeConversations)} />
          <StatCard label="已結束" value={num(c.closedConversations)} />
          <StatCard label="平均每對話訊息" value={num(c.averageMessagesPerConversation)} />
        </StatGrid>
      </Card>

      <Card title="訊息" style={{ marginBottom: 'var(--sp-4)' }}>
        <StatGrid>
          <StatCard label="總訊息數" value={num(m.totalMessages)} />
          <StatCard label="每小時訊息" value={num(m.messagesPerHour)} />
          <StatCard label="平均回應(分)" value={num(m.averageResponseMinutes)} />
        </StatGrid>
      </Card>

      <Card title="使用者" style={{ marginBottom: 'var(--sp-4)' }}>
        <StatGrid>
          <StatCard label="總使用者" value={num(u.totalUsers)} />
          <StatCard label="活躍使用者" value={num(u.activeUsers)} />
          <StatCard label="平均工作階段(分)" value={num(u.averageSessionMinutes)} />
        </StatGrid>
      </Card>

      <Card title="系統效能" style={{ marginBottom: 'var(--sp-4)' }}>
        <StatGrid>
          <StatCard label="平均回應(ms)" value={num(p.averageResponseTimeMs)} />
          <StatCard label="吞吐(rps)" value={num(p.throughputRps)} />
          <StatCard label="錯誤率(%)" value={num(p.errorRatePercent)} />
          <StatCard label="可用率(%)" value={num(p.uptimePercent)} />
        </StatGrid>
      </Card>

      <Card title="頂尖客服">
        <DataTable
          columns={performerColumns}
          rows={data.topPerformers ?? []}
          rowKey={(t) => t.userId ?? t.displayName ?? ''}
          empty="尚無資料"
        />
      </Card>
    </div>
  )
}
