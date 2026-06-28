import { render, screen, fireEvent, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'

const apiMock = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
}))

vi.mock('../api/client', () => apiMock)

vi.mock('../auth/permissions', () => ({
  can: () => true,
}))

vi.mock('../auth/session', () => ({
  session: { position: () => 'system_admin' },
}))

vi.mock('../i18n', () => ({
  t: (key: string) => key,
}))

import Channels from './Channels'

describe('Channels credential entry', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiMock.get.mockResolvedValue({ success: true, data: [] })
    apiMock.post.mockResolvedValue({ success: true })
    apiMock.put.mockResolvedValue({ success: true })
  })

  it('creates a LINE channel from the form values', async () => {
    render(<Channels />)

    // Wait for the initial list load to settle.
    await waitFor(() => expect(apiMock.get).toHaveBeenCalledWith('/api/channels'))

    fireEvent.change(screen.getByLabelText('LINE Channel ID'), {
      target: { value: 'C123' },
    })
    fireEvent.change(screen.getByLabelText('LINE Channel access token'), {
      target: { value: 'tok-abc' },
    })
    fireEvent.change(screen.getByLabelText('LINE Channel secret'), {
      target: { value: 'sec-xyz' },
    })

    fireEvent.click(screen.getByRole('button', { name: 'LINE 儲存' }))

    await waitFor(() => expect(apiMock.post).toHaveBeenCalled())
    const [path, body] = apiMock.post.mock.calls[0]
    expect(path).toBe('/api/channels')
    expect(body.platform).toBe('line')
    expect(body.lineConfig).toMatchObject({
      channelId: 'C123',
      channelAccessToken: 'tok-abc',
      channelSecret: 'sec-xyz',
    })
  })
})
