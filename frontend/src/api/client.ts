// Shared authenticated HTTP client (CRD §8.4, lines 6496-6520).
//
// Every backend call passes through here: bearer + team-context headers,
// JSON-parse fallback, single-flight credential renewal with transparent
// one-time retry, a guarded once-only redirect to login, and bounded
// back-off retries for server/network failures only.

import { session, authChanged } from '../auth/session'
import { t } from '../i18n'

export interface Envelope<T = unknown> {
  success: boolean
  data?: T
  message?: string
  error?: string
  status?: number
  [key: string]: unknown
}

const MAX_RETRIES = 2
const BACKOFF_MS = [300, 900]

let refreshInFlight: Promise<boolean> | null = null
let redirectingToLogin = false // resets on full page reload (CRD 6519)

function statusMessage(status: number): string {
  const map: Record<number, string> = {
    400: t('error.badRequest'),
    401: t('error.unauthorized'),
    403: t('error.forbidden'),
    404: t('error.notFound'),
    429: t('error.tooManyRequests'),
    500: t('error.server'),
    502: t('error.server'),
    503: t('error.server'),
  }
  return map[status] ?? t('error.server')
}

/// Single-flight renewal: concurrent unauthorized calls share one attempt.
async function renewCredentials(): Promise<boolean> {
  if (!refreshInFlight) {
    refreshInFlight = (async () => {
      const refreshToken = session.refreshToken()
      if (!refreshToken) return false
      try {
        const resp = await fetch('/api/auth/refresh', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ refreshToken }),
        })
        const body = await resp.json().catch(() => null)
        if (resp.ok && body?.success && body?.data?.token) {
          session.storeTokens(body.data.token, body.data.refreshToken)
          return true
        }
      } catch {
        /* network failure: treat as renewal failure */
      }
      return false
    })().finally(() => {
      refreshInFlight = null
    })
  }
  return refreshInFlight
}

function redirectToLoginOnce() {
  if (redirectingToLogin) return
  redirectingToLogin = true
  session.clear()
  authChanged.emit()
  window.location.assign('/login')
}

export interface RequestOptions {
  redirectOnUnauthorized?: boolean
  isRetry?: boolean
  attempt?: number
}

