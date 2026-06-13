// Message search & export (Phase 1.4). Search is offset-paginated server-side;
// export streams a CSV download. Both share the same filter shape.

import { get, buildQuery, download } from '../api/client'

export interface MessageHit {
  id: string
  conversationId: string
  senderType?: string
  senderName?: string
  content?: string
  messageType?: string
  isRecalled?: boolean
  createdAt?: string
}

export interface MessageSearchParams {
  q?: string
  conversationId?: string
  senderType?: string
  messageType?: string
  dateFrom?: string
  dateTo?: string
  limit?: number
  offset?: number
}

export interface MessageSearchResult {
  messages: MessageHit[]
  total: number
  hasMore: boolean
}

export async function searchMessages(params: MessageSearchParams): Promise<MessageSearchResult> {
  const qs = buildQuery({ ...params })
  const resp = await get<{ messages?: MessageHit[]; total?: number; pagination?: { hasMore?: boolean } }>(
    `/api/messages/search${qs}`,
  )
  if (resp.success && resp.data) {
    return {
      messages: resp.data.messages ?? [],
      total: resp.data.total ?? 0,
      hasMore: resp.data.pagination?.hasMore ?? false,
    }
  }
  return { messages: [], total: 0, hasMore: false }
}

/// Trigger a CSV export honouring the active filters.
export function exportMessagesCsv(params: MessageSearchParams): Promise<{ ok: boolean; message?: string }> {
  const qs = buildQuery({ ...params, format: 'csv', limit: undefined, offset: undefined })
  return download('GET', `/api/messages/export${qs}`, undefined, 'messages_export.csv')
}
