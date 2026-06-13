// Analytics overview (Phase 3.1): headline metrics for conversations, messages,
// users and system performance over a selectable time range, plus a top
// performers list.

import { useEffect, useState } from 'react'

import { StatCard } from '../components/ui'
import { DataTable } from '../components/DataTable'
import { loadAnalyticsOverview, type CoreSummaries, type TopPerformer } from '../stores/analytics'
import type { Column } from '../components/DataTable'

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

  const c = data.conversations ?? {}
  const m = data.messages ?? {}
  const u = data.users ?? {}
  const p = data.performance ?? {}

  const performerColumns: Column<TopPerformer>[] = [
    { key: 'displayName', header: '客服', render: (t) => t.displayName || t.userId || '—' },
    { key: 'conversationsHandled', header: '處理對話數', align: 'right', render: (t) => num(t.conversationsHandled) },
  ]

  return (
    <main style={{ maxWidth: 1040, margin: '4vh auto', padding: '0 16px' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <h1 style={{ margin: 0 }}>數據分析</h1>
        <select
          value={range}
          onChange={(e) => setRange(e.target.value)}
          style={{ marginLeft: 'auto', padding: '6px 8px', borderRadius: 6, border: '1px solid #ccc' }}
        >
          {RANGES.map((r) => (
            <option key={r.value} value={r.value}>
              {r.label}
            </option>
          ))}
        </select>
      </div>
      {busy && <p style={{ color: '#888' }}>載入中…</p>}

      <Section title="對話">
        <StatCard label="總對話數" value={num(c.totalConversations)} />
        <StatCard label="進行中" value={num(c.activeConversations)} />
        <StatCard label="已結束" value={num(c.closedConversations)} />
        <StatCard label="平均每對話訊息" value={num(c.averageMessagesPerConversation)} />
      </Section>

      <Section title="訊息">
        <StatCard label="總訊息數" value={num(m.totalMessages)} />
        <StatCard label="每小時訊息" value={num(m.messagesPerHour)} />
        <StatCard label="平均回應(分)" value={num(m.averageResponseMinutes)} />
      </Section>

      <Section title="使用者">
        <StatCard label="總使用者" value={num(u.totalUsers)} />
        <StatCard label="活躍使用者" value={num(u.activeUsers)} />
        <StatCard label="平均工作階段(分)" value={num(u.averageSessionMinutes)} />
      </Section>

      <Section title="系統效能">
        <StatCard label="平均回應(ms)" value={num(p.averageResponseTimeMs)} />
        <StatCard label="吞吐(rps)" value={num(p.throughputRps)} />
        <StatCard label="錯誤率(%)" value={num(p.errorRatePercent)} />
        <StatCard label="可用率(%)" value={num(p.uptimePercent)} />
      </Section>

      <h3 style={{ margin: '20px 0 8px' }}>頂尖客服</h3>
      <DataTable
        columns={performerColumns}
        rows={data.topPerformers ?? []}
        rowKey={(t) => t.userId ?? t.displayName ?? ''}
        empty="尚無資料"
      />
    </main>
  )
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ margin: '16px 0' }}>
      <h3 style={{ margin: '0 0 8px' }}>{title}</h3>
      <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap' }}>{children}</div>
    </section>
  )
}
