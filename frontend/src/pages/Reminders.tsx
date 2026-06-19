// Reminders (Phase 3.3): create personal/conversation reminders, see upcoming
// ones within a window, and mark them complete. Summary cards show the backlog.

import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'

import { get, post, put } from '../api/client'
import { DataTable } from '../components/DataTable'
import { Input, Textarea } from '../components/Form'
import { StatCard, Toast } from '../components/ui'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'
import type { Column } from '../components/DataTable'

interface Reminder {
  id: string
  title?: string
  content?: string
  remindAt?: string
  conversationId?: string | null
  isCompleted?: boolean
}

interface ReminderStats {
  total?: number
  pending?: number
  completed?: number
  overdue?: number
}

export default function Reminders() {
  const [reminders, setReminders] = useState<Reminder[]>([])
  const [stats, setStats] = useState<ReminderStats>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState(false)
  const [title, setTitle] = useState('')
  const [content, setContent] = useState('')
  const [remindAt, setRemindAt] = useState('')
  const [toast, setToast] = useState<string | null>(null)

  const load = async () => {
    setBusy(true)
    setError(false)
    const [up, st] = await Promise.all([
      get<{ reminders?: Reminder[] }>('/api/reminders/upcoming'),
      get<ReminderStats>('/api/reminders/stats'),
    ])
    if (up.success && up.data) setReminders(up.data.reminders ?? [])
    else setError(true)
    if (st.success && st.data) setStats(st.data)
    setBusy(false)
  }
  useEffect(() => {
    void load()
  }, [])

  const create = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!title.trim() || !remindAt) return
    const resp = await post('/api/reminders', {
      title: title.trim(),
      content: content.trim() || undefined,
      remindAt: new Date(remindAt).toISOString(),
    })
    if (resp.success) {
      setToast('提醒已建立')
      setTitle('')
      setContent('')
      setRemindAt('')
      void load()
    } else {
      setToast(resp.message ?? '建立失敗')
    }
  }

  const complete = async (id: string) => {
    const resp = await put(`/api/reminders/${id}/complete`)
    if (resp.success) {
      setToast('已完成')
      void load()
    }
  }

  const columns: Column<Reminder>[] = [
    {
      key: 'remindAt',
      header: '時間',
      width: 160,
      render: (r) => (r.remindAt ? new Date(r.remindAt).toLocaleString() : '—'),
    },
    { key: 'title', header: '提醒', render: (r) => r.title || '—' },
    {
      key: 'conversationId',
      header: '對話',
      width: 90,
      render: (r) =>
        r.conversationId ? <Link to={`/conversations/${r.conversationId}`}>{String(r.conversationId).slice(0, 8)}</Link> : '—',
    },
    {
      key: 'action',
      header: '',
      width: 80,
      render: (r) => (!r.isCompleted ? <button onClick={() => void complete(r.id)}>完成</button> : '✓'),
    },
  ]

  return (
    <div style={{ maxWidth: 880, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="提醒" />

      <div style={{ display: 'flex', gap: 'var(--sp-4)', flexWrap: 'wrap', marginBottom: 'var(--sp-5)' }}>
        <StatCard label="總計" value={stats.total ?? 0} />
        <StatCard label="待處理" value={stats.pending ?? 0} />
        <StatCard label="已完成" value={stats.completed ?? 0} />
        <StatCard label="逾期" value={stats.overdue ?? 0} />
      </div>

      <Card title="新增提醒" style={{ marginBottom: 'var(--sp-5)' }}>
        <form onSubmit={create} style={{ display: 'grid', gap: 'var(--sp-3)' }}>
          <Input label="標題" value={title} onChange={(e) => setTitle(e.target.value)} />
          <Input label="提醒時間" type="datetime-local" value={remindAt} onChange={(e) => setRemindAt(e.target.value)} />
          <Textarea label="內容（選填）" value={content} onChange={(e) => setContent(e.target.value)} />
          <div>
            <button type="submit">建立</button>
          </div>
        </form>
      </Card>

      <Card title="即將到來">
        {error ? (
          <div style={{ padding: '24px 16px', textAlign: 'center', color: 'crimson' }}>
            <p style={{ margin: '0 0 var(--sp-3)' }}>載入失敗，請重試</p>
            <button onClick={() => void load()}>重試</button>
          </div>
        ) : (
          <DataTable columns={columns} rows={reminders} rowKey={(r) => r.id} busy={busy} empty="沒有即將到來的提醒" />
        )}
      </Card>

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}
