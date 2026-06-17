// Agents (operators) directory & presence (Phase 2.1). Server-paginated roster
// plus a presence-status histogram and a batch team-transfer action.

import { get, post, put, buildQuery, unwrapList } from '../api/client'

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

/// System-admin-only: create a new agent account. Returns ok + optional message.
export async function createAgent(input: {
  email: string
  password: string
  displayName: string
  role: 'admin' | 'agent'
}): Promise<{ ok: boolean; message?: string }> {
  const resp = await post('/api/auth/register', {
    email: input.email,
    password: input.password,
    displayName: input.displayName,
    role: input.role,
  })
  return { ok: resp.success, message: resp.message }
}
