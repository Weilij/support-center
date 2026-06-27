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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function normalizeRecord(value: unknown): Record<string, unknown> | undefined {
  return isRecord(value) ? value : undefined
}

function normalizeTopPerformer(value: unknown): TopPerformer | null {
  if (!isRecord(value)) return null
  return {
    userId: typeof value.userId === 'string' ? value.userId : undefined,
    displayName: typeof value.displayName === 'string' ? value.displayName : undefined,
    conversationsHandled:
      typeof value.conversationsHandled === 'number' && Number.isFinite(value.conversationsHandled)
        ? value.conversationsHandled
        : undefined,
  }
}

function normalizeTopPerformers(value: unknown): TopPerformer[] {
  return Array.isArray(value)
    ? value.map(normalizeTopPerformer).filter((item): item is TopPerformer => item !== null)
    : []
}

async function summaryOf(path: string, timeRange: string): Promise<{ summary?: Record<string, unknown>; raw?: Record<string, unknown> }> {
  const resp = await get<unknown>(
    `${path}${buildQuery({ timeRange })}`,
  )
  const outer = resp.success ? normalizeRecord(resp.data) : undefined
  const inner = normalizeRecord(outer?.data)
  return { summary: normalizeRecord(inner?.summary), raw: inner }
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
    topPerformers: normalizeTopPerformers(usr.raw?.topPerformers),
  }
}
