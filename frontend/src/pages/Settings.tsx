// System settings screen (CRD §8.2): general group editing over the merged
// settings tree; saves via PUT /api/system/settings.

import { useEffect, useState } from 'react'

import { get, put } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'

interface GeneralSettings {
  systemName?: string
  contactEmail?: string
  timezone?: string
  language?: string
}

export default function Settings() {
  const [general, setGeneral] = useState<GeneralSettings>({})
  const [message, setMessage] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    void get<{ general?: GeneralSettings }>('/api/system/settings').then((resp) => {
      if (resp.success && resp.data?.general) setGeneral(resp.data.general)
      else setError(resp.message ?? null)
    })
  }, [])

  const save = async (e: React.FormEvent) => {
    e.preventDefault()
    setMessage(null)
    setError(null)
    const resp = await put('/api/system/settings', { general })
    if (resp.success) setMessage(resp.message ?? '已儲存')
    else setError(resp.message ?? null)
  }

  const field = (key: keyof GeneralSettings, label: string) => (
    <label style={{ display: 'block', marginBottom: 8 }}>
      {label}
      <input
        value={general[key] ?? ''}
        onChange={(e) => setGeneral({ ...general, [key]: e.target.value })}
        style={{ width: '100%' }}
      />
    </label>
  )

  // Area gate AFTER all hooks (Rules of Hooks: stable hook order).
  if (!can(session.position(), 'system')) {
    return <main style={{ margin: '10vh auto', maxWidth: 480 }}><p>權限不足</p></main>
  }
  return (
    <div style={{ maxWidth: 480, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="系統設定" />
      {message && <p style={{ color: 'seagreen' }}>{message}</p>}
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
      <Card title="一般設定">
        <form onSubmit={save}>
          {field('systemName', '系統名稱')}
          {field('contactEmail', '聯絡信箱')}
          {field('timezone', '時區')}
          <label style={{ display: 'block', marginBottom: 8 }}>
            語言
            <select
              value={general.language ?? 'zh-TW'}
              onChange={(e) => setGeneral({ ...general, language: e.target.value })}
              style={{ width: '100%' }}
            >
              <option value="zh-TW">繁體中文</option>
              <option value="zh-CN">简体中文</option>
              <option value="en">English</option>
              <option value="ja">日本語</option>
            </select>
          </label>
          <button type="submit">儲存</button>
        </form>
      </Card>
    </div>
  )
}
