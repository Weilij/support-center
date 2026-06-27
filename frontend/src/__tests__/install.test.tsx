import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import Install from '../pages/Install'

function jsonResponse(ok: boolean, data: unknown) {
  return {
    ok,
    json: async () => data,
  }
}

describe('Install page', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('shows post-deploy admin setup instead of fabricated credentials', async () => {
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.endsWith('/installer/auth/token')) {
        return jsonResponse(true, { provider: 'cloudflare', accountId: 'acc_123' })
      }
      if (url.endsWith('/installer/deployment/start')) {
        return jsonResponse(true, { deploymentId: 'run-1', status: 'running' })
      }
      if (url.endsWith('/installer/deployment/status/run-1')) {
        return jsonResponse(true, {
          status: 'completed',
          progressPercent: 100,
          currentStep: null,
          completedSteps: ['database', 'kv-sessions', 'frontend-site'],
          adminSetup: {
            required: true,
            note: 'Create the first administrator through the deployed backend setup flow.',
          },
        })
      }
      return jsonResponse(false, { error: `unexpected request: ${url}` })
    })
    vi.stubGlobal('fetch', fetchMock)

    render(<Install />)

    fireEvent.change(screen.getByPlaceholderText('API Token'), { target: { value: 'tok_live' } })
    fireEvent.change(screen.getByPlaceholderText('Account ID'), { target: { value: 'acc_123' } })
    fireEvent.click(screen.getByText('驗證'))

    await screen.findByText('開始佈建')
    fireEvent.change(screen.getByPlaceholderText('my-support-center'), {
      target: { value: 'smoke-tenant' },
    })
    fireEvent.click(screen.getByText('開始佈建'))

    await waitFor(() => expect(screen.getByText('佈建完成')).toBeTruthy())
    expect(screen.getByText(/建立第一位系統管理員/)).toBeTruthy()
    expect(screen.queryByText(/帳號:/)).toBeNull()
    expect(screen.queryByText(/密碼:/)).toBeNull()
  })
})
