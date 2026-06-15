// Inbox — 3-column workspace (N5): conversation list + thread + customer panel.
// Replaces the separate Conversations and ConversationDetail pages.
// Columns: .cs-conv-list (340px) | .cs-thread (flex 1) | .cs-cust (300px)
// The .cs-inbox flex container lives inside .cs-content which already has
// overflow:hidden and flex:1 — so height:100% fills the available space.

import { useCallback, useEffect, useRef, useState } from 'react'
import { useNavigate, useParams } from 'react-router-dom'

import { get, post } from '../api/client'
import { onEvent, subscribeConversation } from '../realtime/client'
import { session } from '../auth/session'
import {
  conversationsStore,
  loadConversations,
  markConversationRead,
  type Conversation,
} from '../stores/conversations'
import {
  loadCustomerDetail,
  loadCustomerTags,
  type Customer,
  type CustomerTag,
} from '../stores/customers'
import { useStore } from '../stores/store'
import { Avatar } from '../components/Avatar'
import { ChanGlyph } from '../components/ChanGlyph'
import { Tag } from '../components/Chip'
import { Icon } from '../components/Icon'
import { AssignDialog, type AssignMode } from '../components/ConversationAssign'
import { channelOf, CHANNELS } from '../components/channels'

// ── Types ────────────────────────────────────────────────────────────────────

interface Message {
  id: string
  content?: string
  senderType?: string
  senderName?: string
  createdAt?: string
  pending?: boolean
}

interface ConvMeta {
  platform?: string
  platformUserId?: string
  teamId?: number | null
  customerId?: number | null
  customerName?: string
}

// ── Helpers ──────────────────────────────────────────────────────────────────

type TabKey = 'all' | 'unread' | 'mine' | 'follow'
const TABS: { key: TabKey; label: string }[] = [
  { key: 'all',    label: '全部' },
  { key: 'unread', label: '未讀' },
  { key: 'mine',   label: '我的' },
  { key: 'follow', label: '待跟進' },
]

function formatTime(iso?: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (isNaN(d.getTime())) return ''
  const now = new Date()
  const diffMs = now.getTime() - d.getTime()
  const diffDays = Math.floor(diffMs / 86400000)
  if (diffDays === 0) return d.toLocaleTimeString('zh-TW', { hour: '2-digit', minute: '2-digit', hour12: false })
  if (diffDays === 1) return '昨天'
  if (diffDays < 7) return ['日', '一', '二', '三', '四', '五', '六'][d.getDay()] !== undefined
    ? `週${ ['日', '一', '二', '三', '四', '五', '六'][d.getDay()] }` : ''
  return d.toLocaleDateString('zh-TW', { month: 'numeric', day: 'numeric' })
}

function dayLabel(iso?: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (isNaN(d.getTime())) return ''
  const now = new Date()
  if (
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate()
  ) {
    return `今天 · ${d.getMonth() + 1} 月 ${d.getDate()} 日`
  }
  return d.toLocaleDateString('zh-TW', { month: 'long', day: 'numeric' })
}

// ── Sub-components ────────────────────────────────────────────────────────────

function ConvItem({
  conv,
  active,
  onClick,
}: {
  conv: Conversation
  active: boolean
  onClick: () => void
}) {
  const platform = String(conv.platform ?? conv['channel'] ?? 'chat')
  const chanKey = channelOf(platform) as 'chat' | 'line' | 'wa' | 'fb'
  const name = conv.customerName ?? conv.id
  const unread = (conv.unreadCount ?? 0) > 0
  const tags = (conv['tags'] as string[] | undefined) ?? []

  return (
    <div
      className={`cs-conv-item${active ? ' cs-conv-item--active' : ''}`}
      onClick={onClick}
      style={{ cursor: 'pointer', position: 'relative' }}
    >
      {/* Avatar + channel badge */}
      <div className="cs-conv-av">
        <Avatar name={name} size="md" />
        <span className="cs-conv-chan">
          <ChanGlyph type={chanKey} size={18} />
        </span>
      </div>

      {/* Body */}
      <div className="cs-conv-body">
        <div className="cs-conv-row1">
          <span className="cs-conv-name">{name}</span>
          <span className="cs-conv-time">{formatTime(conv.lastMessageAt)}</span>
        </div>
        <div
          className="cs-conv-prev"
          style={unread ? { color: 'var(--ink-2)', fontWeight: 600 } : undefined}
        >
          {conv.lastMessage ?? ''}
          {unread && <span className="cs-conv-unread" style={{ marginLeft: 6 }} />}
        </div>
        {tags.length > 0 && (
          <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap', marginTop: 4 }}>
            {tags.map((t) => <Tag key={t} label={t} />)}
          </div>
        )}
      </div>
    </div>
  )
}

