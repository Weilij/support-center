// Setup wizard (CRD §9.2, lines 6855-6979): multi-step flow — authenticate
// the hosting account, collect deployment configuration, trigger
// provisioning, poll live status, present generated admin credentials.

import { useEffect, useRef, useState } from 'react'

const INSTALLER = '/installer' // dev proxy to the provisioning service

type Step = 'auth' | 'config' | 'provisioning' | 'done' | 'failed'

interface RunStatus {
  status: string
  progressPercent?: number
  currentStep?: string | null
  completedSteps?: string[]
  adminCredentials?: { email: string; password: string; note?: string }
  error?: string
}

export default function Install() {
  const [step, setStep] = useState<Step>('auth')
  const [apiToken, setApiToken] = useState('')
  const [accountId, setAccountId] = useState('')
  const [projectName, setProjectName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [run, setRun] = useState<RunStatus | null>(null)
  const [oauthBusy, setOauthBusy] = useState(false)
  const runId = useRef<string | null>(null)
  const timer = useRef<number | null>(null)

  const call = async (path: string, body?: unknown) => {
    const resp = await fetch(`${INSTALLER}${path}`, {
      method: body === undefined ? 'GET' : 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: body === undefined ? undefined : JSON.stringify(body),
    })
    const data = await resp.json().catch(() => ({}))
    return { ok: resp.ok, data }
  }

  const verifyToken = async (e: React.FormEvent) => {
    e.preventDefault()
    setError(null)
    const { ok, data } = await call('/auth/token', { apiToken, accountId })
    if (ok) setStep('config')
    else setError(String(data.error ?? '驗證失敗'))
  }

  const startOAuth = async () => {
    setError(null)
    setOauthBusy(true)
    const redirectUri = `${window.location.origin}${window.location.pathname}`
    const { ok, data } = await call(`/oauth/authorize?redirect_uri=${encodeURIComponent(redirectUri)}`)
    setOauthBusy(false)
    if (!ok || !data.authUrl || !data.verifier) {
      setError(String(data.error ?? 'OAuth 啟動失敗'))
      return
    }
    window.sessionStorage.setItem('installerOauthVerifier', String(data.verifier))
    window.location.assign(String(data.authUrl))
  }

  const startProvision = async (e: React.FormEvent) => {
    e.preventDefault()
    setError(null)
    const { ok, data } = await call('/deployment/start', { projectName, apiToken, accountId })
    if (!ok) {
      setError(String(data.error ?? '啟動失敗'))
      return
    }
    runId.current = String(data.deploymentId)
    setStep('provisioning')
  }

  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const code = params.get('code')
    if (!code) return
    const verifier = window.sessionStorage.getItem('installerOauthVerifier') ?? ''
    const redirectUri = `${window.location.origin}${window.location.pathname}`
    setOauthBusy(true)
    setError(null)
    call('/oauth/callback', { code, verifier, redirectUri }).then(({ ok, data }) => {
      setOauthBusy(false)
      window.sessionStorage.removeItem('installerOauthVerifier')
      window.history.replaceState({}, document.title, window.location.pathname)
      if (!ok || !data.apiToken) {
        setError(String(data.error ?? 'OAuth 驗證失敗'))
        return
      }
      setApiToken(String(data.apiToken))
      setStep('config')
    })
  }, [])

  // Live status polling (CRD §9.2).
  useEffect(() => {
    if (step !== 'provisioning') return
    timer.current = window.setInterval(async () => {
      if (!runId.current) return
      const { ok, data } = await call(`/deployment/status/${runId.current}`)
      if (!ok) return
      const status = data as RunStatus
      setRun(status)
      if (status.status === 'completed') {
        setStep('done')
      } else if (status.status === 'failed') {
        setError(status.error ?? '佈建失敗，已自動回收部分資源')
        setStep('failed')
      }
    }, 500)
    return () => {
      if (timer.current) window.clearInterval(timer.current)
    }
  }, [step])

  return (
    <main style={{ maxWidth: 520, margin: '8vh auto', fontFamily: 'sans-serif' }}>
      <h1>安裝精靈</h1>
      <ol style={{ display: 'flex', gap: 12, listStyle: 'none', padding: 0, fontSize: 13 }}>
        {(['帳號驗證', '部署設定', '佈建中', '完成'] as const).map((label, i) => {
          const active = ['auth', 'config', 'provisioning', 'done'].indexOf(step) >= i
          return <li key={label} style={{ fontWeight: active ? 'bold' : 'normal' }}>{i + 1}. {label}</li>
        })}
      </ol>
      {error && <p role="alert" style={{ color: 'crimson' }}>{error}</p>}

      {step === 'auth' && (
        <form onSubmit={verifyToken} style={{ display: 'grid', gap: 8 }}>
          <p>輸入雲端帳號的 API Token 與帳號識別碼。</p>
          <input value={apiToken} onChange={(e) => setApiToken(e.target.value)}
                 placeholder="API Token" required />
          <input value={accountId} onChange={(e) => setAccountId(e.target.value)}
                 placeholder="Account ID" required />
          <button type="submit">驗證</button>
          <button type="button" onClick={startOAuth} disabled={oauthBusy}>
            {oauthBusy ? '連線中…' : '使用 Cloudflare OAuth'}
          </button>
        </form>
      )}

      {step === 'config' && (
        <form onSubmit={startProvision} style={{ display: 'grid', gap: 8 }}>
          <p>租戶名稱：3-50 字元，小寫字母、數字與連字號。</p>
          <input value={projectName} onChange={(e) => setProjectName(e.target.value)}
                 placeholder="my-support-center" pattern="[a-z0-9-]{3,50}" required />
          <button type="submit">開始佈建</button>
        </form>
      )}

      {step === 'provisioning' && (
        <div>
          <progress value={run?.progressPercent ?? 0} max={100} style={{ width: '100%' }} />
          <p>目前步驟：{run?.currentStep ?? '…'}</p>
          <ul>
            {(run?.completedSteps ?? []).map((s) => <li key={s}>✓ {s}</li>)}
          </ul>
        </div>
      )}

      {step === 'done' && run?.adminCredentials && (
        <div>
          <h2>佈建完成 🎉</h2>
          <p>請記下初始管理員憑證（僅顯示一次）：</p>
          <pre style={{ background: '#f6f6f6', padding: 12 }}>
            {`帳號: ${run.adminCredentials.email}\n密碼: ${run.adminCredentials.password}`}
          </pre>
          <p><small>{run.adminCredentials.note}</small></p>
        </div>
      )}

      {step === 'failed' && <p>佈建失敗。部分建立的資源已自動回收，可安全重試。</p>}
    </main>
  )
}
