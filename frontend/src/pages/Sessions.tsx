// Session management (Phase 2.3, admin): paginated sessions with summary cards,
// inline topic editing, and close/reopen actions.

import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'

import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { DataTable, Pagination } from '../components/DataTable'
import { Modal, ConfirmDialog } from '../components/Modal'
import { Input } from '../components/Form'
import { StatCard, StatusPill, Toast } from '../components/ui'
import { PageHeader } from '../components/PageHeader'
import { StatGrid } from '../components/Card'
import {
  loadSessions,
  loadSessionStats,
  closeSession,
  reopenSession,
  updateSessionTopic,
  type SessionRow,
  type SessionStats,
} from '../stores/sessions'
import type { Column } from '../components/DataTable'

const PAGE_SIZE = 20

export default function Sessions() {
  const [rows, setRows] = useState<SessionRow[]>([])
  const [total, setTotal] = useState(0)
  const [page, setPage] = useState(1)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState(false)
  const [stats, setStats] = useState<SessionStats>({})
  const [editing, setEditing] = useState<SessionRow | null>(null)
  const [topicDraft, setTopicDraft] = useState('')
  const [toast, setToast] = useState<string | null>(null)
  const [toToggle, setToToggle] = useState<SessionRow | null>(null)
  const [togglingId, setTogglingId] = useState<string | null>(null)

  const load = async (p: number) => {
    setBusy(true)
    setError(false)
    const res = await loadSessions(p, PAGE_SIZE)
    if (!res.ok) setError(true)
    setRows(res.sessions)
    setTotal(res.total)
    setPage(p)
    setBusy(false)
  }

  useEffect(() => {
    void load(1)
    void loadSessionStats().then(setStats)
  }, [])

  const confirmToggle = async () => {
    if (!toToggle || togglingId) return
    const s = toToggle
    setTogglingId(s.id)
    try {
      const ok = s.isActive ? await closeSession(s.id) : await reopenSession(s.id)
      setToast(ok ? (s.isActive ? '已關閉工作階段' : '已重新開啟') : '操作失敗')
      if (ok) await load(page)
    } finally {
      setTogglingId(null)
      setToToggle(null)
    }
  }

  const saveTopic = async () => {
    if (!editing) return
    const ok = await updateSessionTopic(editing.id, topicDraft)
    setToast(ok ? '主題已更新' : '更新失敗')
    if (ok) {
      setRows((rs) => rs.map((r) => (r.id === editing.id ? { ...r, topic: topicDraft } : r)))
      setEditing(null)
    }
  }

  if (!can(session.position(), 'ops')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const columns: Column<SessionRow>[] = [
    {
      key: 'topic',
      header: '主題',
      render: (s) => (
        <button onClick={() => { setEditing(s); setTopicDraft(s.topic ?? '') }} style={{ textAlign: 'left' }}>
          {s.topic || '（未命名，點選編輯）'}
        </button>
      ),
    },
    {
      key: 'conversationId',
      header: '對話',
      width: 90,
      render: (s) =>
        s.conversationId ? (
          <Link to={`/conversations/${s.conversationId}`}>{s.conversationId.slice(0, 8)}</Link>
        ) : (
          '—'
        ),
    },
    { key: 'sessionType', header: '類型', render: (s) => s.sessionType || '—' },
    { key: 'messageCount', header: '訊息數', align: 'right', render: (s) => s.messageCount ?? 0 },
    { key: 'priority', header: '優先級', render: (s) => (s.priority ? <StatusPill status={s.priority} /> : '—') },
    {
      key: 'isActive',
      header: '狀態',
      render: (s) => <StatusPill status={s.isActive ? 'active' : 'closed'} label={s.isActive ? '進行中' : '已結束'} />,
    },
    {
      key: 'action',
      header: '',
      width: 90,
      render: (s) => (
        <button onClick={() => setToToggle(s)} disabled={togglingId === s.id}>
          {togglingId === s.id ? '處理中…' : s.isActive ? '關閉' : '重啟'}
        </button>
      ),
    },
  ]

  return (
    <div style={{ maxWidth: 1040, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="工作階段" />

      <StatGrid style={{ marginBottom: 'var(--sp-4)' }}>
        <StatCard label="總計" value={stats.total ?? 0} />
        <StatCard label="進行中" value={stats.active ?? 0} />
        <StatCard label="已結束" value={stats.inactive ?? 0} />
      </StatGrid>

      {error ? (
        <div style={{ padding: '24px 16px', textAlign: 'center', color: 'crimson' }}>
          <p style={{ margin: '0 0 var(--sp-3)' }}>載入失敗，請重試</p>
          <button onClick={() => void load(page)}>重試</button>
        </div>
      ) : (
        <>
          <DataTable columns={columns} rows={rows} rowKey={(s) => s.id} busy={busy} empty="沒有工作階段" />
          <Pagination page={page} total={total} pageSize={PAGE_SIZE} onPage={(p) => void load(p)} />
        </>
      )}

      <Modal open={!!editing} title="編輯主題" onClose={() => setEditing(null)} width={420}>
        <Input
          label="主題"
          value={topicDraft}
          onChange={(e) => setTopicDraft(e.target.value)}
          placeholder="輸入工作階段主題"
        />
        <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
          <button onClick={() => setEditing(null)}>取消</button>
          <button onClick={() => void saveTopic()}>儲存</button>
        </div>
      </Modal>

      <ConfirmDialog
        open={!!toToggle}
        title="確認操作"
        message={toToggle?.isActive ? '確定要關閉此工作階段嗎？' : '確定要重新開啟此工作階段嗎？'}
        confirmLabel={toToggle?.isActive ? '關閉' : '重啟'}
        danger={!!toToggle?.isActive}
        onConfirm={() => void confirmToggle()}
        onCancel={() => setToToggle(null)}
      />

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}
