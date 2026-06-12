// Dashboard shell: greets the identity and shows live system stats.

import { useEffect, useState } from 'react'

import { get } from '../api/client'
import { session } from '../auth/session'
import { t } from '../i18n'

export default function Dashboard() {
  const [stats, setStats] = useState<Record<string, unknown> | null>(null)
  useEffect(() => {
    void get<Record<string, unknown>>('/api/system/stats').then((resp) => {
      if (resp.success && resp.data) setStats(resp.data)
    })
  }, [])
  const who = session.identity()
  return (
    <main style={{ maxWidth: 720, margin: '5vh auto', fontFamily: 'sans-serif' }}>
      <h1>{t('dashboard.title')}</h1>
      <p>{who?.displayName ?? who?.email}</p>
      {stats && (
        <ul>
          <li>對話: {String(stats.totalConversations ?? 0)}</li>
          <li>訊息: {String(stats.totalMessages ?? 0)}</li>
          <li>客戶: {String(stats.totalCustomers ?? 0)}</li>
        </ul>
      )}
    </main>
  )
}
