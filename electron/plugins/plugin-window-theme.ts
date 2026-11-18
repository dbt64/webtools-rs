import type { ThemePreference } from '../../src/shared/domain.ts'

export interface PluginWindowTheme {
  readonly scheme: 'light' | 'dark'
  readonly backgroundColor: string
  readonly css: string
}

export interface PluginThemeSource {
  readonly current: PluginWindowTheme
  subscribe(listener: (theme: PluginWindowTheme) => void): () => void
}

// Fixed semantic values only: no plugin input, element styles or component CSS.
function theme(scheme: 'light' | 'dark', colors: readonly string[]): PluginWindowTheme {
  const names = ['background', 'surface', 'surface-raised', 'text', 'muted', 'border', 'accent', 'accent-foreground']
  const chrome = [
    '--webtools-window-chrome-height: 42px',
    '--webtools-window-chrome-start-padding: 14px',
    '--webtools-window-chrome-end-padding: 8px',
    '--webtools-window-chrome-action-size: 30px',
    '--webtools-window-chrome-action-radius: 5px',
    '--webtools-window-chrome-action-gap: 8px',
    '--webtools-window-chrome-identity-height: 30px',
    '--webtools-window-chrome-identity-padding: 0 6px 0 9px',
    '--webtools-window-chrome-identity-gap: 8px',
    '--webtools-window-chrome-icon-size: 20px',
    '--webtools-window-chrome-font-size: 13px',
    '--webtools-window-chrome-background: var(--webtools-color-surface)',
    '--webtools-window-chrome-border: var(--webtools-color-border)',
    '--webtools-window-chrome-identity-background: var(--webtools-color-surface-raised)',
  ]
  return Object.freeze({ scheme, backgroundColor: colors[0], css: `:root { --webtools-color-scheme: ${scheme}; ${
    names.map((name, index) => `--webtools-color-${name}: ${colors[index]};`).join(' ')} ${chrome.join('; ')}; }` })
}

const light = theme('light', ['#f3f5f8', '#ffffff', '#f7f9fb', '#1b2633', '#5f6f80', '#e5eaf0', '#2477f3', '#ffffff'])
const dark = theme('dark', ['#0b0c0e', '#18191b', '#202124', '#f4f4f5', '#a1a1aa', '#303237', '#91baff', '#111b2c'])

export function resolvePluginWindowTheme(preference: ThemePreference, systemDark: boolean): PluginWindowTheme {
  return (preference === 'system' ? systemDark : preference === 'dark') ? dark : light
}