export async function api<T = unknown>(
  method: string,
  path: string,
  body?: unknown,
  options: RequestOptions = {},
): Promise<Envelope<T>> {
  const { redirectOnUnauthorized = true, isRetry = false, attempt = 0 } = options

  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  const token = session.accessToken()
  if (token) headers['Authorization'] = `Bearer ${token}`
  const teamContext = session.contextTeamId()
  if (teamContext) headers['X-Context-Team-ID'] = String(teamContext)

  let resp: Response
  try {
    resp = await fetch(path.startsWith('/') ? path : `/api/${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    })
  } catch {
    // Network/transport failure: bounded back-off retries, then a zero-status
    // "network connection error" envelope (CRD 6511).
    if (attempt < MAX_RETRIES) {
      await new Promise((r) => setTimeout(r, BACKOFF_MS[attempt]))
      return api(method, path, body, { ...options, attempt: attempt + 1 })
    }
    return { success: false, message: t('error.network'), status: 0 }
  }

  const parsed: Envelope<T> | null = await resp.json().catch(() => null)

  if (resp.ok) {
    return parsed ?? { success: false, message: t('error.format'), status: resp.status }
  }

  if (resp.status === 401) {
    // One transparent renewal + single re-issue (CRD 6508-6509).
    if (redirectOnUnauthorized && !isRetry && session.refreshToken()) {
      if (await renewCredentials()) {
        return api(method, path, body, { ...options, isRetry: true })
      }
    }
    if (redirectOnUnauthorized) redirectToLoginOnce()
  }

  // Server-error retries only — never for 4xx (CRD 6520).
  if (resp.status >= 500 && attempt < MAX_RETRIES) {
    await new Promise((r) => setTimeout(r, BACKOFF_MS[attempt]))
    return api(method, path, body, { ...options, attempt: attempt + 1 })
  }

  return {
    success: false,
    ...(parsed ?? {}),
    message: (parsed?.message as string) || (parsed?.error as string) || statusMessage(resp.status),
    status: resp.status,
  }
}

export const get = <T = unknown>(path: string, options?: RequestOptions) =>
  api<T>('GET', path, undefined, options)
export const post = <T = unknown>(path: string, body?: unknown, options?: RequestOptions) =>
  api<T>('POST', path, body, options)
export const put = <T = unknown>(path: string, body?: unknown, options?: RequestOptions) =>
  api<T>('PUT', path, body, options)
export const del = <T = unknown>(path: string, options?: RequestOptions) =>
  api<T>('DELETE', path, undefined, options)

// ---------------------------------------------------------------------------
// Shared helpers (Epic 0 foundation): the list/upload/download primitives that
// every CRUD screen reuses, so individual pages stop hand-rolling them.
// ---------------------------------------------------------------------------

export type QueryValue = string | number | boolean | null | undefined

/// Build a query string from a params map, skipping null/undefined/'' values
/// and URL-encoding the rest. Returns '' (not '?') when nothing is present so
/// callers can always do `${path}${buildQuery(params)}`.
export function buildQuery(params: Record<string, QueryValue>): string {
  const usp = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value === null || value === undefined || value === '') continue
    usp.append(key, String(value))
  }
  const qs = usp.toString()
  return qs ? `?${qs}` : ''
}

/// The shape list endpoints settle on: a page of rows plus pagination meta.
/// The backend is inconsistent about envelope keys (data vs items vs a bare
/// array, pagination.total vs total), so `unwrapList` tolerates all of them.
export interface ListResult<T> {
  items: T[]
  total: number
  page: number
}

export function unwrapList<T>(resp: Envelope<T[] | { items?: T[] }>, page = 1): ListResult<T> {
  const data = resp.data as unknown
  const items: T[] = Array.isArray(data)
    ? (data as T[])
    : (((data as { items?: T[]; rows?: T[] })?.items ??
        (data as { rows?: T[] })?.rows ??
        []) as T[])
  const pag = (resp as { pagination?: { total?: number; page?: number } }).pagination
  const total = pag?.total ?? (resp as { total?: number }).total ?? items.length
  return { items, total, page: pag?.page ?? page }
}

/// multipart/form-data upload — the JSON `api()` path can't carry binaries.
/// Shares the bearer + team-context headers and the same envelope contract,
/// but lets the browser set the multipart boundary (no Content-Type override).
export async function upload<T = unknown>(
  path: string,
  form: FormData,
): Promise<Envelope<T>> {
  const headers: Record<string, string> = {}
  const token = session.accessToken()
  if (token) headers['Authorization'] = `Bearer ${token}`
  const teamContext = session.contextTeamId()
  if (teamContext) headers['X-Context-Team-ID'] = String(teamContext)

  let resp: Response
  try {
    resp = await fetch(path.startsWith('/') ? path : `/api/${path}`, {
      method: 'POST',
      headers,
      body: form,
    })
  } catch {
    return { success: false, message: t('error.network'), status: 0 }
  }
  const parsed: Envelope<T> | null = await resp.json().catch(() => null)
  if (resp.ok) {
    return parsed ?? { success: false, message: t('error.format'), status: resp.status }
  }
  if (resp.status === 401) redirectToLoginOnce()
  return {
    success: false,
    ...(parsed ?? {}),
    message: (parsed?.message as string) || (parsed?.error as string) || statusMessage(resp.status),
    status: resp.status,
  }
}

export interface DownloadResult {
  ok: boolean
  message?: string
}

/// Fetch a binary/report response and trigger a browser save, honouring the
/// server's Content-Disposition filename. Centralises the blob dance the
/// Reports screen previously hand-rolled.
export async function download(
  method: string,
  path: string,
  body?: unknown,
  fallbackName = 'download',
): Promise<DownloadResult> {
  const headers: Record<string, string> = {}
  const token = session.accessToken()
  if (token) headers['Authorization'] = `Bearer ${token}`
  const teamContext = session.contextTeamId()
  if (teamContext) headers['X-Context-Team-ID'] = String(teamContext)
  if (body !== undefined) headers['Content-Type'] = 'application/json'

  let resp: Response
  try {
    resp = await fetch(path.startsWith('/') ? path : `/api/${path}`, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    })
  } catch {
    return { ok: false, message: t('error.network') }
  }
  if (!resp.ok) {
    if (resp.status === 401) redirectToLoginOnce()
    return { ok: false, message: statusMessage(resp.status) }
  }
  const name =
    resp.headers.get('content-disposition')?.match(/filename="?([^"]+)"?/)?.[1] ?? fallbackName
  const blob = await resp.blob()
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = name
  document.body.appendChild(a)
  a.click()
  a.remove()
  URL.revokeObjectURL(url)
  return { ok: true }
}
