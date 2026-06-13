// Agents (operators) directory & presence (Phase 2.1). Server-paginated roster
// plus a presence-status histogram and a batch team-transfer action.

import { get, put, buildQuery, unwrapList } from '../api/client'

export interface Agent {
  id: string
  email?: string
  displayName?: string
  role?: string
  position?: string
  isActive?: boolean
  teamId?: number | null
  teamName?: string | null
  lastActiveAt?: string | null
  lastLoginAt?: string | null
  createdAt?: string
}

export const PRESENCE_STATES = ['online', 'busy', 'away', 'offline', 'break', 'meeting'] as const

export interface AgentsPage {
  items: Agent[]
  total: number
  page: number
}

export async function loadAgents(page = 1, limit = 20): Promise<AgentsPage> {
  const resp = await get<Agent[]>(`/api/agents${buildQuery({ page, limit })}`)
  const { items, total } = unwrapList<Agent>(resp as never, page)
  return { items, total, page }
}

export async function loadStatusStatistics(): Promise<Record<string, number>> {
  const resp = await get<Record<string, number>>('/api/agents/status/statistics')
  return resp.success && resp.data ? resp.data : {}
}

/// System-admin-only: persist a member's position.
export async function setAgentPosition(
  agentId: string,
  position: string,
): Promise<{ ok: boolean; message?: string }> {
  const resp = await put(`/api/agents/${agentId}`, { position })
  return { ok: resp.success, message: resp.message }
}

/// Move many agents to a team in one call. Returns success count + any errors.
export async function batchTransferAgents(
  agentIds: string[],
  toTeamId: number,
): Promise<{ ok: boolean; message?: string }> {
  const resp = await put('/api/agents/batch/transfer', { agentIds, toTeamId })
  return { ok: resp.success, message: resp.message }
}
