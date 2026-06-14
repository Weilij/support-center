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
  [key: string]: unknown
}

const SNAPSHOT_TTL_MS = 3000

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
let initPromise: Promise<void> | null = null
let snapshot: { at: number; authenticated: boolean } | null = null

export const session = {
  sessionId: () => localStorage.getItem('mcss.sessionId'),
  contextTeamId: () => localStorage.getItem('mcss.contextTeamId'),

  /// Called after a successful login: cache identity from the JSON body (the
  /// backend has already set the HttpOnly auth cookies at this point).
  storeLogin(sessionId: string, who: Identity) {
    localStorage.setItem('mcss.sessionId', sessionId)
    identity = who
    lifecycle = 'authenticated'
    authChanged.emit()
  },

  clear() {
    localStorage.removeItem('mcss.sessionId')
    identity = null
    lifecycle = 'unauthenticated'
  },

  lifecycle: () => lifecycle,
  identity: () => identity,
  isAdmin: () => identity?.role === 'admin',
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
