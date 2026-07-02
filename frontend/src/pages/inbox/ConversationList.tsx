import { useLayoutEffect, useRef, useState } from 'react'

import { Avatar } from '../../components/Avatar'
import { ChanGlyph } from '../../components/ChanGlyph'
import { Tag } from '../../components/Chip'
import { Icon } from '../../components/Icon'
import { channelOf } from '../../components/channels'
import { recordPositions, animateMoves } from '../../lib/flip'
import { session } from '../../auth/session'
import type { Conversation } from '../../stores/conversations'

type TabKey = 'all' | 'unread' | 'team'

const TABS: { key: TabKey; label: string }[] = [
  { key: 'all', label: '全部' },
  { key: 'unread', label: '未讀' },
  { key: 'team', label: '我的團隊' },
]

function formatTime(iso?: string): string {
  if (!iso) return ''
  const d = new Date(iso)
  if (Number.isNaN(d.getTime())) return ''
  const now = new Date()
  const diffMs = now.getTime() - d.getTime()
  const diffDays = Math.floor(diffMs / 86400000)
  if (diffDays === 0) {
    return d.toLocaleTimeString('zh-TW', {
      hour: '2-digit',
      minute: '2-digit',
      hour12: false,
    })
  }
  if (diffDays === 1) return '昨天'
  if (diffDays < 7) {
    const day = ['日', '一', '二', '三', '四', '五', '六'][d.getDay()]
    return day !== undefined ? `週${day}` : ''
  }
  return d.toLocaleDateString('zh-TW', { month: 'numeric', day: 'numeric' })
}

function isMyTeam(c: Conversation): boolean {
  const myIds = session.teamOptions().map((t) => String(t.id))
  if (session.isAdmin() || myIds.length === 0) {
    const assignedTeam = c['assignedTeam']
    return c.teamId != null || (assignedTeam !== null && assignedTeam !== undefined)
  }
  return c.teamId != null && myIds.includes(String(c.teamId))
}

function ConversationItem({
  conv,
  active,
  onClick,
}: {
  conv: Conversation
  active: boolean
  onClick: () => void
}) {
  const platform = String(conv.platform ?? conv['channel'] ?? 'chat')
  const chanKey = channelOf(platform) as 'chat' | 'line' | 'wa' | 'fb' | 'ig' | 'shopee'
  const name = conv.customerName ?? conv.id
  const unread = (conv.unreadCount ?? 0) > 0
  const tags = (conv['tags'] as string[] | undefined) ?? []

  return (
    <div
      data-flip-id={conv.id}
      className={`cs-conv-item${active ? ' cs-conv-item--active' : ''}`}
      onClick={onClick}
      style={{ cursor: 'pointer', position: 'relative' }}
    >
      <div className="cs-conv-av">
        <Avatar name={name} src={conv.customerAvatarUrl as string | undefined} size="md" />
        <span className="cs-conv-chan">
          <ChanGlyph type={chanKey} size={18} />
        </span>
      </div>

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
            {tags.map((tag) => (
              <Tag key={tag} label={tag} />
            ))}
          </div>
        )}
      </div>
    </div>
  )
}

export function ConversationList({
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
  const listRef = useRef<HTMLDivElement>(null)
  const prevPos = useRef<ReturnType<typeof recordPositions> | null>(null)

  const filtered = items.filter((conversation) => {
    if (tab === 'unread' && !((conversation.unreadCount ?? 0) > 0)) return false
    if (tab === 'team' && !isMyTeam(conversation)) return false
    if (search) {
      const q = search.toLowerCase()
      const name = (conversation.customerName ?? '').toLowerCase()
      const preview = (conversation.lastMessage ?? '').toLowerCase()
      if (!name.includes(q) && !preview.includes(q)) return false
    }
    return true
  })

  const orderKey = filtered.map((conversation) => conversation.id).join(',')
  useLayoutEffect(() => {
    const reduce = window.matchMedia?.('(prefers-reduced-motion: reduce)').matches
    if (!reduce && listRef.current && prevPos.current) {
      animateMoves(listRef.current, prevPos.current)
    }
    if (listRef.current) prevPos.current = recordPositions(listRef.current)
  }, [orderKey])

  return (
    <div className="cs-conv-list" style={fullWidth ? { width: '100%', flexShrink: 1 } : undefined}>
      <div className="cs-conv-head">
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 10 }}>
          <span style={{ fontSize: 17, fontWeight: 700, color: 'var(--ink)' }}>對話收件匣</span>
          <button className="cs-icon-btn" aria-label="篩選" title="篩選" style={{ width: 34, height: 34 }}>
            <Icon name="filter" w={18} />
          </button>
        </div>

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
            data-inbox-search
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

        <div className="cs-conv-tabs">
          {TABS.map((tabOption) => (
            <button
              key={tabOption.key}
              className={`cs-conv-tab${tab === tabOption.key ? ' cs-conv-tab--active' : ''}`}
              onClick={() => setTab(tabOption.key)}
            >
              {tabOption.label}
            </button>
          ))}
        </div>
      </div>

      <div ref={listRef} style={{ flex: 1, overflowY: 'auto' }}>
        {busy && filtered.length === 0 && (
          <p style={{ color: 'var(--muted)', fontSize: 13, padding: '16px 20px' }}>載入中…</p>
        )}
        {!busy && filtered.length === 0 && (
          <p style={{ color: 'var(--muted)', fontSize: 13, padding: '16px 20px' }}>沒有對話</p>
        )}
        {filtered.map((conversation) => (
          <ConversationItem
            key={conversation.id}
            conv={conversation}
            active={conversation.id === selectedId}
            onClick={() => onSelect(conversation.id)}
          />
        ))}
      </div>
    </div>
  )
}
