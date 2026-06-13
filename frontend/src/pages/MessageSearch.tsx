// Message search screen (Phase 1.4): full-text + faceted search across the
// message history with offset pagination and a CSV export of the current
// filter set.

import { useState } from 'react'
import { Link } from 'react-router-dom'

import { DataTable } from '../components/DataTable'
import { FilterBar, StatusPill, Toast } from '../components/ui'
import {
  searchMessages,
  exportMessagesCsv,
  type MessageHit,
  type MessageSearchParams,
} from '../stores/messages'
import type { Column } from '../components/DataTable'

const PAGE_SIZE = 50

const SENDER_TYPES = [
  { value: '', label: '全部發送者' },
  { value: 'customer', label: '客戶' },
  { value: 'agent', label: '客服' },
  { value: 'system', label: '系統' },
]

const input: React.CSSProperties = {
  padding: '7px 9px',
  borderRadius: 6,
  border: '1px solid #ccc',
}

export default function MessageSearch() {
  const [filters, setFilters] = useState<MessageSearchParams>({})
  const [rows, setRows] = useState<MessageHit[]>([])
  const [total, setTotal] = useState(0)
  const [offset, setOffset] = useState(0)
  const [busy, setBusy] = useState(false)
  const [searched, setSearched] = useState(false)
  const [toast, setToast] = useState<string | null>(null)

  const run = async (nextOffset = 0) => {
    setBusy(true)
    const res = await searchMessages({ ...filters, limit: PAGE_SIZE, offset: nextOffset })
    setRows(res.messages)
    setTotal(res.total)
    setOffset(nextOffset)
    setBusy(false)
    setSearched(true)
  }

  const exportCsv = async () => {
    const res = await exportMessagesCsv(filters)
    if (!res.ok) setToast(res.message ?? '匯出失敗')
  }

  const set = (patch: Partial<MessageSearchParams>) => setFilters((f) => ({ ...f, ...patch }))

  const columns: Column<MessageHit>[] = [
    {
      key: 'createdAt',
      header: '時間',
      width: 150,
      render: (m) => (m.createdAt ? new Date(m.createdAt).toLocaleString() : '—'),
    },
    { key: 'senderType', header: '發送者', render: (m) => <StatusPill status={m.senderType ?? ''} label={m.senderName || m.senderType} /> },
    {
      key: 'content',
      header: '內容',
      render: (m) => (
        <span style={{ textDecoration: m.isRecalled ? 'line-through' : 'none', color: m.isRecalled ? '#aaa' : 'inherit' }}>
          {m.content || '—'}
        </span>
      ),
    },
    {
      key: 'conversationId',
      header: '對話',
      width: 90,
      render: (m) => <Link to={`/conversations/${m.conversationId}`}>{m.conversationId.slice(0, 8)}</Link>,
    },
  ]

  return (
    <main style={{ maxWidth: 1000, margin: '4vh auto', padding: '0 16px' }}>
      <h1>訊息搜尋</h1>
      <FilterBar>
        <input
          placeholder="關鍵字"
          value={filters.q ?? ''}
          onChange={(e) => set({ q: e.target.value })}
          onKeyDown={(e) => e.key === 'Enter' && void run(0)}
          style={{ ...input, minWidth: 220 }}
        />
        <select value={filters.senderType ?? ''} onChange={(e) => set({ senderType: e.target.value })} style={input}>
          {SENDER_TYPES.map((s) => (
            <option key={s.value} value={s.value}>
              {s.label}
            </option>
          ))}
        </select>
        <label style={{ fontSize: 13, color: '#666' }}>
          從 <input type="date" value={filters.dateFrom ?? ''} onChange={(e) => set({ dateFrom: e.target.value })} style={input} />
        </label>
        <label style={{ fontSize: 13, color: '#666' }}>
          到 <input type="date" value={filters.dateTo ?? ''} onChange={(e) => set({ dateTo: e.target.value })} style={input} />
        </label>
        <button onClick={() => void run(0)} disabled={busy}>
          搜尋
        </button>
        <button onClick={() => void exportCsv()} disabled={busy} style={{ marginLeft: 'auto' }}>
          匯出 CSV
        </button>
      </FilterBar>

      {searched && (
        <>
          <DataTable
            columns={columns}
            rows={rows}
            rowKey={(m) => m.id}
            busy={busy}
            empty="沒有符合的訊息"
          />
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', padding: '8px 0' }}>
            <button disabled={busy || offset === 0} onClick={() => void run(Math.max(0, offset - PAGE_SIZE))}>
              上一頁
            </button>
            <span style={{ fontSize: 14, color: '#555' }}>
              {total === 0 ? 0 : offset + 1}–{Math.min(offset + PAGE_SIZE, total)} / 共 {total} 筆
            </span>
            <button disabled={busy || offset + PAGE_SIZE >= total} onClick={() => void run(offset + PAGE_SIZE)}>
              下一頁
            </button>
          </div>
        </>
      )}

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </main>
  )
}
