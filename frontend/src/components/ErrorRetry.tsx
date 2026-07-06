// Shared error state with a retry affordance (Phase 3.2). Replaces the bare
// crimson error text scattered across screens.

export function ErrorRetry({
  message,
  onRetry,
  pad = true,
}: {
  message?: string | null
  onRetry?: () => void
  pad?: boolean
}) {
  return (
    <div
      role="alert"
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 12,
        padding: pad ? '32px 20px' : '8px 0',
        textAlign: 'center',
      }}
    >
      <span style={{ color: 'var(--color-danger)', fontSize: 13 }}>
        {message ?? '載入失敗，請稍後再試'}
      </span>
      {onRetry && (
        <button type="button" className="cs-btn" onClick={onRetry}>
          重試
        </button>
      )}
    </div>
  )
}
