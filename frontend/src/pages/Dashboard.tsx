// Dashboard — rebuilt to clean-light handoff spec (Task N4).
// Real data only: /api/system/stats, loadAgents(), loadStatusStatistics(),
// loadConversations(). No fabricated numbers or percentages.

import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'

import { get } from '../api/client'
import { Card } from '../components/Card'
import { Avatar } from '../components/Avatar'
import { ChanGlyph } from '../components/ChanGlyph'
import { Tag } from '../components/Chip'
import { KpiCard } from '../components/KpiCard'
import { Bar } from '../components/Bar'
import { StatusPill } from '../components/ui'
import { CHANNELS, channelOf } from '../components/channels'
import { conversationsStore, loadConversations } from '../stores/conversations'
import { loadAgents, loadStatusStatistics } from '../stores/agents'
import type { Agent } from '../stores/agents'
import type { Conversation } from '../stores/conversations'
import { useStore } from '../stores/store'

// ─────────────────────────────────────────────
// Types
// ─────────────────────────────────────────────
interface SystemStats {
  totalConversations: number
  totalMessages: number
  totalCustomers: number
}

// ─────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────

/** Derive per-channel counts + percentages from loaded conversations. */
function channelDistribution(items: Conversation[]) {
  const counts: Record<string, number> = {}
  for (const c of items) {
    const platform = (c as { platform?: string }).platform
    const key = platform ? channelOf(platform) : 'chat'
    counts[key] = (counts[key] ?? 0) + 1
  }
  const total = items.length
  return Object.entries(counts)
    .sort((a, b) => b[1] - a[1])
    .map(([key, count]) => ({
      key,
      name: CHANNELS[key]?.name ?? key,
      color: CHANNELS[key]?.color ?? '#64748b',
      count,
      pct: total > 0 ? Math.round((count / total) * 100) : 0,
    }))
}

/** Filter to "waiting" conversations: not closed, and has unread or high priority. */
function waitingConversations(items: Conversation[]): Conversation[] {
  return items
    .filter(
      (c) =>
        c.status !== 'closed' &&
        (((c.unreadCount ?? 0) > 0) || c.priority === 'high' || c.priority === 'urgent'),
    )
    .slice(0, 4)
}

// ─────────────────────────────────────────────
// Sub-components
// ─────────────────────────────────────────────

/** KPI row — 4 cards, real values only; no trend badges (no comparison data). */
function KpiRow({
  stats,
  presenceOnline,
  agentTotal,
}: {
  stats: SystemStats | null
  presenceOnline: number
  agentTotal: number
}) {
  return (
    <div className="grid-auto">
      <KpiCard
        icon="chat"
        iconBg="#e0f2fe"
        iconColor="#0284c7"
        label="對話總數"
        value={stats ? stats.totalConversations.toLocaleString() : '—'}
        unit="則"
      />
      <KpiCard
        icon="inbox"
        iconBg="#e0f2fe"
        iconColor="#0284c7"
        label="訊息總數"
        value={stats ? stats.totalMessages.toLocaleString() : '—'}
        unit="則"
      />
      <KpiCard
        icon="users"
        iconBg="#dcfce7"
        iconColor="#16a34a"
        label="客戶總數"
        value={stats ? stats.totalCustomers.toLocaleString() : '—'}
        unit="位"
      />
      <KpiCard
        icon="smile"
        iconBg="#fef3c7"
        iconColor="#d97706"
        label="在線客服"
        value={presenceOnline}
        unit="人"
        base={agentTotal > 0 ? `共 ${agentTotal} 位客服` : undefined}
      />
    </div>
  )
}

/** 渠道對話分佈 card — derived entirely from loaded conversations. */
function ChannelCard({ items }: { items: Conversation[] }) {
  const dist = channelDistribution(items)

  return (
    <Card
      title="渠道對話分佈"
      actions={
        <Link
          to="/conversations"
          style={{ fontSize: 12.5, color: 'var(--blue-600)', fontWeight: 600, textDecoration: 'none' }}
        >
          查看明細
        </Link>
      }
    >
      <p style={{ margin: '0 0 12px', fontSize: 12, color: 'var(--muted)', fontWeight: 500 }}>
        依目前載入的對話 · 共 {items.length} 則
      </p>
      {dist.length === 0 ? (
        <p style={{ color: 'var(--muted)', fontSize: 14, margin: 0 }}>尚無對話資料</p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column' }}>
          {dist.map(({ key, name, color, count, pct }) => (
            <div key={key} className="cs-chan-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 8, padding: '10px 0' }}>
              <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
                <ChanGlyph type={key as 'chat' | 'line' | 'wa' | 'fb'} size={26} />
                <span style={{ fontSize: 13.5, fontWeight: 600, flex: 1 }}>{name}</span>
                <span className="cs-mono" style={{ fontSize: 13, fontWeight: 700 }}>{count}</span>
                <span className="cs-mono" style={{ fontSize: 12, color: 'var(--muted)', width: 42, textAlign: 'right' }}>{pct}%</span>
              </div>
              <Bar pct={pct} color={color} />
            </div>
          ))}
        </div>
      )}
    </Card>
  )
}

