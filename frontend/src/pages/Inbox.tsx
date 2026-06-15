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
import { Drawer } from '../components/Modal'
import { FileUpload } from '../components/FileUpload'
import { Toast } from '../components/ui'
import {
  loadConversationFiles,
  uploadConversationFile,
  fileDownloadUrl,
  type Attachment,
} from '../stores/files'
import {
  loadPendingDelayed,
  scheduleDelayed,
  cancelDelayed,
  type PendingDelayed,
} from '../stores/delayedMessages'

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
  fullWidth,
}: {
  items: Conversation[]
  busy: boolean
  selectedId: string | undefined
  onSelect: (id: string) => void
  fullWidth?: boolean
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
    <div className="cs-conv-list" style={fullWidth ? { width: '100%', flexShrink: 1 } : undefined}>
      {/* Head */}
      <div className="cs-conv-head">
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 10 }}>
          <span style={{ fontSize: 17, fontWeight: 700, color: 'var(--ink)' }}>對話收件匣</span>
          <button className="cs-icon-btn" aria-label="篩選" title="篩選" style={{ width: 34, height: 34 }}>
            <Icon name="filter" w={18} />
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
  onBack,
  onToggleCustPanel,
  showCustToggle,
}: {
  convId: string | undefined
  meta: ConvMeta
  onMetaLoaded: (m: ConvMeta) => void
  onBack?: () => void
  onToggleCustPanel?: () => void
  showCustToggle?: boolean
}) {
  const [messages, setMessages] = useState<Message[]>([])
  const [draft, setDraft] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [assignMode, setAssignMode] = useState<AssignMode | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const bottom = useRef<HTMLDivElement>(null)

  // ── Files drawer state ──────────────────────────────────────────────────────
  const [showFiles, setShowFiles] = useState(false)
  const [files, setFiles] = useState<Attachment[]>([])

  const refreshFiles = useCallback(async () => {
    if (!convId) return
    setFiles(await loadConversationFiles(convId))
  }, [convId])

  useEffect(() => {
    if (showFiles) void refreshFiles()
  }, [showFiles, convId, refreshFiles])

  // ── Schedule drawer state ───────────────────────────────────────────────────
  const [showSchedule, setShowSchedule] = useState(false)
  const [pending, setPending] = useState<PendingDelayed[]>([])
  const [schedDraft, setSchedDraft] = useState('')
  const [delayMin, setDelayMin] = useState(5)
  const [schedMsg, setSchedMsg] = useState<string | null>(null)

  const refreshPending = useCallback(async () => {
    if (!convId) return
    setPending(await loadPendingDelayed(convId))
  }, [convId])

  useEffect(() => {
    if (showSchedule) void refreshPending()
  }, [showSchedule, convId, refreshPending])

  const submitSchedule = async () => {
    if (!convId || !schedDraft.trim()) return
    if (!meta.platform || !meta.platformUserId) {
      setSchedMsg('缺少客戶平台資訊，無法排程')
      return
    }
    const res = await scheduleDelayed({
      conversationId: convId,
      content: schedDraft.trim(),
      platform: meta.platform,
      userId: meta.platformUserId,
      delaySeconds: Math.max(1, Math.round(delayMin * 60)),
    })
    setSchedMsg(res.ok ? '已排程' : res.message ?? '排程失敗')
    if (res.ok) {
      setSchedDraft('')
      await refreshPending()
    }
  }

  // ── Drag-drop state ─────────────────────────────────────────────────────────
  const [dragOver, setDragOver] = useState(false)

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault()
    setDragOver(true)
  }
  const handleDragLeave = () => setDragOver(false)
  const handleDrop = async (e: React.DragEvent) => {
    e.preventDefault()
    setDragOver(false)
    if (!convId) return
    const file = e.dataTransfer.files[0]
    if (!file) return
    const { error: uploadErr } = await uploadConversationFile(convId, file)
    if (!uploadErr) {
      await refreshFiles()
      setToast(`已上傳 ${file.name}`)
    } else {
      setToast(`上傳失敗：${uploadErr}`)
    }
  }

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
        {/* Back button — narrow layout only */}
        {onBack && (
          <button
            className="cs-icon-btn"
            aria-label="返回列表"
            title="返回列表"
            onClick={onBack}
            style={{ width: 38, height: 38, marginRight: 4, transform: 'scaleX(-1)' }}
          >
            <Icon name="arrowRight" w={19} />
          </button>
        )}
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
        {/* Right: action buttons — only functional ones */}
        <div style={{ display: 'flex', gap: 6 }}>
          {/* Files drawer button */}
          <button
            className="cs-icon-btn"
            aria-label="檔案"
            title="檔案"
            style={{ width: 38, height: 38, position: 'relative' }}
            onClick={() => setShowFiles((v) => !v)}
          >
            <Icon name="paperclip" w={19} />
            {files.length > 0 && (
              <span style={{
                position: 'absolute',
                top: 4,
                right: 4,
                background: 'var(--brand, var(--blue-600))',
                color: '#fff',
                fontSize: 10,
                fontWeight: 700,
                borderRadius: 8,
                minWidth: 14,
                height: 14,
                lineHeight: '14px',
                textAlign: 'center',
                padding: '0 3px',
              }}>
                {files.length}
              </span>
            )}
          </button>
          {/* Schedule drawer button */}
          <button
            className="cs-icon-btn"
            aria-label="排程"
            title="排程"
            style={{ width: 38, height: 38, position: 'relative' }}
            onClick={() => setShowSchedule((v) => !v)}
          >
            <Icon name="clock" w={19} />
            {pending.length > 0 && (
              <span style={{
                position: 'absolute',
                top: 4,
                right: 4,
                background: 'var(--brand, var(--blue-600))',
                color: '#fff',
                fontSize: 10,
                fontWeight: 700,
                borderRadius: 8,
                minWidth: 14,
                height: 14,
                lineHeight: '14px',
                textAlign: 'center',
                padding: '0 3px',
              }}>
                {pending.length}
              </span>
            )}
          </button>
          {/* Assign button */}
          <button
            className="cs-icon-btn"
            aria-label="指派"
            title="指派"
            style={{ width: 38, height: 38 }}
            onClick={() => setAssignMode('assign')}
          >
            <Icon name="user" w={19} />
          </button>
          {/* Transfer button */}
          <button
            className="cs-icon-btn"
            aria-label="轉接"
            title="轉接"
            style={{ width: 38, height: 38 }}
            onClick={() => setAssignMode('transfer')}
          >
            <Icon name="arrowRight" w={19} />
          </button>
          {/* Customer panel toggle — medium/narrow layouts only */}
          {showCustToggle && (
            <button
              className="cs-icon-btn"
              aria-label="客戶資訊"
              title="客戶資訊"
              style={{ width: 38, height: 38 }}
              onClick={onToggleCustPanel}
            >
              <Icon name="pin" w={19} />
            </button>
          )}
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
      <div
        className="cs-composer"
        onDragOver={handleDragOver}
        onDragLeave={handleDragLeave}
        onDrop={(e) => void handleDrop(e)}
      >
        <form onSubmit={(e) => void send(e)}>
          <div
            className="cs-composer-box"
            style={dragOver ? {
              border: '2px dashed var(--blue-500)',
              position: 'relative',
            } : undefined}
          >
            {dragOver && (
              <div style={{
                position: 'absolute',
                inset: 0,
                background: 'rgba(14,165,233,.08)',
                borderRadius: 'inherit',
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'center',
                zIndex: 10,
                fontSize: 14,
                fontWeight: 600,
                color: 'var(--blue-600)',
                pointerEvents: 'none',
              }}>
                放開以上傳檔案到此對話
              </div>
            )}
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
                <Icon name="paperclip" w={20} />
              </button>
              <button type="button" className="cs-composer-ico" aria-label="表情">
                <Icon name="emoji" w={20} />
              </button>
              <button type="button" className="cs-composer-ico" aria-label="快捷回覆">
                <Icon name="zap" w={20} />
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
                <Icon name="send" w={18} />
                傳送
              </button>
            </div>
          </div>
        </form>
      </div>

      {/* Assign / Transfer dialog */}
      {convId && assignMode && (
        <AssignDialog
          open
          mode={assignMode}
          conversationId={convId}
          currentTeamId={currentTeamId}
          onClose={() => setAssignMode(null)}
        />
      )}

      {/* Files drawer */}
      <Drawer
        open={showFiles}
        title="附件檔案"
        onClose={() => setShowFiles(false)}
        width={420}
      >
        {convId && (
          <>
            <FileUpload
              label="拖放或點選上傳檔案到此對話"
              onUpload={async (file) => {
                const { error } = await uploadConversationFile(convId, file)
                if (!error) await refreshFiles()
                return error ?? null
              }}
            />
            <div style={{ marginTop: 12 }}>
              {files.length === 0 ? (
                <p style={{ color: 'var(--muted)', fontSize: 13, margin: 0 }}>尚無檔案</p>
              ) : (
                <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
                  {files.map((f) => (
                    <li
                      key={f.id}
                      style={{
                        display: 'flex',
                        gap: 8,
                        alignItems: 'center',
                        padding: '6px 0',
                        fontSize: 14,
                        borderBottom: '1px solid var(--line)',
                      }}
                    >
                      <Icon name="paperclip" w={14} style={{ flexShrink: 0, color: 'var(--muted)' }} />
                      <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        {f.originalName || f.filename || f.id}
                      </span>
                      {f.size != null && (
                        <span style={{ color: 'var(--muted)', fontSize: 12, flexShrink: 0 }}>
                          {Math.round(f.size / 1024)} KB
                        </span>
                      )}
                      <button
                        className="cs-btn"
                        style={{ flexShrink: 0, fontSize: 12, padding: '3px 10px' }}
                        onClick={async () => {
                          const url = (await fileDownloadUrl(f.id)) ?? f.publicUrl ?? f.url
                          if (url) window.open(url, '_blank')
                        }}
                      >
                        下載
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </>
        )}
      </Drawer>

      {/* Schedule drawer */}
      <Drawer
        open={showSchedule}
        title="排程訊息"
        onClose={() => setShowSchedule(false)}
        width={420}
      >
        {convId && (
          <>
            {(!meta.platform || !meta.platformUserId) ? (
              <p style={{ color: 'var(--muted)', fontSize: 13, margin: '0 0 12px' }}>
                缺少客戶平台資訊，無法排程
              </p>
            ) : (
              <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginBottom: 12 }}>
                <textarea
                  value={schedDraft}
                  onChange={(e) => setSchedDraft(e.target.value)}
                  placeholder="排程訊息內容"
                  rows={3}
                  style={{
                    width: '100%',
                    padding: '6px 8px',
                    fontSize: 14,
                    border: '1px solid var(--line)',
                    borderRadius: 8,
                    resize: 'vertical',
                    fontFamily: 'inherit',
                    boxSizing: 'border-box',
                  }}
                />
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <label style={{ fontSize: 13, color: 'var(--muted)', display: 'flex', alignItems: 'center', gap: 4 }}>
                    延遲
                    <input
                      type="number"
                      min={1}
                      value={delayMin}
                      onChange={(e) => setDelayMin(Number(e.target.value))}
                      style={{ width: 60, padding: '4px 6px', border: '1px solid var(--line)', borderRadius: 6, textAlign: 'center' }}
                    />
                    分鐘
                  </label>
                  <button
                    className="cs-btn cs-btn--primary"
                    disabled={!schedDraft.trim()}
                    style={{ marginLeft: 'auto' }}
                    onClick={() => void submitSchedule()}
                  >
                    排程送出
                  </button>
                </div>
                {schedMsg && (
                  <p style={{ fontSize: 13, color: 'var(--muted)', margin: 0 }}>{schedMsg}</p>
                )}
              </div>
            )}
            <hr style={{ border: 'none', borderTop: '1px solid var(--line)', margin: '0 0 12px' }} />
            {pending.length === 0 ? (
              <p style={{ color: 'var(--muted)', fontSize: 13, margin: 0 }}>無待送訊息</p>
            ) : (
              <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
                {pending.map((p) => (
                  <li
                    key={p.messageId}
                    style={{
                      display: 'flex',
                      gap: 8,
                      alignItems: 'center',
                      padding: '6px 0',
                      fontSize: 14,
                      borderBottom: '1px solid var(--line)',
                    }}
                  >
                    <span style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                      {p.preview || '(無內容)'}
                    </span>
                    <span style={{ color: 'var(--muted)', fontSize: 12, flexShrink: 0 }}>
                      {p.remainingMs != null ? `${Math.ceil(p.remainingMs / 1000)}s` : ''}
                    </span>
                    <button
                      className="cs-btn"
                      style={{ flexShrink: 0, fontSize: 12, padding: '3px 10px' }}
                      onClick={async () => {
                        if (await cancelDelayed(p.messageId)) await refreshPending()
                      }}
                    >
                      取消
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </Drawer>

      {/* Toast for drag-drop upload feedback */}
      <Toast message={toast} onDismiss={() => setToast(null)} />
    </div>
  )
}

// ── Column 3: Customer panel ──────────────────────────────────────────────────

function CustPanel({ meta, overlay, onClose }: { meta: ConvMeta; overlay?: boolean; onClose?: () => void }) {
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
      <div
        className="cs-cust"
        style={{
          alignItems: 'center',
          justifyContent: 'center',
          ...(overlay ? overlayPanelStyle : {}),
        }}
      >
        {overlay && onClose && (
          <button
            className="cs-icon-btn"
            onClick={onClose}
            style={{ position: 'absolute', top: 12, right: 12, width: 32, height: 32 }}
            title="關閉"
          >
            <Icon name="plus" w={16} style={{ transform: 'rotate(45deg)' }} />
          </button>
        )}
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
    <div
      className="cs-cust"
      style={{
        overflowY: 'auto',
        ...(overlay ? overlayPanelStyle : {}),
      }}
    >
      {/* Close button for overlay mode */}
      {overlay && onClose && (
        <button
          className="cs-icon-btn"
          onClick={onClose}
          style={{ position: 'absolute', top: 12, right: 12, width: 32, height: 32 }}
          title="關閉"
        >
          <Icon name="plus" w={16} style={{ transform: 'rotate(45deg)' }} />
        </button>
      )}

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

// Overlay style for the customer panel when used as a drawer (medium/narrow breakpoints)
const overlayPanelStyle: React.CSSProperties = {
  position: 'absolute',
  top: 0,
  right: 0,
  bottom: 0,
  zIndex: 100,
  boxShadow: 'var(--shadow-lg)',
  borderLeft: '1px solid var(--line)',
}

// ── Main Inbox page ───────────────────────────────────────────────────────────

export default function Inbox() {
  const { id: paramId } = useParams<{ id?: string }>()
  const navigate = useNavigate()
  const { items, busy } = useStore(conversationsStore)
  const [selectedId, setSelectedId] = useState<string | undefined>(paramId)
  const [meta, setMeta] = useState<ConvMeta>({})

  // ── RWD breakpoints ─────────────────────────────────────────────────────────
  const [isWide, setIsWide] = useState(() => window.matchMedia('(min-width: 1101px)').matches)
  const [isMedium, setIsMedium] = useState(() => window.matchMedia('(max-width: 1100px) and (min-width: 769px)').matches)
  const [isNarrow, setIsNarrow] = useState(() => window.matchMedia('(max-width: 768px)').matches)
  const [custPanelOpen, setCustPanelOpen] = useState(false)

  useEffect(() => {
    const wide   = window.matchMedia('(min-width: 1101px)')
    const medium = window.matchMedia('(max-width: 1100px) and (min-width: 769px)')
    const narrow = window.matchMedia('(max-width: 768px)')

    const onWide   = (e: MediaQueryListEvent) => { setIsWide(e.matches);   if (e.matches) { setIsMedium(false); setIsNarrow(false) } }
    const onMedium = (e: MediaQueryListEvent) => { setIsMedium(e.matches); if (e.matches) { setIsWide(false); setIsNarrow(false) } }
    const onNarrow = (e: MediaQueryListEvent) => { setIsNarrow(e.matches); if (e.matches) { setIsWide(false); setIsMedium(false) } }

    wide.addEventListener('change', onWide)
    medium.addEventListener('change', onMedium)
    narrow.addEventListener('change', onNarrow)
    return () => {
      wide.removeEventListener('change', onWide)
      medium.removeEventListener('change', onMedium)
      narrow.removeEventListener('change', onNarrow)
    }
  }, [])

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

  const handleBack = useCallback(() => {
    setSelectedId(undefined)
    navigate('/conversations', { replace: true })
  }, [navigate])

  // ── Layout helpers ──────────────────────────────────────────────────────────

  // Narrow: show list or thread, never both
  if (isNarrow) {
    const showList = !selectedId
    return (
      <div
        className="cs-inbox"
        style={{ margin: '-28px -32px', height: 'calc(100% + 56px)', position: 'relative' }}
      >
        {showList ? (
          <ConvList
            items={items}
            busy={busy}
            selectedId={selectedId}
            onSelect={handleSelect}
            fullWidth
          />
        ) : (
          <Thread
            convId={selectedId}
            meta={meta}
            onMetaLoaded={handleMetaLoaded}
            onBack={handleBack}
            onToggleCustPanel={() => setCustPanelOpen((v) => !v)}
            showCustToggle
          />
        )}
        {/* Customer panel as overlay drawer */}
        {custPanelOpen && selectedId && (
          <>
            {/* Dim backdrop */}
            <div
              onClick={() => setCustPanelOpen(false)}
              style={{
                position: 'absolute',
                inset: 0,
                background: 'rgba(15,23,42,.32)',
                zIndex: 99,
              }}
            />
            <CustPanel
              meta={meta}
              overlay
              onClose={() => setCustPanelOpen(false)}
            />
          </>
        )}
      </div>
    )
  }

  // Medium (≤ 1100px): conv list + thread side by side; customer panel as overlay
  if (isMedium) {
    return (
      <div
        className="cs-inbox"
        style={{ margin: '-28px -32px', height: 'calc(100% + 56px)', position: 'relative' }}
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
          onToggleCustPanel={() => setCustPanelOpen((v) => !v)}
          showCustToggle
        />
        {/* Customer panel overlay drawer */}
        {custPanelOpen && (
          <>
            <div
              onClick={() => setCustPanelOpen(false)}
              style={{
                position: 'absolute',
                inset: 0,
                background: 'rgba(15,23,42,.32)',
                zIndex: 99,
              }}
            />
            <CustPanel
              meta={meta}
              overlay
              onClose={() => setCustPanelOpen(false)}
            />
          </>
        )}
      </div>
    )
  }

  // Wide (> 1100px, including default): 3-column layout
  // isWide may be true OR both isMedium and isNarrow are false (SSR/init safety)
  void isWide // suppress unused-var lint in some setups
  return (
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
