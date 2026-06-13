// Conversation sessions & topics (Phase 2.3). Sessions segment a conversation
// over time; admins browse them, edit topics, and close/reopen. Server paginates.

import { get, post, put, buildQuery } from '../api/client'

export interface SessionRow {
  id: string
  conversationId?: string
  sessionType?: string
  topic?: string | null
  startTime?: string
  endTime?: string | null
  lastActivityTime?: string | null
  messageCount?: number
  isActive?: boolean
  priority?: string
  sentiment?: string | null
}

export interface SessionStats {
  total?: number
  active?: number
  inactive?: number
  byType?: Record<string, number>
  byPriority?: Record<string, number>
}

export interface SessionsPage {
  sessions: SessionRow[]
  total: number
  page: number
}

export async function loadSessions(page = 1, pageSize = 20): Promise<SessionsPage> {
  const resp = await get<{ sessions?: SessionRow[]; pagination?: { total?: number } }>(
    `/api/sessions${buildQuery({ page, pageSize })}`,
  )
  if (resp.success && resp.data) {
    return {
      sessions: resp.data.sessions ?? [],
      total: resp.data.pagination?.total ?? (resp.data.sessions?.length ?? 0),
      page,
    }
  }
  return { sessions: [], total: 0, page }
}

export async function loadSessionStats(): Promise<SessionStats> {
  const resp = await get<SessionStats>('/api/sessions/stats')
  return resp.success && resp.data ? resp.data : {}
}

export async function closeSession(id: string): Promise<boolean> {
  return (await post(`/api/sessions/${id}/close`, {})).success
}

export async function reopenSession(id: string): Promise<boolean> {
  return (await post(`/api/sessions/${id}/reopen`, {})).success
}

export async function updateSessionTopic(id: string, topic: string): Promise<boolean> {
  return (await put(`/api/sessions/${id}/topic`, { topic })).success
}
