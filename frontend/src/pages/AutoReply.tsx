// Auto-reply configuration (admin): tabbed into rules (keyword/welcome/
// fallback), weekly off-hours schedules, and a delivery log (Phase 2.4 adds the
// schedules and logs tabs on top of the original rule CRUD).

import { useEffect, useState } from 'react'

import { get, post, del } from '../api/client'
import { session } from '../auth/session'
import { DataTable } from '../components/DataTable'
import { StatCard, Toast } from '../components/ui'
import type { Column } from '../components/DataTable'

interface Rule {
  id: number
  name: string
  triggerType: string
  priority: number
  isActive: boolean
}

type Tab = 'rules' | 'schedules' | 'logs'

export default function AutoReply() {
  const [tab, setTab] = useState<Tab>('rules')

  if (!session.isAdmin()) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  return (
    <main style={{ maxWidth: 820, margin: '4vh auto', padding: '0 16px' }}>
      <h1>自動回覆</h1>
      <div style={{ display: 'flex', gap: 8, borderBottom: '1px solid #ddd', marginBottom: 16 }}>
        {([
          ['rules', '規則'],
          ['schedules', '營業時間排程'],
          ['logs', '回覆記錄'],
        ] as [Tab, string][]).map(([key, label]) => (
          <button
            key={key}
            onClick={() => setTab(key)}
            style={{
              border: 'none',
              background: 'none',
              padding: '8px 12px',
              borderBottom: tab === key ? '2px solid #3B82F6' : '2px solid transparent',
              fontWeight: tab === key ? 700 : 400,
              cursor: 'pointer',
            }}
          >
            {label}
          </button>
        ))}
      </div>

      {tab === 'rules' && <RulesTab />}
      {tab === 'schedules' && <SchedulesTab />}
      {tab === 'logs' && <LogsTab />}
    </main>
  )
}

function RulesTab() {
  const [rules, setRules] = useState<Rule[]>([])
  const [name, setName] = useState('')
  const [trigger, setTrigger] = useState('keyword')
  const [keyword, setKeyword] = useState('')
  const [reply, setReply] = useState('')
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    const resp = await get<{ items?: Rule[] }>('/api/auto-reply/rules')
    if (resp.success && resp.data) setRules(resp.data.items ?? [])
    else setError(resp.message ?? null)
  }
  useEffect(() => {
    void load()
  }, [])

  const create = async (e: React.FormEvent) => {
    e.preventDefault()
    const body: Record<string, unknown> = {
      name,
      triggerType: trigger,
      actions: [{ actionType: 'text', content: JSON.stringify({ text: reply }) }],
    }
    if (trigger === 'keyword' && keyword.trim()) {
      body.conditions = [{ conditionType: 'contains', value: keyword.trim() }]
    }
    const resp = await post('/api/auto-reply/rules', body)
    if (resp.success) {
      setName('')
      setKeyword('')
      setReply('')
      void load()
    } else {
      setError(resp.message ?? null)
    }
  }

  const remove = async (id: number) => {
    const resp = await del(`/api/auto-reply/rules/${id}`)
    if (resp.success) void load()
    else setError(resp.message ?? null)
  }

  return (
    <>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <form onSubmit={create} style={{ display: 'grid', gap: 8, marginBottom: 16 }}>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="規則名稱" required />
        <select value={trigger} onChange={(e) => setTrigger(e.target.value)}>
          <option value="keyword">關鍵字</option>
          <option value="welcome">歡迎訊息</option>
          <option value="off_hours">非營業時間</option>
          <option value="fallback">預設回覆</option>
        </select>
        {trigger === 'keyword' && (
          <input value={keyword} onChange={(e) => setKeyword(e.target.value)} placeholder="關鍵字（包含比對）" />
        )}
        <input value={reply} onChange={(e) => setReply(e.target.value)} placeholder="回覆內容" required />
        <button type="submit">新增規則</button>
      </form>
      <ul style={{ listStyle: 'none', padding: 0 }}>
        {rules.map((r) => (
          <li
            key={r.id}
            style={{ display: 'flex', gap: 8, padding: 6, alignItems: 'center', borderBottom: '1px solid #f0f0f0' }}
          >
            <span style={{ color: '#999' }}>#{r.priority}</span>
            <strong>{r.name}</strong>
            <small>{r.triggerType}</small>
            {!r.isActive && <small style={{ color: 'orange' }}>停用</small>}
            <button onClick={() => void remove(r.id)} style={{ marginLeft: 'auto' }}>
              刪除
            </button>
          </li>
        ))}
      </ul>
    </>
  )
}

interface ScheduleEntry {
  dayOfWeek: number
  startTime: string
  endTime: string
  isActive: boolean
}

