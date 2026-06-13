// Agent management (Phase 2.1, admin): roster with presence summary cards and
// a batch team-transfer action. Roster is server-paginated.

import { useEffect, useState } from 'react'

import { DataTable, Pagination } from '../components/DataTable'
import { StatCard, StatusPill, Toast } from '../components/ui'
import { useStore } from '../stores/store'
import { teamsStore, loadTeams } from '../stores/teams'
import {
  loadAgents,
  loadStatusStatistics,
  batchTransferAgents,
  setAgentPosition,
  PRESENCE_STATES,
  type Agent,
} from '../stores/agents'
import type { Column } from '../components/DataTable'
import { session } from '../auth/session'
import { positionOf, POSITION_LABELS, AREA_ACCESS, type Position } from '../auth/permissions'

const PAGE_SIZE = 20

const PRESENCE_LABELS: Record<string, string> = {
  online: '上線',
  busy: '忙碌',
  away: '離開',
  offline: '離線',
  break: '休息',
  meeting: '會議',
}

const AREA_LABELS: Record<string, string> = {
  daily: '日常',
  ops: '營運管理',
  analytics: '分析',
  system: '系統',
}

export default function Agents() {
  const { items: teams } = useStore(teamsStore)
  const [agents, setAgents] = useState<Agent[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [busy, setBusy] = useState(false)
  const [stats, setStats] = useState<Record<string, number>>({})
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [toast, setToast] = useState<string | null>(null)

  const load = async (p: number) => {
    setBusy(true)
    const res = await loadAgents(p, PAGE_SIZE)
    setAgents(res.items)
    setTotal(res.total)
    setPage(p)
    setBusy(false)
  }

  useEffect(() => {
    void load(1)
    void loadTeams()
    void loadStatusStatistics().then(setStats)
  }, [])

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const transfer = async (toTeamId: number) => {
    const ids = [...selected]
    if (ids.length === 0) return
    const res = await batchTransferAgents(ids, toTeamId)
    setToast(res.ok ? `已轉移 ${ids.length} 位客服` : res.message ?? '轉移失敗')
    if (res.ok) {
      setSelected(new Set())
      void load(page)
    }
  }

  const canEditPosition = session.position() === 'system_admin'

  const changePosition = async (agentId: string, position: string) => {
    const res = await setAgentPosition(agentId, position)
    setToast(res.ok ? '職位已更新' : res.message ?? '更新失敗')
    if (res.ok) setAgents((as) => as.map((a) => (a.id === agentId ? { ...a, position } : a)))
  }

  const columns: Column<Agent>[] = [
    {
      key: 'sel',
      header: '',
      width: 30,
      render: (a) => (
        <input type="checkbox" checked={selected.has(a.id)} onChange={() => toggle(a.id)} />
      ),
    },
    { key: 'displayName', header: '名稱', render: (a) => a.displayName || a.email || a.id },
    { key: 'email', header: 'Email', render: (a) => a.email || '—' },
    { key: 'role', header: '角色', render: (a) => <StatusPill status={a.role ?? ''} /> },
    {
      key: 'position',
      header: '職位',
      width: 150,
      render: (a) =>
        canEditPosition ? (
          <select
            value={positionOf(a as { position?: string; role?: string })}
            onChange={(e) => void changePosition(a.id, e.target.value)}
            style={{ padding: '3px 6px', borderRadius: 6, border: '1px solid #ccc' }}
          >
            {(Object.keys(POSITION_LABELS) as Position[]).map((p) => (
              <option key={p} value={p}>
                {POSITION_LABELS[p]}
              </option>
            ))}
          </select>
        ) : (
          POSITION_LABELS[positionOf(a as { position?: string; role?: string })]
        ),
    },
    { key: 'teamName', header: '團隊', render: (a) => a.teamName || '—' },
    {
      key: 'isActive',
      header: '狀態',
      render: (a) => <StatusPill status={a.isActive ? 'active' : 'inactive'} label={a.isActive ? '啟用' : '停用'} />,
    },
    {
      key: 'lastActiveAt',
      header: '最後活動',
      render: (a) => (a.lastActiveAt ? new Date(a.lastActiveAt).toLocaleString() : '—'),
    },
  ]

  return (
    <main style={{ maxWidth: 1040, margin: '4vh auto', padding: '0 16px' }}>
      <h1>客服人員管理</h1>

      <div style={{ display: 'flex', gap: 10, flexWrap: 'wrap', margin: '12px 0' }}>
        {PRESENCE_STATES.map((s) => (
          <StatCard key={s} label={PRESENCE_LABELS[s] ?? s} value={stats[s] ?? 0} />
        ))}
      </div>

      {selected.size > 0 && (
        <div
          style={{
            display: 'flex',
            gap: 10,
            alignItems: 'center',
            padding: 10,
            background: '#F1F5F9',
            borderRadius: 8,
            margin: '10px 0',
          }}
        >
          <strong>{selected.size} 位已選</strong>
          <select
            defaultValue=""
            onChange={(e) => {
              if (e.target.value) void transfer(Number(e.target.value))
              e.target.value = ''
            }}
            style={{ padding: '6px 8px', borderRadius: 6, border: '1px solid #ccc' }}
          >
            <option value="">批次轉移至團隊…</option>
            {teams.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>
          <button onClick={() => setSelected(new Set())} style={{ marginLeft: 'auto' }}>
            取消選取
          </button>
        </div>
      )}

      <DataTable columns={columns} rows={agents} rowKey={(a) => a.id} busy={busy} empty="沒有客服人員" />
      <Pagination page={page} total={total} pageSize={PAGE_SIZE} onPage={(p) => void load(p)} />

      <Toast message={toast} onDismiss={() => setToast(null)} />

      <section style={{ marginTop: 24 }}>
        <h3>職位權限對照表</h3>
        <table style={{ borderCollapse: 'collapse', fontSize: 14 }}>
          <thead>
            <tr>
              <th style={{ textAlign: 'left', padding: '6px 12px' }}>區域</th>
              {(Object.keys(POSITION_LABELS) as Position[]).map((p) => (
                <th key={p} style={{ padding: '6px 12px' }}>{POSITION_LABELS[p]}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {(['daily', 'ops', 'analytics', 'system'] as const).map((area) => (
              <tr key={area} style={{ borderTop: '1px solid #eee' }}>
                <td style={{ padding: '6px 12px' }}>{AREA_LABELS[area]}</td>
                {(Object.keys(POSITION_LABELS) as Position[]).map((p) => (
                  <td key={p} style={{ textAlign: 'center', padding: '6px 12px' }}>
                    {AREA_ACCESS[p].includes(area) ? '✅' : '—'}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </main>
  )
}