// ── Column 1: Conversation list ───────────────────────────────────────────────

function ConvList({
  items,
  busy,
  selectedId,
  onSelect,
}: {
  items: Conversation[]
  busy: boolean
  selectedId: string | undefined
  onSelect: (id: string) => void
}) {
  const [tab, setTab] = useState<TabKey>('all')
  const [search, setSearch] = useState('')

  const myId = session.identity()?.sub

  const filtered = items.filter((c) => {
    // Tab filter
    if (tab === 'unread' && !((c.unreadCount ?? 0) > 0)) return false
    if (tab === 'mine' && c['assigneeId'] !== myId && c['agentId'] !== myId) {
      // best-effort: show all if field not present
      if ('assigneeId' in c || 'agentId' in c) return false
    }
    // follow: no specific backend field, keep all
    // Search filter
    if (search) {
      const q = search.toLowerCase()
      const name = (c.customerName ?? '').toLowerCase()
      const preview = (c.lastMessage ?? '').toLowerCase()
      if (!name.includes(q) && !preview.includes(q)) return false
    }
    return true
  })

  return (
    <div className="cs-conv-list">
      {/* Head */}
      <div className="cs-conv-head">
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 10 }}>
          <span style={{ fontSize: 17, fontWeight: 700, color: 'var(--ink)' }}>對話收件匣</span>
          <button className="cs-icon-btn" aria-label="篩選" style={{ width: 34, height: 34 }}>
            <Icon name="filter" w={17} />
          </button>
        </div>
        {/* Search */}
        <div style={{ position: 'relative', marginBottom: 0 }}>
          <Icon
            name="search"
            w={15}
            style={{
              position: 'absolute',
              left: 10,
              top: '50%',
              transform: 'translateY(-50%)',
              color: 'var(--muted)',
              pointerEvents: 'none',
            }}
          />
          <input
            type="search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="搜尋對話…"
            style={{
              width: '100%',
              paddingLeft: 32,
              paddingRight: 10,
              paddingTop: 7,
              paddingBottom: 7,
              fontSize: 13,
              background: 'var(--bg)',
              border: '1px solid var(--line)',
              borderRadius: 9,
              color: 'var(--ink)',
              outline: 'none',
              boxSizing: 'border-box',
            }}
          />
        </div>
        {/* Tabs */}
        <div className="cs-conv-tabs">
          {TABS.map((t) => (
            <button
              key={t.key}
              className={`cs-conv-tab${tab === t.key ? ' cs-conv-tab--active' : ''}`}
              onClick={() => setTab(t.key)}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {/* List */}
      <div style={{ flex: 1, overflowY: 'auto' }}>
        {busy && filtered.length === 0 && (
          <p style={{ color: 'var(--muted)', fontSize: 13, padding: '16px 20px' }}>載入中…</p>
        )}
        {!busy && filtered.length === 0 && (
          <p style={{ color: 'var(--muted)', fontSize: 13, padding: '16px 20px' }}>沒有對話</p>
        )}
        {filtered.map((c) => (
          <ConvItem
            key={c.id}
            conv={c}
            active={c.id === selectedId}
            onClick={() => onSelect(c.id)}
          />
        ))}
      </div>
    </div>
  )
}

// ── Column 2: Thread ──────────────────────────────────────────────────────────

function Thread({
  convId,
  meta,
  onMetaLoaded,
}: {
  convId: string | undefined
  meta: ConvMeta
  onMetaLoaded: (m: ConvMeta) => void
}) {
  const [messages, setMessages] = useState<Message[]>([])
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [assignMode, setAssignMode] = useState<AssignMode | null>(null)
  const bottom = useRef<HTMLDivElement>(null)

  // Load conversation meta + messages on convId change
  useEffect(() => {
    if (!convId) { setMessages([]); return }
    // Fetch meta (platform, platformUserId, teamId, customerId)
    void get<{
      platform?: string
      platformUserId?: string
      teamId?: number | null
      customerId?: number | null
      customerName?: string
    }>(`/api/conversations/${convId}`).then((resp) => {
      if (resp.success && resp.data) {
        onMetaLoaded({
          platform: resp.data.platform,
          platformUserId: resp.data.platformUserId,
          teamId: resp.data.teamId ?? null,
          customerId: resp.data.customerId ?? null,
          customerName: resp.data.customerName,
        })
      }
    })
    // Fetch messages
    void get<{ items?: Message[]; messages?: Message[] }>(
      `/api/conversations/${convId}/messages`,
    ).then((resp) => {
      if (resp.success && resp.data) {
        const items = resp.data.items ?? resp.data.messages ?? []
        setMessages([...items].reverse())
      } else {
        setError(resp.message ?? null)
      }
    })
    subscribeConversation(convId)
    return onEvent('new_message', (payload) => {
      if (String(payload.conversationId) !== convId) return
      const m = (payload.message ?? {}) as Record<string, unknown>
      setMessages((prev) =>
        prev.some((x) => x.id === m.id)
          ? prev
          : [...prev, {
              id: String(m.id ?? crypto.randomUUID()),
              content: String(m.content ?? ''),
              senderType: String(m.senderType ?? 'customer'),
              createdAt: String(m.timestamp ?? ''),
            }],
      )
    })
  }, [convId]) // onMetaLoaded intentionally omitted — stable callback ref

  useEffect(() => {
    bottom.current?.scrollIntoView({ behavior: 'smooth' })
  }, [messages.length])

  const send = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!convId || !draft.trim()) return
    const text = draft.trim()
    setDraft('')
    const tempId = `pending-${Date.now()}`
    const who = session.identity()
    setMessages((prev) => [...prev, {
      id: tempId, content: text, senderType: 'agent',
      senderName: who?.displayName, pending: true,
    }])
    const resp = await post<{ message?: Message; id?: string }>(
      `/api/conversations/${convId}/messages`,
      { content: text },
    )
    if (resp.success) {
      const confirmed = resp.data?.message ?? { id: resp.data?.id ?? tempId, content: text }
      setMessages((prev) => prev.map((m) => (m.id === tempId ? { ...m, ...confirmed, pending: false } : m)))
    } else {
      setMessages((prev) => prev.filter((m) => m.id !== tempId))
      setError(resp.message ?? null)
      setDraft(text)
    }
  }

  const currentTeamId =
    meta.teamId ??
    ((conversationsStore.get().items.find((c) => c.id === convId)?.teamId ?? null) as number | null)

  // Compute day separator groups
  const messagesWithSeps: Array<{ type: 'sep'; label: string } | { type: 'msg'; msg: Message }> = []
  let lastDay = ''
  for (const msg of messages) {
    const day = msg.createdAt ? new Date(msg.createdAt).toDateString() : ''
    if (day && day !== lastDay) {
      messagesWithSeps.push({ type: 'sep', label: dayLabel(msg.createdAt) })
      lastDay = day
    }
    messagesWithSeps.push({ type: 'msg', msg })
  }

  if (!convId) {
    return (
      <div className="cs-thread" style={{ alignItems: 'center', justifyContent: 'center' }}>
        <div style={{ textAlign: 'center', color: 'var(--muted)', fontSize: 15 }}>
          選擇一則對話開始
        </div>
      </div>
    )
  }

  const chanKey = channelOf(meta.platform ?? 'chat')
  const chanDef = CHANNELS[chanKey]
  const customerName = meta.customerName ?? ''

  return (
    <div className="cs-thread">
      {/* Thread head */}
      <div className="cs-thread-head">
        {/* Left: avatar + channel + name */}
        <div style={{ position: 'relative', flexShrink: 0 }}>
          <Avatar name={customerName || '?'} size="md" />
          <span style={{ position: 'absolute', bottom: -2, right: -4 }}>
            <ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb'} size={17} />
          </span>
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontSize: 15.5, fontWeight: 700, color: 'var(--ink)' }}>
              {customerName || convId}
            </span>
          </div>
          <div style={{ fontSize: 12, color: 'var(--muted)', marginTop: 1 }}>
            透過 {chanDef?.name ?? chanKey}
          </div>
        </div>
        {/* Right: action buttons */}
        <div style={{ display: 'flex', gap: 6 }}>
          <button className="cs-icon-btn" aria-label="電話" style={{ width: 36, height: 36 }}>
            <Icon name="phone" w={17} />
          </button>
          <button className="cs-icon-btn" aria-label="星號" style={{ width: 36, height: 36 }}>
            <Icon name="star" w={17} />
          </button>
          <button
            className="cs-icon-btn"
            aria-label="指派"
            title="指派"
            style={{ width: 36, height: 36 }}
            onClick={() => setAssignMode('assign')}
          >
            <Icon name="user" w={17} />
          </button>
          <button className="cs-icon-btn" aria-label="更多" style={{ width: 36, height: 36 }}>
            <Icon name="dots" w={17} />
          </button>
        </div>
      </div>

      {/* Thread body — scrollable message area */}
      <div className="cs-thread-body" style={{ overflowY: 'auto' }}>
        {error && (
          <p role="alert" style={{ color: 'crimson', fontSize: 13 }}>{error}</p>
        )}
        {messagesWithSeps.map((item, i) => {
          if (item.type === 'sep') {
            return <div key={`sep-${i}`} className="cs-day-sep">{item.label}</div>
          }
          const msg = item.msg
          const isMe = msg.senderType === 'agent'
          return (
            <div
              key={msg.id}
              className={`cs-bubble-row${isMe ? ' cs-bubble-row--me' : ''}`}
              style={{ opacity: msg.pending ? 0.55 : 1 }}
            >
              {!isMe && (
                <Avatar name={customerName || '?'} size="sm" />
              )}
              <div>
                <div className={`cs-bubble${isMe ? ' cs-bubble--me' : ''}`}>
                  {msg.content}
                </div>
                <div
                  className="cs-bubble-time"
                  style={{ textAlign: isMe ? 'right' : 'left' }}
                >
                  {msg.createdAt
                    ? new Date(msg.createdAt).toLocaleTimeString('zh-TW', {
                        hour: '2-digit',
                        minute: '2-digit',
                        hour12: false,
                      })
                    : ''}
                  {isMe && !msg.pending && ' · 已讀'}
                </div>
              </div>
            </div>
          )
        })}
        <div ref={bottom} />
      </div>

      {/* Composer */}
      <div className="cs-composer">
        <form onSubmit={(e) => void send(e)}>
          <div className="cs-composer-box">
            <textarea
              className="cs-composer-input"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.shiftKey) {
                  e.preventDefault()
                  void send(e as unknown as React.FormEvent)
                }
              }}
              placeholder="輸入訊息，或按「/」插入罐頭回覆…"
              rows={1}
              style={{
                width: '100%',
                border: 'none',
                outline: 'none',
                resize: 'none',
                background: 'transparent',
                fontFamily: 'inherit',
              }}
            />
            <div className="cs-composer-tools">
              <button type="button" className="cs-composer-ico" aria-label="附件">
                <Icon name="paperclip" w={17} />
              </button>
              <button type="button" className="cs-composer-ico" aria-label="表情">
                <Icon name="emoji" w={17} />
              </button>
              <button type="button" className="cs-composer-ico" aria-label="快捷回覆">
                <Icon name="zap" w={17} />
              </button>
              <span style={{ flex: 1 }} />
              <button
                type="button"
                onClick={() => setAssignMode('assign')}
                className="cs-chip cs-chip--blue"
                style={{ cursor: 'pointer', border: 'none' }}
              >
                指派給我
              </button>
              <button
                type="submit"
                className="cs-btn cs-btn--primary"
                disabled={!draft.trim()}
                style={{ display: 'flex', alignItems: 'center', gap: 6 }}
              >
                <Icon name="send" w={15} />
                傳送
              </button>
            </div>
          </div>
        </form>
      </div>

      {/* Assign dialog */}
      {convId && assignMode && (
        <AssignDialog
          open
          mode={assignMode}
          conversationId={convId}
          currentTeamId={currentTeamId}
          onClose={() => setAssignMode(null)}
        />
      )}
    </div>
  )
}

