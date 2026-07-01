// Browser session state (CRD §8.4 guard inputs + §8.1 auth state): cached
// identity, lifecycle pending/authenticated/unauthenticated, a short-lived
// auth snapshot, and a global change signal.
//
// Access/refresh tokens are NO LONGER stored in JS — the backend sets them as
// HttpOnly cookies. Only the non-sensitive sessionId and contextTeamId are kept
// in localStorage. Identity is cached in memory from login / /me.

import { positionOf, type Position } from './permissions'

export type SessionLifecycle = 'pending' | 'authenticated' | 'unauthenticated'

export interface Identity {
  id: string
  email?: string
  displayName?: string
  role?: string
  position?: string
  teamId?: string | number | null
  teamName?: string | null
  teams?: unknown
  [key: string]: unknown
}

const SNAPSHOT_TTL_MS = 3000
const TEAM_CONTEXT_KEY = 'mcss.contextTeamId'

export interface TeamOption {
  id: string
  name: string
  isPrimary: boolean
  role?: string // in-team role: member/lead/supervisor (absent on the primary-only fallback)
}

class AuthChangedSignal {
  private listeners = new Set<() => void>()
  on(fn: () => void) {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }
  emit() {
    snapshot = null // invalidate the auth snapshot immediately (CRD 6492)
    this.listeners.forEach((fn) => fn())
  }
}

export const authChanged = new AuthChangedSignal()

let lifecycle: SessionLifecycle = 'pending'
let identity: Identity | null = null
let teamOptions: TeamOption[] = []
let initPromise: Promise<void> | null = null
let snapshot: { at: number; authenticated: boolean } | null = null

function teamId(value: unknown): string | null {
  if (typeof value === 'number' && Number.isFinite(value) && value > 0) return String(value)
  if (typeof value === 'string' && value.trim() !== '') return value.trim()
  return null
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
}

function readTeams(who: Identity): TeamOption[] {
  const raw = Array.isArray(who.teams) ? who.teams : []
  const seen = new Set<string>()
  const options: TeamOption[] = []
  for (const item of raw) {
    if (!isRecord(item)) continue
    const id = teamId(item.id ?? item.teamId)
    if (!id || seen.has(id)) continue
    seen.add(id)
    options.push({
      id,
      name: String(item.name ?? item.teamName ?? `Team ${id}`),
      isPrimary: item.isPrimary === true || item.primary === true,
      role: typeof item.roleInTeam === 'string' ? item.roleInTeam : undefined,
    })
  }
  const primaryId = teamId(who.teamId)
  if (primaryId && !seen.has(primaryId)) {
    options.unshift({
      id: primaryId,
      name: String(who.teamName ?? `Team ${primaryId}`),
      isPrimary: true,
    })
  }
  return options
}

function canUseTeam(id: string): boolean {
  if (identity?.role === 'admin') return true
  return teamOptions.some((team) => team.id === id)
}

function setInitialTeamContext(who: Identity) {
  teamOptions = readTeams(who)
  const persisted = teamId(localStorage.getItem(TEAM_CONTEXT_KEY))
  if (persisted && canUseTeam(persisted)) return

  const primary = teamOptions.find((team) => team.isPrimary)?.id ?? teamOptions[0]?.id
  if (primary) localStorage.setItem(TEAM_CONTEXT_KEY, primary)
  else localStorage.removeItem(TEAM_CONTEXT_KEY)
}

export const session = {
  sessionId: () => localStorage.getItem('mcss.sessionId'),
  contextTeamId: () => localStorage.getItem(TEAM_CONTEXT_KEY),
  teamOptions: () => teamOptions.map((team) => ({ ...team })),
  currentTeam: () => {
    const current = localStorage.getItem(TEAM_CONTEXT_KEY)
    return teamOptions.find((team) => team.id === current) ?? null
  },
  switchContextTeam(nextTeamId: string | number): boolean {
    const id = teamId(nextTeamId)
    if (!id || !canUseTeam(id)) return false
    localStorage.setItem(TEAM_CONTEXT_KEY, id)
    return true
  },
  clearContextTeam(): boolean {
    if (identity?.role !== 'admin') return false
    localStorage.removeItem(TEAM_CONTEXT_KEY)
    return true
  },

  /// Called after a successful login: cache identity from the JSON body (the
  /// backend has already set the HttpOnly auth cookies at this point).
  storeLogin(sessionId: string, who: Identity) {
    localStorage.setItem('mcss.sessionId', sessionId)
    identity = who
    setInitialTeamContext(who)
    lifecycle = 'authenticated'
    authChanged.emit()
  },

  clear() {
    localStorage.removeItem('mcss.sessionId')
    localStorage.removeItem(TEAM_CONTEXT_KEY)
    identity = null
    teamOptions = []
    lifecycle = 'unauthenticated'
  },

  lifecycle: () => lifecycle,
  identity: () => identity,
  isAdmin: () => identity?.role === 'admin',
  isTeamManager: () =>
    identity?.role === 'admin' ||
    teamOptions.some((t) => t.role === 'lead' || t.role === 'supervisor'),
  position: (): Position => positionOf(identity),

  /// Short-lived auth snapshot for instant guard decisions (CRD 6480).
  snapshot(): boolean | null {
    if (snapshot && Date.now() - snapshot.at < SNAPSHOT_TTL_MS) {
      return snapshot.authenticated
    }
    return null
  },
  recordSnapshot(authenticated: boolean) {
    snapshot = { at: Date.now(), authenticated }
  },

  /// One-time session initialization: validates the session via the
  /// current-identity endpoint using the mcss_access cookie (CRD 6483).
  init(): Promise<void> {
    if (!initPromise) {
      initPromise = (async () => {
        try {
          const resp = await fetch('/api/auth/me', {
            credentials: 'include',
          })
          const body = await resp.json().catch(() => null)
          if (resp.ok && body?.success) {
            identity = body.data as Identity
            setInitialTeamContext(identity)
            lifecycle = 'authenticated'
          } else {
            this.clear()
          }
        } catch {
          this.clear()
        }
      })()
    }
    return initPromise
  },
}
