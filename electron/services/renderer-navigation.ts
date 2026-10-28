import { pathToFileURL } from 'node:url'

function normalizePath(value: string): string {
  let decoded = value
  try { decoded = decodeURIComponent(value) } catch { /* keep the encoded form */ }
  const trimmed = decoded.replace(/\/+$/, '')
  return process.platform === 'win32' ? trimmed.toLowerCase() : trimmed
}

/**
 * Navigation guard for the privileged Manager and Translation windows.
 * Only the exact renderer entry (or its exact development-server URL) may load;
 * every other navigation - including a file dropped onto the window - is denied.
 */
export function isAllowedRendererNavigation(rawUrl: string, expectedFile: string, developmentUrl?: string): boolean {
  let target: URL
  try { target = new URL(rawUrl) } catch { return false }
  if (developmentUrl) {
    try {
      const development = new URL(developmentUrl)
      if (target.origin === development.origin && normalizePath(target.pathname) === normalizePath(development.pathname)) return true
    } catch { /* fall through to the packaged entry */ }
  }
  if (target.protocol !== 'file:') return false
  try {
    const expected = pathToFileURL(expectedFile)
    return target.host === expected.host
      && !target.username && !target.password && !target.port
      && normalizePath(target.pathname) === normalizePath(expected.pathname)
  } catch { return false }
}
