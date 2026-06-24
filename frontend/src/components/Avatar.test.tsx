import { render, fireEvent } from '@testing-library/react'
import { describe, it, expect } from 'vitest'
import { Avatar } from './Avatar'

describe('Avatar', () => {
  it('renders an <img> when src is provided', () => {
    const { container } = render(<Avatar name="Alice" src="https://cdn/x.png" />)
    const img = container.querySelector('img')
    expect(img).toBeTruthy()
    expect(img?.getAttribute('src')).toBe('https://cdn/x.png')
  })

  it('renders initials (no <img>) when src is absent', () => {
    const { container } = render(<Avatar name="Alice" />)
    expect(container.querySelector('img')).toBeNull()
    expect(container.textContent).toBe('ce') // last-two-chars behaviour
  })

  it('renders initials when src is empty string', () => {
    const { container } = render(<Avatar name="Bob" src="" />)
    expect(container.querySelector('img')).toBeNull()
  })

  it('retries (shows img) when src changes after a load error', () => {
    const { container, rerender } = render(<Avatar name="Alice" src="https://cdn/a.png" />)
    const img = container.querySelector('img')!
    fireEvent.error(img)
    // after error, falls back to initials
    expect(container.querySelector('img')).toBeNull()
    // new src → fresh attempt → img again
    rerender(<Avatar name="Bob" src="https://cdn/b.png" />)
    const img2 = container.querySelector('img')
    expect(img2).toBeTruthy()
    expect(img2?.getAttribute('src')).toBe('https://cdn/b.png')
  })
})
