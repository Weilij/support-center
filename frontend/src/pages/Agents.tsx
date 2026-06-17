// Agent management (Phase 2.1, admin): roster with presence summary cards and
// a batch team-transfer action. Roster is server-paginated.

import { useEffect, useState } from 'react'

import { DataTable, Pagination } from '../components/DataTable'
import { StatCard, StatusPill, Toast } from '../components/ui'
import { PageHeader } from '../components/PageHeader'
import { Card, StatGrid } from '../components/Card'
import { Modal, ConfirmDialog } from '../components/Modal'
import { Input, Select } from '../components/Form'
import { useStore } from '../stores/store'
import { teamsStore, loadTeams } from '../stores/teams'
import {
  loadAgents,
  loadStatusStatistics,
  batchTransferAgents,
  setAgentPosition,
  createAgent,
  deleteAgent,
  PRESENCE_STATES,
  type Agent,
} from '../stores/agents'
import type { Column } from '../components/DataTable'
import { session } from '../auth/session'
import { can, positionOf, POSITION_LABELS, AREA_ACCESS, type Position } from '../auth/permissions'

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

  // Create-account modal state
  const [createOpen, setCreateOpen] = useState(false)
  const [createName, setCreateName] = useState('')
  const [createEmail, setCreateEmail] = useState('')
  const [createPassword, setCreatePassword] = useState('')
  const [createRole, setCreateRole] = useState<'agent' | 'admin'>('agent')
  const [createError, setCreateError] = useState<string | null>(null)
  const [createBusy, setCreateBusy] = useState(false)

  const [toDelete, setToDelete] = useState<Agent | null>(null)

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

  const confirmDelete = async () => {
    if (!toDelete) return
    const res = await deleteAgent(toDelete.id)
    setToast(res.ok ? '帳號已刪除' : res.message ?? '刪除失敗')
    setToDelete(null)
    if (res.ok) void load(page)
  }

  const resetCreateForm = () => {
    setCreateName('')
    setCreateEmail('')
    setCreatePassword('')
    setCreateRole('agent')
    setCreateError(null)
  }

  const handleCreateSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!createName.trim() || !createEmail.trim() || !createPassword.trim()) {
      setCreateError('請填寫所有欄位')
      return
    }
    if (createPassword.length < 6) {
      setCreateError('密碼至少需要 6 個字元')
      return
    }
    setCreateError(null)
    setCreateBusy(true)
    const res = await createAgent({
      email: createEmail.trim(),
      password: createPassword,
      displayName: createName.trim(),
      role: createRole,
    })
    setCreateBusy(false)
    if (res.ok) {
      setCreateOpen(false)
      resetCreateForm()
      setToast('帳號已建立')
      void load(page)
    } else {
      setCreateError(res.message ?? '建立失敗')
    }
  }

  if (!can(session.position(), 'ops')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
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
    {
      key: 'del',
      header: '',
      width: 70,
      render: (a) =>
        session.position() === 'system_admin' && a.id !== session.identity()?.id ? (
          <button onClick={() => setToDelete(a)} style={{ color: 'var(--busy)' }}>刪除</button>
        ) : null,
    },
  ]

  return (
    <div style={{ maxWidth: 1040, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader
        title="客服人員管理"
        actions={
          canEditPosition ? (
            <button
              className="cs-btn cs-btn--primary"
              onClick={() => { resetCreateForm(); setCreateOpen(true) }}
            >
              新增帳號
            </button>
          ) : undefined
        }
      />

      <StatGrid style={{ marginBottom: 'var(--sp-4)' }}>
        {PRESENCE_STATES.map((s) => (
          <StatCard key={s} label={PRESENCE_LABELS[s] ?? s} value={stats[s] ?? 0} />
        ))}
      </StatGrid>

      {selected.size > 0 && (
        <Card style={{ marginBottom: 'var(--sp-3)' }}>
          <div style={{ display: 'flex', gap: 10, alignItems: 'center', flexWrap: 'wrap' }}>
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
        </Card>
      )}

      <DataTable columns={columns} rows={agents} rowKey={(a) => a.id} busy={busy} empty="沒有客服人員" />
      <Pagination page={page} total={total} pageSize={PAGE_SIZE} onPage={(p) => void load(p)} />

      <Toast message={toast} onDismiss={() => setToast(null)} />

      <Modal
        open={createOpen}
        title="新增帳號"
        onClose={() => { setCreateOpen(false); resetCreateForm() }}
      >
        <form onSubmit={(e) => void handleCreateSubmit(e)}>
          <Input
            label="顯示名稱"
            value={createName}
            onChange={(e) => setCreateName(e.target.value)}
            autoFocus
          />
          <Input
            label="電子郵件"
            type="email"
            value={createEmail}
            onChange={(e) => setCreateEmail(e.target.value)}
          />
          <Input
            label="密碼"
            type="password"
            value={createPassword}
            onChange={(e) => setCreatePassword(e.target.value)}
          />
          <Select
            label="角色"
            value={createRole}
            onChange={(e) => setCreateRole(e.target.value as 'agent' | 'admin')}
            options={[
              { value: 'agent', label: '客服' },
              { value: 'admin', label: '管理員' },
            ]}
          />
          {createError && (
            <p role="alert" style={{ color: 'crimson', fontSize: 13, margin: '0 0 12px' }}>
              {createError}
            </p>
          )}
          <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
            <button
              type="button"
              onClick={() => { setCreateOpen(false); resetCreateForm() }}
            >
              取消
            </button>
            <button
              type="submit"
              className="cs-btn cs-btn--primary"
              disabled={createBusy}
            >
              {createBusy ? '建立中…' : '建立'}
            </button>
          </div>
        </form>
      </Modal>

      <ConfirmDialog
        open={!!toDelete}
        title="刪除帳號"
        message={`確定要刪除「${toDelete?.displayName || toDelete?.email}」這個帳號嗎？此動作會移除其團隊關聯。`}
        confirmLabel="刪除"
        danger
        onConfirm={() => void confirmDelete()}
        onCancel={() => setToDelete(null)}
      />

      <Card title="職位權限對照表" style={{ marginTop: 'var(--sp-5)' }}>
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
              <tr key={area} style={{ borderTop: '1px solid var(--hairline)' }}>
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
      </Card>
    </div>
  )
}
