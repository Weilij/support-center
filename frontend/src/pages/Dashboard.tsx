// Dashboard landing screen: greeting + system stats + recent conversations + team status.

import { useEffect, useState } from 'react'
import { Link } from 'react-router-dom'

import { get } from '../api/client'
import { session } from '../auth/session'
import { PageHeader } from '../components/PageHeader'
import { Card, StatGrid } from '../components/Card'
import { StatCard, StatusPill } from '../components/ui'
import { conversationsStore, loadConversations } from '../stores/conversations'
import { teamsStore, loadTeams } from '../stores/teams'
import { useStore } from '../stores/store'

interface SystemStats {
  totalConversations: number
  totalMessages: number
  totalCustomers: number
}

function greeting(name: string | undefined): string {
  const hour = new Date().getHours()
  const salutation = hour < 12 ? '早安' : hour < 18 ? '午安' : '晚安'
  return name ? `${salutation}，${name}` : salutation
}

export default function Dashboard() {
  const [stats, setStats] = useState<SystemStats | null>(null)
  const who = session.identity()

  const { items: conversations } = useStore(conversationsStore)
  const { items: teams } = useStore(teamsStore)

  useEffect(() => {
    void get<SystemStats>('/api/system/stats').then((resp) => {
      if (resp.success && resp.data) setStats(resp.data)
    })
    void loadConversations()
    void loadTeams()
  }, [])

  const recentConversations = conversations.slice(0, 6)

  return (
    <main style={{ maxWidth: 1100, margin: '0 auto' }}>
      <PageHeader
        title="儀表板"
        subtitle={greeting(who?.displayName ?? who?.email)}
      />

      {/* Stat cards row */}
      <div style={{ marginBottom: 'var(--sp-5)' }}>
        <StatGrid>
          <StatCard
            label="對話總數"
            value={stats ? (stats.totalConversations).toLocaleString() : '—'}
          />
          <StatCard
            label="訊息總數"
            value={stats ? (stats.totalMessages).toLocaleString() : '—'}
          />
          <StatCard
            label="客戶總數"
            value={stats ? (stats.totalCustomers).toLocaleString() : '—'}
          />
        </StatGrid>
      </div>

      {/* Two-column panel area */}
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: '1.6fr 1fr',
          gap: 'var(--sp-4)',
        }}
      >
        {/* Recent conversations */}
        <Card
          title="最近對話"
          actions={
            <Link
              to="/conversations"
              style={{ fontSize: 13, color: 'var(--muted)', textDecoration: 'none' }}
            >
              查看全部
            </Link>
          }
        >
          {recentConversations.length === 0 ? (
            <p style={{ color: 'var(--muted)', fontSize: 14, margin: 0 }}>尚無對話紀錄</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              {recentConversations.map((c) => (
                <Link
                  key={c.id}
                  to={`/conversations/${c.id}`}
                  style={{ textDecoration: 'none', color: 'inherit' }}
                >
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 8,
                      padding: '8px 6px',
                      borderRadius: 6,
                      cursor: 'pointer',
                    }}
                  >
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ fontWeight: 500, fontSize: 14 }}>
                        {c.customerName ?? c.id}
                      </div>
                      {c.lastMessage && (
                        <div
                          style={{
                            fontSize: 12,
                            color: 'var(--muted)',
                            overflow: 'hidden',
                            whiteSpace: 'nowrap',
                            textOverflow: 'ellipsis',
                          }}
                        >
                          {c.lastMessage}
                        </div>
                      )}
                    </div>
                    <StatusPill status={c.status} />
                  </div>
                </Link>
              ))}
            </div>
          )}
        </Card>

        {/* Team status */}
        <Card title="團隊狀態">
          {teams.length === 0 ? (
            <p style={{ color: 'var(--muted)', fontSize: 14, margin: 0 }}>尚無團隊資料</p>
          ) : (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
              {teams.map((t) => (
                <div
                  key={t.id}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 8,
                    padding: '6px 0',
                    borderBottom: '1px solid var(--surface-border)',
                  }}
                >
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontWeight: 500, fontSize: 14 }}>{t.name}</div>
                    {t.memberCount !== undefined && (
                      <div style={{ fontSize: 12, color: 'var(--muted)' }}>
                        {t.memberCount} 人
                      </div>
                    )}
                  </div>
                  <StatusPill
                    status={t.isActive ? 'active' : 'inactive'}
                    label={t.isActive ? '啟用' : '停用'}
                  />
                </div>
              ))}
            </div>
          )}
        </Card>
      </div>
    </main>
  )
}
