// Conversation list screen wired to the conversations store. Supports
// bulk selection with a routing/priority action toolbar (Phase 1.2) on top of
// the original open-on-click + mark-read behaviour.

import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router-dom'

import {
  conversationsStore,
  loadConversations,
  markConversationRead,
  bulkConversations,
  type BulkOperation,
} from '../stores/conversations'
import { useStore } from '../stores/store'
import { teamsStore, loadTeams } from '../stores/teams'
import { StatusPill, Toast } from '../components/ui'

const PRIORITIES = [
  { value: 'low', label: '低' },
  { value: 'normal', label: '一般' },
  { value: 'high', label: '高' },
  { value: 'urgent', label: '緊急' },
]

export default function Conversations() {
  const navigate = useNavigate()
  const state = useStore(conversationsStore)
  const { items: teams } = useStore(teamsStore)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [toast, setToast] = useState<string | null>(null)

  useEffect(() => {
    void loadConversations()
    void loadTeams()
  }, [])

  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })

  const clearSelection = () => setSelected(new Set())

  const runBulk = async (operation: BulkOperation, data: Record<string, unknown>) => {
    const ids = [...selected]
    if (ids.length === 0) return
    const ok = await bulkConversations(ids, operation, data)
    setToast(ok ? `已更新 ${ids.length} 筆對話` : '批次操作失敗')
    if (ok) clearSelection()
  }

  return (
    <main style={{ maxWidth: 820, margin: '4vh auto', fontFamily: 'sans-serif', padding: '0 16px' }}>
      <h1>對話</h1>
      {state.busy && <p>載入中…</p>}
      {state.error && <p role="alert" style={{ color: 'crimson' }}>{state.error}</p>}

      {selected.size > 0 && (
        <div
          style={{
            display: 'flex',
            gap: 10,
            alignItems: 'center',
            flexWrap: 'wrap',
            padding: 10,
            background: '#F1F5F9',
            borderRadius: 8,
            margin: '10px 0',
          }}
        >
          <strong>{selected.size} 筆已選</strong>

          <select
            defaultValue=""
            onChange={(e) => {
              if (e.target.value) void runBulk('assign', { teamId: Number(e.target.value) })
              e.target.value = ''
            }}
            style={{ padding: '6px 8px', borderRadius: 6, border: '1px solid #ccc' }}
          >
            <option value="">指派團隊…</option>
            {teams.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name}
              </option>
            ))}
          </select>

          <select
            defaultValue=""
            onChange={(e) => {
              if (e.target.value) void runBulk('set_priority', { priority: e.target.value })
              e.target.value = ''
            }}
            style={{ padding: '6px 8px', borderRadius: 6, border: '1px solid #ccc' }}
          >
            <option value="">設定優先級…</option>
            {PRIORITIES.map((p) => (
              <option key={p.value} value={p.value}>
                {p.label}
              </option>
            ))}
          </select>

          <button onClick={clearSelection} style={{ marginLeft: 'auto' }}>
            取消選取
          </button>
        </div>
      )}

      <ul style={{ listStyle: 'none', padding: 0 }}>
        {state.items.map((c) => (
          <li
            key={c.id}
            style={{
              display: 'flex',
              gap: 10,
              alignItems: 'center',
              padding: 8,
              borderBottom: '1px solid #eee',
            }}
          >
            <input
              type="checkbox"
              checked={selected.has(c.id)}
              onChange={() => toggle(c.id)}
              onClick={(e) => e.stopPropagation()}
            />
            <div
              style={{ flex: 1, cursor: 'pointer' }}
              onClick={() => {
                void markConversationRead(c.id)
                navigate(`/conversations/${c.id}`)
              }}
            >
              <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
                <strong>{c.customerName ?? c.id}</strong>
                <StatusPill status={c.status} />
                {c.priority && c.priority !== 'normal' && (
                  <span style={{ fontSize: 12, color: '#b45309' }}>{c.priority}</span>
                )}
                {(c.unreadCount ?? 0) > 0 && (
                  <span
                    style={{
                      color: 'white',
                      background: 'crimson',
                      borderRadius: 8,
                      padding: '0 6px',
                    }}
                  >
                    {c.unreadCount}
                  </span>
                )}
              </div>
              <div style={{ color: '#666', fontSize: 13 }}>{c.lastMessage}</div>
            </div>
          </li>
        ))}
      </ul>
      <p>共 {state.total} 筆</p>

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </main>
  )
}
