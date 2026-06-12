// Browser session state (CRD §8.4 guard inputs + §8.1 auth state): stored
// credentials, cached identity, lifecycle pending/authenticated/
// unauthenticated, a short-lived auth snapshot, and a global change signal.

export type SessionLifecycle = 'pending' | 'authenticated' | 'unauthenticated'

export interface Identity {
  id: string
  email?: string
  displayName?: string
  role?: string
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
  accessToken: () => localStorage.getItem('mcss.token'),
  refreshToken: () => localStorage.getItem('mcss.refreshToken'),
  sessionId: () => localStorage.getItem('mcss.sessionId'),
  contextTeamId: () => localStorage.getItem('mcss.contextTeamId'),

  storeTokens(token: string, refreshToken?: string) {
    localStorage.setItem('mcss.token', token)
    if (refreshToken) localStorage.setItem('mcss.refreshToken', refreshToken)
  },

  storeLogin(token: string, refreshToken: string, sessionId: string, who: Identity) {
    this.storeTokens(token, refreshToken)
    localStorage.setItem('mcss.sessionId', sessionId)
    identity = who
    lifecycle = 'authenticated'
    authChanged.emit()
  },

  clear() {
    for (const key of ['mcss.token', 'mcss.refreshToken', 'mcss.sessionId']) {
      localStorage.removeItem(key)
    }
    identity = null
    lifecycle = 'unauthenticated'
  },

  lifecycle: () => lifecycle,
  identity: () => identity,
  isAdmin: () => identity?.role === 'admin',

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

  /// One-time session initialization: validates the stored credential via the
  /// current-identity endpoint (CRD 6483).
  init(): Promise<void> {
    if (!initPromise) {
      initPromise = (async () => {
        if (!this.accessToken()) {
          lifecycle = 'unauthenticated'
          return
        }
        try {
          const resp = await fetch('/api/auth/me', {
            headers: { Authorization: `Bearer ${this.accessToken()}` },
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