// ── Column 3: Customer panel ──────────────────────────────────────────────────

function CustPanel({ meta }: { meta: ConvMeta }) {
  const [customer, setCustomer] = useState<Customer | null>(null)
  const [tags, setTags] = useState<CustomerTag[]>([])
  const [convCount, setConvCount] = useState<number | null>(null)

  const customerId = meta.customerId

  useEffect(() => {
    if (!customerId) {
      setCustomer(null)
      setTags([])
      setConvCount(null)
      return
    }
    void loadCustomerDetail(customerId).then((detail) => {
      if (!detail) return
      setCustomer(detail.customer)
      setConvCount(detail.conversationCount)
    })
    void loadCustomerTags(customerId).then(setTags)
  }, [customerId])

  // If no meta yet, show a placeholder
  if (!meta.platform && !meta.customerName) {
    return (
      <div className="cs-cust" style={{ alignItems: 'center', justifyContent: 'center' }}>
        <span style={{ color: 'var(--muted)', fontSize: 13 }}>選擇對話以查看客戶資訊</span>
      </div>
    )
  }

  const name = customer?.display_name ?? meta.customerName ?? ''
  const email = customer?.email
  const phone = customer?.phone
  const platform = customer?.platform ?? meta.platform ?? ''
  const platformUserId = customer?.platform_user_id ?? meta.platformUserId ?? ''
  const chanKey = channelOf(platform)
  const chanDef = CHANNELS[chanKey]

  return (
    <div className="cs-cust" style={{ overflowY: 'auto' }}>
      {/* Top: avatar + name + ID */}
      <div style={{ textAlign: 'center', display: 'flex', flexDirection: 'column', alignItems: 'center', gap: 8 }}>
        <Avatar name={name || '?'} size="lg" ring />
        <div style={{ fontSize: 16, fontWeight: 700, color: 'var(--ink)' }}>{name}</div>
        {(customer?.id || platformUserId) && (
          <div style={{ fontSize: 12.5, color: 'var(--muted)' }}>
            {customer?.id ? `會員編號 #C-${customer.id}` : platformUserId}
          </div>
        )}
        {tags.length > 0 && (
          <div style={{ display: 'flex', gap: 4, flexWrap: 'wrap', justifyContent: 'center' }}>
            {tags.map((t) => <Tag key={t.id} label={t.name} />)}
          </div>
        )}
      </div>

      <hr style={{ border: 'none', borderTop: '1px solid var(--line)', margin: 0 }} />

      {/* Contact info */}
      <div>
        <div className="cs-cust-block-label">聯絡資訊</div>
        {email && (
          <div className="cs-kv">
            <span className="cs-kv-k">電子郵件</span>
            <span className="cs-kv-v cs-mono" style={{ fontSize: 12 }}>{email}</span>
          </div>
        )}
        {phone && (
          <div className="cs-kv">
            <span className="cs-kv-k">電話</span>
            <span className="cs-kv-v cs-mono">{phone}</span>
          </div>
        )}
        {chanDef && (
          <div className="cs-kv">
            <span className="cs-kv-k">偏好渠道</span>
            <span className="cs-kv-v" style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
              <ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb'} size={14} />
              {chanDef.name}
            </span>
          </div>
        )}
        {!email && !phone && !chanDef && (
          <p style={{ fontSize: 13, color: 'var(--muted)' }}>無聯絡資料</p>
        )}
      </div>

      {/* Stats — only if real data available */}
      {convCount !== null && (
        <>
          <hr style={{ border: 'none', borderTop: '1px solid var(--line)', margin: 0 }} />
          <div>
            <div className="cs-cust-block-label">統計</div>
            <div style={{ display: 'flex', gap: 0 }}>
              <div style={{ flex: 1, textAlign: 'center', borderRight: '1px solid var(--line)', paddingRight: 8 }}>
                <div style={{ fontSize: 20, fontWeight: 700, color: 'var(--ink)' }}>{convCount}</div>
                <div style={{ fontSize: 11.5, color: 'var(--muted)', marginTop: 2 }}>歷史對話</div>
              </div>
            </div>
          </div>
        </>
      )}
    </div>
  )
}

