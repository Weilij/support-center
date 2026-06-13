// LIFF settings (Phase 2.6, admin): read-only LIFF/LINE config plus the team
// join-QR coverage report with a one-click batch generate for teams missing a
// LIFF QR. (Customer-facing LIFF flows — assign-team/welcome — are not admin UI.)

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { session } from '../auth/session'
import { DataTable } from '../components/DataTable'
import { StatCard, StatusPill, Toast } from '../components/ui'
import type { Column } from '../components/DataTable'

interface LiffConfig {
  liffId?: string
  lineBotId?: string
  lineOaId?: string
  apiEndpoint?: string
  version?: string
}

interface CoverageTeam {
  id: number
  name: string
  hasLiffQR: boolean
}

interface Coverage {
  totalTeams?: number
  teamsWithLiffQR?: number
  teamsWithoutLiffQR?: number
  coverage?: string
  teams?: CoverageTeam[]
}

export default function LiffSettings() {
  const [config, setConfig] = useState<LiffConfig>({})
  const [coverage, setCoverage] = useState<Coverage>({})
  const [busy, setBusy] = useState(false)
  const [toast, setToast] = useState<string | null>(null)

  const loadCoverage = async () => {
    const resp = await get<Coverage>('/api/admin/liff-qr/status')
    if (resp.success && resp.data) setCoverage(resp.data)
  }

  useEffect(() => {
    void get<LiffConfig>('/api/liff/config').then((resp) => {
      if (resp.success && resp.data) setConfig(resp.data)
    })
    void loadCoverage()
  }, [])

  const batchGenerate = async () => {
    setBusy(true)
    const resp = await post<{ success?: number; failed?: number }>('/api/admin/liff-qr/batch-generate', {})
    setBusy(false)
    if (resp.success) {
      setToast(`已產生 ${resp.data?.success ?? 0} 筆，失敗 ${resp.data?.failed ?? 0} 筆`)
      void loadCoverage()
    } else {
      setToast(resp.message ?? '產生失敗')
    }
  }

  if (!session.isAdmin()) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const columns: Column<CoverageTeam>[] = [
    { key: 'name', header: '團隊' },
    {
      key: 'hasLiffQR',
      header: 'LIFF QR',
      render: (t) => <StatusPill status={t.hasLiffQR ? 'active' : 'inactive'} label={t.hasLiffQR ? '已產生' : '尚未'} />,
    },
  ]

  return (
    <main style={{ maxWidth: 880, margin: '4vh auto', padding: '0 16px' }}>
      <h1>LIFF 設定</h1>

      <section
        style={{ border: '1px solid #eee', borderRadius: 10, padding: 16, margin: '12px 0' }}
      >
        <h3 style={{ marginTop: 0 }}>LINE / LIFF 組態</h3>
        <Row label="LIFF ID">{config.liffId || '—'}</Row>
        <Row label="LINE Bot ID">{config.lineBotId || '—'}</Row>
        <Row label="官方帳號">{config.lineOaId || '—'}</Row>
        <Row label="API Endpoint">{config.apiEndpoint || '—'}</Row>
        <Row label="版本">{config.version || '—'}</Row>
      </section>

      <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap', margin: '12px 0' }}>
        <StatCard label="團隊總數" value={coverage.totalTeams ?? 0} />
        <StatCard label="已有 QR" value={coverage.teamsWithLiffQR ?? 0} />
        <StatCard label="缺少 QR" value={coverage.teamsWithoutLiffQR ?? 0} />
        <StatCard label="覆蓋率" value={coverage.coverage ?? '—'} />
      </div>

      <button onClick={() => void batchGenerate()} disabled={busy}>
        {busy ? '產生中…' : '批次產生缺少的 LIFF QR'}
      </button>

      <div style={{ marginTop: 16 }}>
        <DataTable columns={columns} rows={coverage.teams ?? []} rowKey={(t) => t.id} empty="沒有團隊" />
      </div>

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </main>
  )
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', gap: 8, padding: '4px 0', fontSize: 14 }}>
      <span style={{ color: '#888', width: 120, flexShrink: 0 }}>{label}</span>
      <span style={{ wordBreak: 'break-all' }}>{children}</span>
    </div>
  )
}
