// Generic metrics renderer (Phase 4): ops endpoints return deeply-nested JSON
// whose exact shape varies. Rather than hand-code each, flatten scalar leaves
// into grouped label/value rows so any stats payload renders readably.

import type { ReactNode } from 'react'

type Json = unknown

function isScalar(v: Json): v is string | number | boolean {
  return v === null || ['string', 'number', 'boolean'].includes(typeof v)
}

function scalarText(v: Json): string {
  if (v === null || v === undefined) return '—'
  if (typeof v === 'boolean') return v ? '是' : '否'
  if (typeof v === 'number') return v.toLocaleString()
  return String(v)
}

function Group({ title, obj }: { title?: string; obj: Record<string, Json> }) {
  const scalarEntries = Object.entries(obj).filter(([, v]) => isScalar(v))
  const nestedEntries = Object.entries(obj).filter(([, v]) => !isScalar(v) && !Array.isArray(v))
  const arrayEntries = Object.entries(obj).filter(([, v]) => Array.isArray(v))

  return (
    <div style={{ marginBottom: 14 }}>
      {title && <h4 style={{ margin: '0 0 6px', color: '#444' }}>{title}</h4>}
      {scalarEntries.length > 0 && (
        <div style={{ display: 'flex', gap: 8, flexWrap: 'wrap' }}>
          {scalarEntries.map(([k, v]) => (
            <div
              key={k}
              style={{ border: '1px solid #eee', borderRadius: 8, padding: '8px 12px', minWidth: 120 }}
            >
              <div style={{ fontSize: 12, color: '#888' }}>{k}</div>
              <div style={{ fontSize: 18, fontWeight: 600 }}>{scalarText(v)}</div>
            </div>
          ))}
        </div>
      )}
      {nestedEntries.map(([k, v]) => (
        <div key={k} style={{ marginTop: 10, paddingLeft: 10, borderLeft: '2px solid #f0f0f0' }}>
          <Group title={k} obj={v as Record<string, Json>} />
        </div>
      ))}
      {arrayEntries.map(([k, v]) => (
        <div key={k} style={{ marginTop: 10 }}>
          <h4 style={{ margin: '0 0 6px', color: '#444' }}>
            {k}（{(v as Json[]).length}）
          </h4>
          <ul style={{ margin: 0, paddingLeft: 18, fontSize: 13, color: '#555' }}>
            {(v as Json[]).slice(0, 20).map((item, i) => (
              <li key={i}>{isScalar(item) ? scalarText(item) : JSON.stringify(item)}</li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  )
}

export function MetricsView({ data, empty = '無資料' }: { data: Json; empty?: ReactNode }) {
  if (data == null || (typeof data === 'object' && Object.keys(data as object).length === 0)) {
    return <p style={{ color: '#888' }}>{empty}</p>
  }
  if (isScalar(data)) return <p>{scalarText(data)}</p>
  if (Array.isArray(data)) return <Group obj={{ items: data }} />
  return <Group obj={data as Record<string, Json>} />
}