// ── Main Inbox page ───────────────────────────────────────────────────────────

export default function Inbox() {
  const { id: paramId } = useParams<{ id?: string }>()
  const navigate = useNavigate()
  const { items, busy } = useStore(conversationsStore)
  const [selectedId, setSelectedId] = useState<string | undefined>(paramId)
  const [meta, setMeta] = useState<ConvMeta>({})

  // Load conversation list on mount
  useEffect(() => {
    void loadConversations()
  }, [])

  // Keep selectedId in sync with route param changes
  useEffect(() => {
    if (paramId && paramId !== selectedId) setSelectedId(paramId)
  }, [paramId])

  const handleSelect = useCallback((id: string) => {
    setSelectedId(id)
    setMeta({}) // clear stale meta while new one loads
    void markConversationRead(id)
    navigate(`/conversations/${id}`, { replace: true })
  }, [navigate])

  const handleMetaLoaded = useCallback((m: ConvMeta) => {
    setMeta(m)
  }, [])

  return (
    // .cs-inbox escapes .cs-content's 28px/32px padding so it can fill the
    // full available viewport height without extra whitespace. The negative
    // margins + extra size exactly cancel the parent padding on all four sides.
    <div
      className="cs-inbox"
      style={{
        margin: '-28px -32px',
        height: 'calc(100% + 56px)',
      }}
    >
      <ConvList
        items={items}
        busy={busy}
        selectedId={selectedId}
        onSelect={handleSelect}
      />
      <Thread
        convId={selectedId}
        meta={meta}
        onMetaLoaded={handleMetaLoaded}
      />
      <CustPanel meta={meta} />
    </div>
  )
}
