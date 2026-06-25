import { render, fireEvent } from '@testing-library/react'
import { describe, it, expect } from 'vitest'
import { MessageMedia, isMediaKind, kindFromMime } from './MessageMedia'

const base = { convId: 'c1', msgId: 'm1', content: '[x]' }

describe('MessageMedia', () => {
  it('image → <img> pointing at the preview proxy URL', () => {
    const { container } = render(<MessageMedia {...base} messageType="image" />)
    const img = container.querySelector('img')
    expect(img?.getAttribute('src')).toBe('/api/conversations/c1/messages/m1/media/preview')
  })

  it('sticker → <img> from the LINE sticker CDN', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="sticker" media={{ stickerId: '52002734' }} />,
    )
    const src = container.querySelector('img')?.getAttribute('src') ?? ''
    expect(src).toContain('stickershop.line-scdn.net')
    expect(src).toContain('52002734')
  })

  it('file → download link with the file name', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="file" media={{ fileName: 'report.pdf' }} />,
    )
    const a = container.querySelector('a')
    expect(a?.getAttribute('href')).toBe('/api/conversations/c1/messages/m1/media')
    expect(a?.textContent).toContain('report.pdf')
  })

  it('text/unknown → plain content, no <img>', () => {
    const { container } = render(<MessageMedia {...base} messageType="text" content="hello" />)
    expect(container.querySelector('img')).toBeNull()
    expect(container.textContent).toContain('hello')
  })

  it('image onError falls back to text content', () => {
    const { container } = render(<MessageMedia {...base} messageType="image" content="[Image]" />)
    const img = container.querySelector('img')!
    fireEvent.error(img)
    expect(container.querySelector('img')).toBeNull()
    expect(container.textContent).toContain('[Image]')
  })

  it('uses srcUrl directly for an agent image (no proxy URL)', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="image" srcUrl="https://files/x.png" />,
    )
    expect(container.querySelector('img')?.getAttribute('src')).toBe('https://files/x.png')
  })

  it('kindFromMime maps mimes', () => {
    expect(kindFromMime('image/png')).toBe('image')
    expect(kindFromMime('video/mp4')).toBe('video')
    expect(kindFromMime('audio/m4a')).toBe('audio')
    expect(kindFromMime('application/pdf')).toBe('file')
    expect(kindFromMime(undefined)).toBe('file')
  })

  it('isMediaKind recognizes media kinds', () => {
    expect(isMediaKind('image')).toBe(true)
    expect(isMediaKind('text')).toBe(false)
  })

  it('file branch falls back to content for the filename (agent attachment)', () => {
    const { container } = render(
      <MessageMedia {...base} messageType="file" srcUrl="https://files/report.pdf" content="report.pdf" />,
    )
    const a = container.querySelector('a')
    expect(a?.getAttribute('href')).toBe('https://files/report.pdf')
    expect(a?.textContent).toContain('report.pdf')
  })
})
