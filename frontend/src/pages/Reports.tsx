// Reporting area (CRD §8.2): report list, generation over the generatable
// subset, download links.

import { useEffect, useState } from 'react'

import { get, post } from '../api/client'
import { session } from '../auth/session'

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

  const load = async () => {
    const resp = await get<{ reports?: Report[] }>('/api/reports')
    if (resp.success && resp.data) setReports(resp.data.reports ?? [])
    else setError(resp.message ?? null)
  }
  useEffect(() => {
    void load()
  }, [])

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
    // Authenticated download: fetch with the bearer header and trigger a save.
    const resp = await fetch(`/api/reports/${id}/download`, {
      headers: { Authorization: `Bearer ${session.accessToken() ?? ''}` },
    })
    if (!resp.ok) {
      setError('下載失敗')
      return
    }
    const blob = await resp.blob()
    const url = URL.createObjectURL(blob)
    const a = document.createElement('a')
    a.href = url
    a.download = resp.headers.get('content-disposition')?.match(/filename="(.+)"/)?.[1] ?? 'report'
    a.click()
    URL.revokeObjectURL(url)
  }

  return (
    <main style={{ maxWidth: 720, margin: '5vh auto' }}>
      <h1>報表</h1>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}
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
      </form>
      <table style={{ width: '100%', marginTop: 16, borderCollapse: 'collapse' }}>
        <thead>
          <tr style={{ textAlign: 'left', borderBottom: '1px solid #ddd' }}>
            <th>標題</th><th>類型</th><th>狀態</th><th></th>
          </tr>
        </thead>
        <tbody>
          {reports.map((r) => (
            <tr key={r.id} style={{ borderBottom: '1px solid #f0f0f0' }}>
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
    </main>
  )
}
