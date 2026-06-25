// Frontend access-control (spec 2026-06-14). Three positions map to feature
// areas; the backend mirrors this for protected route groups.

export type Position = 'system_admin' | 'supervisor' | 'agent'
export type Area = 'daily' | 'ops' | 'analytics' | 'system'

export const AREA_ACCESS: Record<Position, Area[]> = {
  agent: ['daily'],
  supervisor: ['daily', 'ops', 'analytics'],
  system_admin: ['daily', 'ops', 'analytics', 'system'],
}

export const POSITION_LABELS: Record<Position, string> = {
  system_admin: '系統管理員',
  supervisor: '主管／分析師',
  agent: '客服',
}

const POSITIONS: Position[] = ['system_admin', 'supervisor', 'agent']

/// Resolve a user's position: an explicit, valid `position` wins; otherwise
/// derive from the backend role (admin → system_admin, else agent).
export function positionOf(identity: { position?: string; role?: string } | null | undefined): Position {
  const p = identity?.position
  if (p && (POSITIONS as string[]).includes(p)) return p as Position
  return identity?.role === 'admin' ? 'system_admin' : 'agent'
}

export function can(position: Position, area: Area): boolean {
  return AREA_ACCESS[position].includes(area)
}
