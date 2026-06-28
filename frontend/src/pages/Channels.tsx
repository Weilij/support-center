// Channel management screen (CRD §8.2, admin-flagged): per-platform credential
// entry for LINE / Facebook / Instagram, plus connection status and a live
// verification trigger. The backend stores secrets encrypted and never returns
// them — it only reports which secret field names are set (`credentialsSet`).

import { useEffect, useState } from 'react'

import { get, post, put } from '../api/client'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'

interface Channel {
  id: number
  platform: string
  isActive?: boolean
  isVerified?: boolean
  errorCount?: number
  config?: Record<string, unknown>
  credentialsSet?: string[]
}

// Per-platform form descriptor: the JSON body key the backend expects, the
// plain (non-secret) identifier fields, and the secret credential fields.
const PLATFORM_FORMS = {
  line: {
    key: 'lineConfig',
    label: 'LINE',
    plain: [
      ['channelId', 'Channel ID'],
      ['liffId', 'LIFF ID（選填）'],
    ],
    secret: [
      ['channelAccessToken', 'Channel access token'],
      ['channelSecret', 'Channel secret'],
    ],
  },
  facebook: {
    key: 'facebookConfig',
    label: 'Facebook',
    plain: [['pageId', 'Page ID']],
    secret: [
      ['accessToken', 'Page access token'],
      ['appSecret', 'App secret'],
    ],
  },
  instagram: {
    key: 'instagramConfig',
    label: 'Instagram',
    plain: [['igId', 'IG ID']],
    secret: [['accessToken', 'Access token']],
  },
} as const

type PlatformKey = keyof typeof PLATFORM_FORMS

const PLATFORM_KEYS = Object.keys(PLATFORM_FORMS) as PlatformKey[]

const inputStyle: React.CSSProperties = {
  display: 'block',
  width: '100%',
  marginTop: 4,
  padding: '6px 8px',
  boxSizing: 'border-box',
}

export default function Channels() {
  const [channels, setChannels] = useState<Channel[]>([])
  const [error, setError] = useState<string | null>(null)
  const [message, setMessage] = useState<string | null>(null)
  // One field bag per platform: { platform: { field: value } }.
  const [forms, setForms] = useState<Record<string, Record<string, string>>>({})

  const load = async () => {
    const resp = await get<Channel[]>('/api/channels')
    if (resp.success && Array.isArray(resp.data)) setChannels(resp.data)
    else if (resp.success) setChannels([])
    else setError(resp.message ?? null)
  }
  useEffect(() => {
    void load()
  }, [])

  const verify = async (id: number) => {
    setMessage(null)
    setError(null)
    const resp = await post(`/api/channels/${id}/verify`, {})
    if (resp.success) setMessage('驗證成功')
    else setError(resp.message ?? '驗證失敗')
    void load()
  }

  const setField = (platform: PlatformKey, field: string, value: string) => {
    setForms((prev) => ({ ...prev, [platform]: { ...prev[platform], [field]: value } }))
  }

  const save = async (platform: PlatformKey) => {
    setMessage(null)
    setError(null)
    const descriptor = PLATFORM_FORMS[platform]
    const existing = channels.find((c) => c.platform === platform)
    const bag = forms[platform] ?? {}

    // Build the config block from non-empty inputs. Plain fields fall back to the
    // already-stored value (so an untouched form still round-trips identifiers);
    // empty secret inputs are omitted so a blank never overwrites a stored secret.
    const filled: Record<string, string> = {}
    for (const [field] of descriptor.plain) {
      const typed = bag[field]?.trim()
      if (typed) filled[field] = typed
      else if (existing) {
        const stored = existing.config?.[field]
        if (typeof stored === 'string' && stored.trim()) filled[field] = stored
      }
    }
    for (const [field] of descriptor.secret) {
      const typed = bag[field]?.trim()
      if (typed) filled[field] = typed
    }

    const body = { platform, [descriptor.key]: filled }
    const resp = existing
      ? await put(`/api/channels/${existing.id}`, body)
      : await post('/api/channels', body)

    if (resp.success) {
      setMessage(`${descriptor.label} 已儲存`)
      // Clear secret inputs so they don't linger in the DOM after save.
      setForms((prev) => ({ ...prev, [platform]: {} }))
      void load()
    } else {
      setError(resp.message ?? `${descriptor.label} 儲存失敗`)
    }
  }

  // Admin gate AFTER all hooks (Rules of Hooks: stable hook order).
  if (!can(session.position(), 'system')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  return (
    <div style={{ maxWidth: 720, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="頻道管理" />
      {message && <p style={{ color: 'seagreen' }}>{message}</p>}
      {error && (
        <p role="alert" style={{ color: 'crimson' }}>
          {error}
        </p>
      )}

      {PLATFORM_KEYS.map((platform) => {
        const descriptor = PLATFORM_FORMS[platform]
        const existing = channels.find((c) => c.platform === platform)
        const bag = forms[platform] ?? {}
        const credsSet = existing?.credentialsSet ?? []
        return (
          <Card key={platform}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, marginBottom: 12 }}>
              <h3 style={{ margin: 0 }}>{descriptor.label}</h3>
              {existing && (
                <>
                  <small style={{ color: 'var(--muted)' }}>
                    {existing.isActive ? '啟用' : '停用'}
                  </small>
                  <small style={{ color: existing.isVerified ? 'seagreen' : 'orange' }}>
                    {existing.isVerified ? '已驗證' : '未驗證'}
                  </small>
                  {(existing.errorCount ?? 0) > 0 && (
                    <small style={{ color: 'crimson' }}>錯誤 {existing.errorCount}</small>
                  )}
                </>
              )}
            </div>

            {descriptor.plain.map(([field, label]) => {
              const stored = existing?.config?.[field]
              const value =
                bag[field] ?? (typeof stored === 'string' ? stored : '')
              return (
                <label key={field} style={{ display: 'block', marginBottom: 10 }}>
                  <span>{label}</span>
                  <input
                    type="text"
                    aria-label={`${descriptor.label} ${label}`}
                    value={value}
                    onChange={(e) => setField(platform, field, e.target.value)}
                    style={inputStyle}
                  />
                </label>
              )
            })}

            {descriptor.secret.map(([field, label]) => (
              <label key={field} style={{ display: 'block', marginBottom: 10 }}>
                <span>{label}</span>
                <input
                  type="password"
                  aria-label={`${descriptor.label} ${label}`}
                  value={bag[field] ?? ''}
                  placeholder={credsSet.includes(field) ? '已設定 ••••' : '未設定'}
                  onChange={(e) => setField(platform, field, e.target.value)}
                  style={inputStyle}
                />
              </label>
            ))}

            {platform === 'line' && (
              <p style={{ color: 'var(--muted)', fontSize: 13, marginTop: 4 }}>
                Webhook 路徑：<code>/api/webhook</code>
                <br />
                接在你的公開後端網址後（例如 https://&lt;your-tunnel-or-domain&gt;/api/webhook），填入
                LINE 後台 Webhook URL。
              </p>
            )}

            <div style={{ display: 'flex', gap: 8, marginTop: 8 }}>
              <button onClick={() => void save(platform)}>{descriptor.label} 儲存</button>
              {existing && <button onClick={() => void verify(existing.id)}>驗證</button>}
            </div>
          </Card>
        )
      })}
    </div>
  )
}