/** 客服團隊狀態 card — real agents from loadAgents(); status from isActive. */
function TeamCard({
  agents,
  presenceOnline,
}: {
  agents: Agent[]
  presenceOnline: number
}) {
  return (
    <Card
      title="客服團隊狀態"
      actions={
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 12.5, fontWeight: 600, color: 'var(--ok)' }}>
          <span style={{ width: 7, height: 7, borderRadius: '50%', background: 'var(--ok)', display: 'inline-block' }} />
          {presenceOnline} 人線上
        </span>
      }
    >
      {agents.length === 0 ? (
        <p style={{ color: 'var(--muted)', fontSize: 14, margin: 0 }}>尚無客服資料</p>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
          {agents.map((a) => (
            <div
              key={a.id}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 10,
                padding: '9px 0',
                borderBottom: '1px solid var(--line-2)',
              }}
            >
              <Avatar name={a.displayName ?? a.email ?? a.id} size="sm" />
              <div style={{ flex: 1, minWidth: 0 }}>
                <div style={{ fontSize: 13.5, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {a.displayName ?? a.email ?? a.id}
                </div>
                {(a.role ?? a.position) && (
                  <div style={{ fontSize: 11.5, color: 'var(--muted)', marginTop: 1 }}>
                    {a.position ?? a.role}
                  </div>
                )}
              </div>
              <StatusPill
                status={a.isActive ? 'active' : 'inactive'}
                label={a.isActive ? '啟用' : '停用'}
              />
            </div>
          ))}
        </div>
      )}
    </Card>
  )
}

/** 待處理佇列 card — real conversations filtered to waiting state. */
function QueueCard({ items }: { items: Conversation[] }) {
  const queue = waitingConversations(items)

  return (
    <Card
      title={`待處理佇列 · ${queue.length} 則`}
      actions={
        <Link
          to="/conversations"
          style={{ fontSize: 12.5, color: 'var(--blue-600)', fontWeight: 600, textDecoration: 'none' }}
        >
          進入收件匣 →
        </Link>
      }
    >
      {queue.length === 0 ? (
        <p style={{ color: 'var(--muted)', fontSize: 14, margin: 0, padding: '8px 0' }}>目前無待處理對話</p>
      ) : (
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(180px, 1fr))',
          }}
        >
          {queue.map((c, idx) => {
            const platform = (c as { platform?: string }).platform
            const chanKey = platform ? channelOf(platform) : undefined
            const name = c.customerName ?? c.id
            const tags = (c as { tags?: string[] }).tags ?? []
            const isLast = idx === queue.length - 1

            return (
              <Link
                key={c.id}
                to={`/conversations/${c.id}`}
                style={{ textDecoration: 'none', color: 'inherit' }}
              >
                <div
                  style={{
                    padding: '14px 16px',
                    borderRight: isLast ? 'none' : '1px solid var(--line-2)',
                    minWidth: 0,
                  }}
                >
                  {/* Avatar + channel glyph overlay */}
                  <div style={{ position: 'relative', display: 'inline-block', marginBottom: 8 }}>
                    <Avatar name={name} size="sm" />
                    {chanKey && (
                      <span
                        style={{
                          position: 'absolute',
                          right: -3,
                          bottom: -3,
                          border: '2px solid var(--surface)',
                          borderRadius: '50%',
                          display: 'flex',
                          lineHeight: 0,
                        }}
                      >
                        <ChanGlyph type={chanKey as 'chat' | 'line' | 'wa' | 'fb'} size={16} />
                      </span>
                    )}
                  </div>

                  {/* Name + priority */}
                  <div style={{ display: 'flex', alignItems: 'center', gap: 6, flexWrap: 'wrap' }}>
                    <span style={{ fontSize: 13, fontWeight: 600 }}>{name}</span>
                    {(c.priority === 'high' || c.priority === 'urgent') && (
                      <span style={{ fontSize: 10.5, fontWeight: 600, color: 'var(--busy)' }}>
                        {c.priority === 'urgent' ? '緊急' : '高優先'}
                      </span>
                    )}
                  </div>

                  {/* Message preview */}
                  {c.lastMessage && (
                    <div
                      style={{
                        fontSize: 12.5,
                        color: 'var(--ink-2)',
                        marginTop: 4,
                        display: '-webkit-box',
                        WebkitLineClamp: 2,
                        WebkitBoxOrient: 'vertical',
                        overflow: 'hidden',
                        lineHeight: 1.45,
                      }}
                    >
                      {c.lastMessage}
                    </div>
                  )}

                  {/* Tags */}
                  {tags.length > 0 && (
                    <div style={{ marginTop: 6, display: 'flex', flexWrap: 'wrap', gap: 4 }}>
                      {tags.slice(0, 2).map((t) => (
                        <Tag key={t} label={t} />
                      ))}
                    </div>
                  )}
                </div>
              </Link>
            )
          })}
        </div>
      )}
    </Card>
  )
}

// ─────────────────────────────────────────────
// Main page
// ─────────────────────────────────────────────

export default function Dashboard() {
  const [stats, setStats] = useState<SystemStats | null>(null)
  const [agents, setAgents] = useState<Agent[]>([])
  const [presenceOnline, setPresenceOnline] = useState(0)

  const { items: conversations } = useStore(conversationsStore)

  useEffect(() => {
    // System stats
    void get<SystemStats>('/api/system/stats').then((resp) => {
      if (resp.success && resp.data) setStats(resp.data)
    })

    // Conversations
    void loadConversations()

    // Agents roster + presence
    void loadAgents().then(({ items }) => setAgents(items))
    void loadStatusStatistics().then((counts) => {
      setPresenceOnline(counts.online ?? 0)
    })
  }, [])

  return (
    <main
      style={{
        padding: '28px 32px',
        display: 'flex',
        flexDirection: 'column',
        gap: 20,
        minHeight: 0,
        overflowY: 'auto',
      }}
    >
      {/* 1. KPI row */}
      <KpiRow
        stats={stats}
        presenceOnline={presenceOnline}
        agentTotal={agents.length}
      />

      {/* 2. Two-column row: channel distribution + team status */}
      <div className="row-2col">
        <ChannelCard items={conversations} />
        <TeamCard agents={agents} presenceOnline={presenceOnline} />
      </div>

      {/* 3. Full-width queue */}
      <QueueCard items={conversations} />
    </main>
  )
}
