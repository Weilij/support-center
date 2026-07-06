// Reusable list table (Epic 0 foundation). Owns the load/empty/error/row
// rendering that every CRUD screen repeats, driven by a column descriptor so
// callers declare *what* to show, not *how* to lay it out.

import type { ReactNode } from 'react'

import { Loading } from './Loading'
import { ErrorRetry } from './ErrorRetry'

export interface Column<T> {
  key: string
  header: ReactNode
  /// Cell renderer; defaults to String(row[key]).
  render?: (row: T) => ReactNode
  width?: number | string
  align?: 'left' | 'right' | 'center'
}

export interface DataTableProps<T> {
  columns: Column<T>[]
  rows: T[]
  rowKey: (row: T) => string | number
  busy?: boolean
  error?: string | null
  /// Optional retry handler; when provided the error state offers a 重試 button.
  onRetry?: () => void
  /// Empty-state message when rows is empty and not busy.
  empty?: ReactNode
  onRowClick?: (row: T) => void
}

const cell: React.CSSProperties = {
  padding: '8px 10px',
  borderBottom: '1px solid var(--line-2)',
  textAlign: 'left',
  fontSize: 14,
}

export function DataTable<T>({
  columns,
  rows,
  rowKey,
  busy,
  error,
  onRetry,
  empty = '沒有資料',
  onRowClick,
}: DataTableProps<T>) {
  return (
    <div style={{ overflowX: 'auto', overflowY: 'auto', background: 'var(--surface)', border: '1px solid var(--line)', borderRadius: 'var(--radius-lg)', boxShadow: 'var(--shadow-sm)' }}>
      {error && <ErrorRetry message={error} onRetry={onRetry} />}
      <table style={{ width: '100%', borderCollapse: 'collapse' }}>
        <thead>
          <tr>
            {columns.map((c) => (
              <th
                key={c.key}
                style={{
                  ...cell,
                  width: c.width,
                  textAlign: c.align ?? 'left',
                  borderBottom: '1px solid var(--line)',
                  background: 'var(--bg)',
                  color: 'var(--muted)',
                  fontWeight: 600,
                  fontSize: 13,
                }}
              >
                {c.header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {busy && rows.length === 0 && !error && (
            <tr>
              <td style={{ ...cell }} colSpan={columns.length}>
                <Loading pad={false} />
              </td>
            </tr>
          )}
          {!busy && rows.length === 0 && !error && (
            <tr>
              <td style={{ ...cell, color: 'var(--muted)' }} colSpan={columns.length}>
                {empty}
              </td>
            </tr>
          )}
          {rows.map((row) => (
            <tr
              key={rowKey(row)}
              onClick={onRowClick ? () => onRowClick(row) : undefined}
              tabIndex={onRowClick ? 0 : undefined}
              role={onRowClick ? 'button' : undefined}
              onKeyDown={onRowClick ? (event) => {
                if (event.key === 'Enter' || event.key === ' ') {
                  event.preventDefault()
                  onRowClick(row)
                }
              } : undefined}
              style={{ cursor: onRowClick ? 'pointer' : 'default' }}
            >
              {columns.map((c) => (
                <td key={c.key} style={{ ...cell, textAlign: c.align ?? 'left' }}>
                  {c.render ? c.render(row) : String((row as Record<string, unknown>)[c.key] ?? '')}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

export interface PaginationProps {
  page: number
  total: number
  pageSize: number
  onPage: (page: number) => void
}

export function Pagination({ page, total, pageSize, onPage }: PaginationProps) {
  const pages = Math.max(1, Math.ceil(total / pageSize))
  if (pages <= 1) return null
  return (
    <div style={{ display: 'flex', gap: 8, alignItems: 'center', padding: '8px 0' }}>
      <button disabled={page <= 1} onClick={() => onPage(page - 1)}>
        上一頁
      </button>
      <span style={{ fontSize: 14, color: 'var(--muted)' }}>
        第 {page} / {pages} 頁（共 {total} 筆）
      </span>
      <button disabled={page >= pages} onClick={() => onPage(page + 1)}>
        下一頁
      </button>
    </div>
  )
}
