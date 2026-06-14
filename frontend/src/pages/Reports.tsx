// Reporting area (CRD §8.2): report list, generation over the generatable
// subset, download links.

import { useEffect, useState } from 'react'

import { get, post, download as downloadFile } from '../api/client'
import { Modal } from '../components/Modal'
import { can } from '../auth/permissions'
import { session } from '../auth/session'
import { PageHeader } from '../components/PageHeader'
import { Card } from '../components/Card'

interface Report {
  id: string
  title: string
  type?: string
  format?: string
  status: string
  createdAt?: string
}

const GENERATABLE = [
  ['conversation_summary', '對話摘要'],
  ['agent_performance', '客服績效'],
  ['message_statistics', '訊息統計'],
] as const

export default function Reports() {
  const [reports, setReports] = useState<Report[]>([])
  const [kind, setKind] = useState('conversation_summary')
  const [title, setTitle] = useState('')
  const [format, setFormat] = useState('json')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [preview, setPreview] = useState<unknown | null>(null)
  const [previewOpen, setPreviewOpen] = useState(false)

  const load = async () => {
    const resp = await get<{ reports?: Report[] }>('/api/reports')
    if (resp.success && resp.data) setReports(resp.data.reports ?? [])
    else setError(resp.message ?? null)
  }
  useEffect(() => {
    void load()
  }, [])

  if (!can(session.position(), 'analytics')) {
    return (
      <main style={{ margin: '10vh auto', maxWidth: 480 }}>
        <p>權限不足</p>
      </main>
    )
  }

  const generate = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true); setError(null)
    const resp = await post('/api/reports', {
      type: kind, title: title || '未命名報表', format, timeRange: 'last_7_days',
    })
    setBusy(false)
    if (resp.success) {
      setTitle('')
      void load()
    } else {
      setError(resp.message ?? null)
    }
  }

  const download = async (id: string) => {
    const res = await downloadFile('GET', `/api/reports/${id}/download`, undefined, 'report')
    if (!res.ok) setError(res.message ?? '下載失敗')
  }

  const showPreview = async () => {
    setError(null)
    const resp = await post<unknown>('/api/reports/preview', { kind, timeRange: 'last_7_days' })
    if (resp.success) {
      setPreview(resp.data ?? null)
      setPreviewOpen(true)
    } else {
      setError(resp.message ?? '預覽失敗')
    }
  }

  return (
    <div style={{ maxWidth: 720, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="報表" />
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}

      <Card title="產生報表" style={{ marginBottom: 'var(--sp-4)' }}>
        <form onSubmit={generate} style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            {GENERATABLE.map(([code, label]) => (
              <option key={code} value={code}>{label}</option>
            ))}
          </select>
          <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="報表標題" />
          <select value={format} onChange={(e) => setFormat(e.target.value)}>
            <option value="json">JSON</option>
            <option value="csv">CSV</option>
          </select>
          <button type="submit" disabled={busy}>產生報表</button>
          <button type="button" onClick={() => void showPreview()}>預覽</button>
        </form>
      </Card>

      <Card title="報表清單">
        <table style={{ width: '100%', borderCollapse: 'collapse' }}>
          <thead>
            <tr style={{ textAlign: 'left', borderBottom: '1px solid var(--hairline)' }}>
              <th>標題</th><th>類型</th><th>狀態</th><th></th>
            </tr>
          </thead>
          <tbody>
            {reports.map((r) => (
              <tr key={r.id} style={{ borderBottom: '1px solid var(--hairline)' }}>
                <td>{r.title}</td>
                <td>{r.type}</td>
                <td>{r.status}</td>
                <td>
                  {r.status === 'completed' && (
                    <button onClick={() => void download(r.id)}>下載</button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </Card>

      <Modal open={previewOpen} title="報表預覽" onClose={() => setPreviewOpen(false)} width={560}>
        <pre style={{ background: '#f7f7f7', padding: 12, borderRadius: 6, overflowX: 'auto', fontSize: 12 }}>
          {preview ? JSON.stringify(preview, null, 2) : '無資料'}
        </pre>
      </Modal>
    </div>
  )
}
