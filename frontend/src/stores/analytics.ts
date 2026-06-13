// Analytics overview (Phase 3.1). Pulls the four core analytics endpoints —
// conversations, messages, users, performance — each returning a nested
// { data: { summary, ... } }. The full widget designer/comparison presets are
// deferred; this delivers the headline metrics.

import { get, buildQuery } from '../api/client'

export interface CoreSummaries {
  conversations?: Record<string, unknown>
  messages?: Record<string, unknown>
  users?: Record<string, unknown>
  performance?: Record<string, unknown>
  topPerformers?: TopPerformer[]
}

export interface TopPerformer {
  userId?: string
  displayName?: string
  conversationsHandled?: number
}

async function summaryOf(path: string, timeRange: string): Promise<{ summary?: Record<string, unknown>; raw?: Record<string, unknown> }> {
  const resp = await get<{ data?: { summary?: Record<string, unknown>; topPerformers?: TopPerformer[] } }>(
    `${path}${buildQuery({ timeRange })}`,
  )
  const inner = resp.success ? resp.data?.data : undefined
  return { summary: inner?.summary, raw: inner as Record<string, unknown> | undefined }
}

export async function loadAnalyticsOverview(timeRange = '7d'): Promise<CoreSummaries> {
  const [conv, msg, usr, perf] = await Promise.all([
    summaryOf('/api/analytics/conversations', timeRange),
    summaryOf('/api/analytics/messages', timeRange),
    summaryOf('/api/analytics/users', timeRange),
    summaryOf('/api/analytics/performance', timeRange),
  ])
  return {
    conversations: conv.summary,
    messages: msg.summary,
    users: usr.summary,
    performance: perf.summary,
    topPerformers: (usr.raw?.topPerformers as TopPerformer[]) ?? [],
  }
}
