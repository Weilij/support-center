// Shared loading indicator (Phase 3.1): a spinner + muted label used for
// page-level initial loads and the router's lazy-chunk Suspense fallback.

export function Loading({ label = '載入中…', pad = true }: { label?: string; pad?: boolean }) {
  return (
    <div
      role="status"
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 12,
        padding: pad ? '48px 20px' : 0,
        color: 'var(--muted)',
        fontSize: 13,
      }}
    >
      <span className="cs-spinner" aria-hidden="true" />
      {label}
    </div>
  )
}