const DAY_LABELS = ['週日', '週一', '週二', '週三', '週四', '週五', '週六']

function SchedulesTab() {
  const [entries, setEntries] = useState<ScheduleEntry[]>(
    DAY_LABELS.map((_, d) => ({ dayOfWeek: d, startTime: '09:00', endTime: '18:00', isActive: false })),
  )
  const [timezone, setTimezone] = useState('Asia/Taipei')
  const [toast, setToast] = useState<string | null>(null)

  useEffect(() => {
    void get<ScheduleEntry[]>('/api/auto-reply/schedules').then((resp) => {
      if (resp.success && Array.isArray(resp.data) && resp.data.length > 0) {
        setEntries((prev) =>
          prev.map((row) => {
            const found = resp.data!.find((s) => s.dayOfWeek === row.dayOfWeek)
            return found
              ? {
                  dayOfWeek: row.dayOfWeek,
                  startTime: found.startTime ?? row.startTime,
                  endTime: found.endTime ?? row.endTime,
                  isActive: !!found.isActive,
                }
              : row
          }),
        )
      }
    })
  }, [])

  const setRow = (d: number, patch: Partial<ScheduleEntry>) =>
    setEntries((es) => es.map((e) => (e.dayOfWeek === d ? { ...e, ...patch } : e)))

  const save = async () => {
    const schedules = entries.filter((e) => e.isActive)
    const resp = await post('/api/auto-reply/schedules', { timezone, schedules })
    setToast(resp.success ? '排程已儲存' : resp.message ?? '儲存失敗')
  }

  return (
    <div>
      <label style={{ fontSize: 14, color: '#555' }}>
        時區{' '}
        <input value={timezone} onChange={(e) => setTimezone(e.target.value)} style={{ padding: '4px 8px' }} />
      </label>
      <table style={{ width: '100%', borderCollapse: 'collapse', marginTop: 12 }}>
        <tbody>
          {entries.map((e) => (
            <tr key={e.dayOfWeek} style={{ borderBottom: '1px solid #f0f0f0' }}>
              <td style={{ padding: 8, width: 80 }}>
                <label style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                  <input
                    type="checkbox"
                    checked={e.isActive}
                    onChange={(ev) => setRow(e.dayOfWeek, { isActive: ev.target.checked })}
                  />
                  {DAY_LABELS[e.dayOfWeek]}
                </label>
              </td>
              <td style={{ padding: 8 }}>
                <input
                  type="time"
                  value={e.startTime}
                  disabled={!e.isActive}
                  onChange={(ev) => setRow(e.dayOfWeek, { startTime: ev.target.value })}
                />
                {' ~ '}
                <input
                  type="time"
                  value={e.endTime}
                  disabled={!e.isActive}
                  onChange={(ev) => setRow(e.dayOfWeek, { endTime: ev.target.value })}
                />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <button onClick={() => void save()} style={{ marginTop: 12 }}>
        儲存排程
      </button>
      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}

interface LogRow {
  id: string
  ruleName?: string
  triggerContent?: string
  responseContent?: string
  platform?: string
  createdAt?: string
}

function LogsTab() {
  const [logs, setLogs] = useState<LogRow[]>([])
  const [todayCount, setTodayCount] = useState(0)
  const [total, setTotal] = useState(0)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    setBusy(true)
    void get<{ items?: LogRow[]; todayCount?: number; total?: number }>('/api/auto-reply/logs').then((resp) => {
      if (resp.success && resp.data) {
        setLogs(resp.data.items ?? [])
        setTodayCount(resp.data.todayCount ?? 0)
        setTotal(resp.data.total ?? 0)
      }
      setBusy(false)
    })
  }, [])

  const columns: Column<LogRow>[] = [
    {
      key: 'createdAt',
      header: '時間',
      width: 150,
      render: (l) => (l.createdAt ? new Date(l.createdAt).toLocaleString() : '—'),
    },
    { key: 'ruleName', header: '規則', render: (l) => l.ruleName || '—' },
    { key: 'triggerContent', header: '觸發內容', render: (l) => l.triggerContent || '—' },
    { key: 'responseContent', header: '回覆內容', render: (l) => l.responseContent || '—' },
    { key: 'platform', header: '平台', width: 80, render: (l) => l.platform || '—' },
  ]

  return (
    <div>
      <div style={{ display: 'flex', gap: 10, margin: '0 0 12px' }}>
        <StatCard label="今日觸發" value={todayCount} />
        <StatCard label="累計觸發" value={total} />
      </div>
      <DataTable columns={columns} rows={logs} rowKey={(l) => l.id} busy={busy} empty="尚無回覆記錄" />
    </div>
  )
}
