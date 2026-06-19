import { useState } from 'react'

import { currentTheme, toggleTheme, type Theme } from '../theme'
import { Icon } from './Icon'

// Minimal top-bar control: shows a moon in light mode (click → dark) and a sun
// in dark mode (click → light). Holds its own display state; theme.ts owns the
// actual document/localStorage side effect.
export function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(currentTheme())
  return (
    <button
      className="cs-icon-btn"
      aria-label={theme === 'dark' ? '切換為淺色' : '切換為深色'}
      onClick={() => setTheme(toggleTheme())}
    >
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} w={18} />
    </button>
  )
}
