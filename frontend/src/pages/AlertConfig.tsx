// Alert configuration (Phase 4, admin): configure Slack / email / webhook alert
// channels, view which are configured, and dispatch a synthetic test alert.

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { session } from '../auth/session'
import { Input } from '../components/Form'
import { StatusPill, Toast } from '../components/ui'

interface ChannelStatus {
  slack?: { configured?: boolean }
  email?: { configured?: boolean; recipientCount?: number }
  webhook?: { configured?: boolean }
}

export default function AlertConfig() {
  const [status, setStatus] = useState<ChannelStatus>({})
  const [slackUrl, setSlackUrl] = useState('')
  const [webhookUrl, setWebhookUrl] = useState('')
  const [email, setEmail] = useState({ host: '', port: 587, sender: '', password: '', recipients: '' })
  const [toast, setToast] = useState<string | null>(null)

  const loadStatus = async () => {
    const resp = await get<ChannelStatus>('/api/alert-config/channels/status')
    if (resp.success && resp.data) setStatus(resp.data)
  }
  useEffect(() => {
    void loadStatus()
  }, [])

  const after = (resp: { success: boolean; message?: string }, ok: string) => {
    setToast(resp.success ? ok : resp.message ?? '失敗')
    if (resp.success) void loadStatus()
  }

  const saveSlack = async () => after(await post('/api/alert-config/channels/slack', { webhookUrl: slackUrl }), 'Slack 已設定')
  const saveWebhook = async () => after(await post('/api/alert-config/channels/webhook', { webhookUrl }), 'Webhook 已設定')
  const saveEmail = async () =>
    after(
      await post('/api/alert-config/channels/email', {
        host: email.host,
        port: Number(email.port),
        sender: email.sender,
        password: email.password,
        recipients: email.recipients.split(',').map((r) => r.trim()).filter(Boolean),
      }),
      'Email 已設定',
    )
  const testAlert = async () =>
    setToast((await post('/api/alert-config/test-alert', { level: 'warning', title: '測試告警' })).success ? '測試告警已送出' : '送出失敗')

  if (!session.isAdmin()) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  return (
    <main style={{ maxWidth: 720, margin: '4vh auto', padding: '0 16px' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <h1 style={{ margin: 0 }}>告警設定</h1>
        <button onClick={() => void testAlert()} style={{ marginLeft: 'auto' }}>
          發送測試告警
        </button>
      </div>

      <div style={{ display: 'flex', gap: 10, margin: '12px 0' }}>
        <StatusPill status={status.slack?.configured ? 'active' : 'inactive'} label={`Slack ${status.slack?.configured ? '已設定' : '未設定'}`} />
        <StatusPill status={status.email?.configured ? 'active' : 'inactive'} label={`Email ${status.email?.configured ? '已設定' : '未設定'}`} />
        <StatusPill status={status.webhook?.configured ? 'active' : 'inactive'} label={`Webhook ${status.webhook?.configured ? '已設定' : '未設定'}`} />
      </div>

      <Card title="Slack">
        <Input label="Webhook URL" value={slackUrl} onChange={(e) => setSlackUrl(e.target.value)} placeholder="https://hooks.slack.com/..." />
        <button onClick={() => void saveSlack()}>儲存 Slack</button>
      </Card>

      <Card title="Webhook">
        <Input label="Webhook URL" value={webhookUrl} onChange={(e) => setWebhookUrl(e.target.value)} placeholder="https://..." />
        <button onClick={() => void saveWebhook()}>儲存 Webhook</button>
      </Card>

      <Card title="Email (SMTP)">
        <Input label="SMTP 主機" value={email.host} onChange={(e) => setEmail({ ...email, host: e.target.value })} />
        <Input label="Port" type="number" value={email.port} onChange={(e) => setEmail({ ...email, port: Number(e.target.value) })} />
        <Input label="寄件者" value={email.sender} onChange={(e) => setEmail({ ...email, sender: e.target.value })} />
        <Input label="密碼" type="password" value={email.password} onChange={(e) => setEmail({ ...email, password: e.target.value })} />
        <Input label="收件者（逗號分隔）" value={email.recipients} onChange={(e) => setEmail({ ...email, recipients: e.target.value })} />
        <button onClick={() => void saveEmail()}>儲存 Email</button>
      </Card>

      <Toast message={toast} onDismiss={() => setToast(null)} />
    </main>
  )
}

function Card({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ border: '1px solid #eee', borderRadius: 8, padding: 14, marginBottom: 14 }}>
      <h3 style={{ marginTop: 0 }}>{title}</h3>
      {children}
    </section>
  )
}
