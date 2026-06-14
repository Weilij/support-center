// Customer directory (Phase 1.1): searchable, paginated list with a detail
// drawer showing profile, tags, and linked conversations. Filtering and paging
// are client-side because the backend returns the full visible set at once.

import { useEffect, useMemo, useState } from 'react'
import { Link } from 'react-router-dom'

import { DataTable, Pagination } from '../components/DataTable'
import { Drawer } from '../components/Modal'
import { FilterBar, StatusPill, Badge, EmptyState } from '../components/ui'
import { useStore } from '../stores/store'
import {
  customersStore,
  loadCustomers,
  loadCustomerDetail,
  loadCustomerTags,
  type Customer,
  type CustomerDetail,
  type CustomerTag,
} from '../stores/customers'
import type { Column } from '../components/DataTable'
import { PageHeader } from '../components/PageHeader'

const PAGE_SIZE = 20

export default function Customers() {
  const { items, busy, error } = useStore(customersStore)
  const [search, setSearch] = useState('')
  const [platform, setPlatform] = useState('')
  const [page, setPage] = useState(1)
  const [selected, setSelected] = useState<number | null>(null)

  useEffect(() => {
    void loadCustomers()
  }, [])

  const platforms = useMemo(
    () => Array.from(new Set(items.map((c) => c.platform).filter(Boolean))),
    [items],
  )

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase()
    return items.filter((c) => {
      if (platform && c.platform !== platform) return false
      if (!q) return true
      return [c.display_name, c.email, c.phone, c.platform_user_id]
        .some((v) => String(v ?? '').toLowerCase().includes(q))
    })
  }, [items, search, platform])

  const paged = filtered.slice((page - 1) * PAGE_SIZE, page * PAGE_SIZE)

  // Keep the page in range when filters shrink the result set.
  useEffect(() => {
    setPage(1)
  }, [search, platform])

  const columns: Column<Customer>[] = [
    { key: 'display_name', header: '名稱', render: (c) => c.display_name || '（未命名）' },
    { key: 'platform', header: '平台', render: (c) => <StatusPill status={c.platform} /> },
    { key: 'email', header: 'Email', render: (c) => c.email || '—' },
    { key: 'phone', header: '電話', render: (c) => c.phone || '—' },
    {
      key: 'created_at',
      header: '建立時間',
      render: (c) => (c.created_at ? new Date(c.created_at).toLocaleDateString() : '—'),
    },
  ]

  return (
    <div style={{ maxWidth: 1000, margin: '0 auto', padding: '0 16px' }}>
      <PageHeader title="客戶管理" subtitle={`共 ${filtered.length} 位`} />

      <FilterBar>
        <input
          placeholder="搜尋名稱 / Email / 電話"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          style={{ padding: '7px 9px', borderRadius: 6, border: '1px solid var(--hairline)', minWidth: 240 }}
        />
        <select
          value={platform}
          onChange={(e) => setPlatform(e.target.value)}
          style={{ padding: '7px 9px', borderRadius: 6, border: '1px solid var(--hairline)' }}
        >
          <option value="">全部平台</option>
          {platforms.map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </select>
      </FilterBar>

      <DataTable
        columns={columns}
        rows={paged}
        rowKey={(c) => c.id}
        busy={busy}
        error={error}
        empty="沒有符合的客戶"
        onRowClick={(c) => setSelected(c.id)}
      />
      <Pagination page={page} total={filtered.length} pageSize={PAGE_SIZE} onPage={setPage} />

      <CustomerDrawer id={selected} onClose={() => setSelected(null)} />
    </div>
  )
}

function CustomerDrawer({ id, onClose }: { id: number | null; onClose: () => void }) {
  const [detail, setDetail] = useState<CustomerDetail | null>(null)
  const [tags, setTags] = useState<CustomerTag[]>([])
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    if (id == null) {
      setDetail(null)
      setTags([])
      return
    }
    setLoading(true)
    void Promise.all([loadCustomerDetail(id), loadCustomerTags(id)]).then(([d, t]) => {
      setDetail(d)
      setTags(t)
      setLoading(false)
    })
  }, [id])

  const c = detail?.customer
  return (
    <Drawer open={id != null} title={c?.display_name || '客戶詳情'} onClose={onClose} width={460}>
      {loading && <p style={{ color: 'var(--muted)' }}>載入中…</p>}
      {!loading && !detail && <EmptyState message="找不到客戶資料" />}
      {!loading && c && (
        <div>
          <section style={{ marginBottom: 16 }}>
            <Row label="平台">
              <StatusPill status={c.platform} />
            </Row>
            <Row label="平台 ID">{c.platform_user_id}</Row>
            <Row label="Email">{c.email || '—'}</Row>
            <Row label="電話">{c.phone || '—'}</Row>
            <Row label="建立時間">
              {c.created_at ? new Date(c.created_at).toLocaleString() : '—'}
            </Row>
          </section>

          <h3 style={{ fontSize: 15, margin: '0 0 8px' }}>標籤</h3>
          {tags.length === 0 ? (
            <p style={{ color: 'var(--muted)', fontSize: 13 }}>尚無標籤</p>
          ) : (
            <div style={{ display: 'flex', gap: 6, flexWrap: 'wrap', marginBottom: 16 }}>
              {tags.map((t) => (
                <Badge key={t.id} color={t.color ?? '#3B82F6'}>
                  {t.name}
                </Badge>
              ))}
            </div>
          )}

          <h3 style={{ fontSize: 15, margin: '0 0 8px' }}>
            對話紀錄（{detail.conversationCount}）
          </h3>
          {detail.conversations.length === 0 ? (
            <p style={{ color: 'var(--muted)', fontSize: 13 }}>尚無對話</p>
          ) : (
            <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
              {detail.conversations.map((conv) => (
                <li
                  key={conv.id}
                  style={{
                    display: 'flex',
                    gap: 8,
                    alignItems: 'center',
                    padding: '6px 0',
                    borderBottom: '1px solid var(--hairline)',
                  }}
                >
                  <StatusPill status={conv.status} />
                  <Link to={`/conversations/${conv.id}`} onClick={onClose}>
                    對話 {conv.id.slice(0, 8)}
                  </Link>
                  <span style={{ marginLeft: 'auto', fontSize: 12, color: 'var(--muted)' }}>
                    {conv.last_message_at
                      ? new Date(conv.last_message_at).toLocaleDateString()
                      : ''}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </Drawer>
  )
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div style={{ display: 'flex', gap: 8, padding: '4px 0', fontSize: 14 }}>
      <span style={{ color: 'var(--muted)', width: 80, flexShrink: 0 }}>{label}</span>
      <span>{children}</span>
    </div>
  )
}
