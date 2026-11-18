import { BrowserWindow, nativeImage, session } from 'electron'
import { createHash, randomUUID } from 'node:crypto'
import { BridgeWireError } from '../../plugin-sdk/declarative-v1/web/protocol.mjs'
import type { BrowserStep, ImageTransform } from '../../plugin-sdk/declarative-v1/web/capability-methods.mjs'

const fail = (code: 'INVALID_REQUEST' | 'RUNTIME_CLOSED' | 'LIMIT_EXCEEDED' | 'TIMEOUT') => { throw new BridgeWireError(code) }
export function workflowUrl(value: string): string {
  const url = new URL(value)
  if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password) return fail('INVALID_REQUEST')
  return url.href
}
export function browserPartition(pluginId: string): string {
  return 'persist:plugin-browser-' + createHash('sha256').update(pluginId).digest('hex')
}
export async function clearPluginBrowserData(pluginId: string): Promise<void> {
  const current = session.fromPartition(browserPartition(pluginId))
  await current.clearStorageData()
  await current.clearCache()
  await current.closeAllConnections()
}

/** Every browser belongs to one runtime signal; plugin pages never receive Electron objects. */
export function createPluginWorkflowEffects() {
  const browsers = new WeakMap<AbortSignal, Map<string, { window: BrowserWindow; busy: boolean }>>()
  function owned(signal: AbortSignal) {
    if (signal.aborted) return fail('RUNTIME_CLOSED')
    let entries = browsers.get(signal)
    if (!entries) {
      entries = new Map(); browsers.set(signal, entries)
      const ownedEntries = entries
      signal.addEventListener('abort', () => {
        for (const entry of ownedEntries.values()) if (!entry.window.isDestroyed()) entry.window.destroy()
        ownedEntries.clear()
      }, { once: true })
    }
    return entries
  }
  async function bounded<T>(window: BrowserWindow, work: Promise<T>): Promise<T> {
    let timer: ReturnType<typeof setTimeout> | undefined
    let closed: () => void = () => {}
    try {
      return await Promise.race([work, new Promise<never>((_, reject) => {
        closed = () => reject(new BridgeWireError('RUNTIME_CLOSED'))
        window.once('closed', closed)
        timer = setTimeout(() => { if (!window.isDestroyed()) window.destroy(); reject(new BridgeWireError('TIMEOUT')) }, 15000)
      })])
    } finally { clearTimeout(timer); window.removeListener('closed', closed) }
  }
  return {
    browserOpen: async (pluginId: string, url: string, lifetime: AbortSignal) => {
      const entries = owned(lifetime)
      if (entries.size >= 2) return fail('LIMIT_EXCEEDED')
      const target = workflowUrl(url)
      const partition = browserPartition(pluginId)
      const browserSession = session.fromPartition(partition)
      browserSession.setPermissionRequestHandler((_contents, _permission, callback) => callback(false))
      browserSession.setPermissionCheckHandler(() => false)
      const window = new BrowserWindow({ title: 'WebTools · 插件网页自动化', width: 1000, height: 720,
        webPreferences: { partition, sandbox: true, contextIsolation: true, nodeIntegration: false, webSecurity: true, webviewTag: false } })
      const id = randomUUID()
      entries.set(id, { window, busy: false })
      window.once('closed', () => entries.delete(id))
      window.webContents.setWindowOpenHandler(() => ({ action: 'deny' }))
      const allowNavigation = (event: Electron.Event, next: string) => { try { workflowUrl(next) } catch { event.preventDefault() } }
      window.webContents.on('will-navigate', allowNavigation)
      window.webContents.on('will-redirect', allowNavigation)
      // Downloads need an explicit file API; web pages cannot silently write to Downloads.
      const stopDownload = (event: Electron.Event, _item: Electron.DownloadItem, contents: Electron.WebContents) => {
        if (contents === window.webContents) event.preventDefault()
      }
      browserSession.on('will-download', stopDownload)
      window.once('closed', () => browserSession.removeListener('will-download', stopDownload))
      try { await bounded(window, window.loadURL(target)); return { id } }
      catch (error) { if (!window.isDestroyed()) window.destroy(); throw error }
    },
    browserStep: async (input: BrowserStep, lifetime: AbortSignal, signal: AbortSignal) => {
      const entry = owned(lifetime).get(input.id)
      if (!entry || entry.window.isDestroyed()) return fail('RUNTIME_CLOSED')
      if (entry.busy) return fail('LIMIT_EXCEEDED')
      entry.busy = true
      const window = entry.window
      const abort = () => { if (!window.isDestroyed()) window.destroy() }
      signal.addEventListener('abort', abort, { once: true })
      try {
        if (signal.aborted) return fail('RUNTIME_CLOSED')
        if (input.action === 'navigate') { await bounded(window, window.loadURL(workflowUrl(input.value))); return null }
        if (input.action === 'screenshot') {
          const image = await bounded(window, window.webContents.capturePage())
          const data = image.toDataURL(); if (data.length > 15000000) return fail('LIMIT_EXCEEDED'); return data
        }
        // Only fixed operations are evaluated; selector/value are encoded as data, never source.
        const script = `(() => { const input = ${JSON.stringify(input)}; const element = document.querySelector(input.selector);
          if (!element) throw new Error('Element not found: ' + input.selector);
          if (input.action === 'text') return (element.innerText ?? element.textContent ?? '').slice(0, 65536);
          if (input.action === 'click') { element.click(); return null; }
          if (!(element instanceof HTMLInputElement || element instanceof HTMLTextAreaElement)) throw new Error('Element is not a text input');
          const setter = Object.getOwnPropertyDescriptor(element instanceof HTMLInputElement ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype, 'value').set;
          setter.call(element, input.value); element.dispatchEvent(new Event('input', { bubbles: true })); element.dispatchEvent(new Event('change', { bubbles: true })); return null;
        })()`
        return await bounded(window, window.webContents.executeJavaScript(script, true)) as string | null
      } finally { entry.busy = false; signal.removeEventListener('abort', abort) }
    },
    browserClose: async (id: string, lifetime: AbortSignal) => {
      const entry = owned(lifetime).get(id)
      if (entry && !entry.window.isDestroyed()) entry.window.destroy()
    },
    imageTransform: async (input: ImageTransform) => {
      const source = nativeImage.createFromDataURL(input.dataUrl)
      if (source.isEmpty()) return fail('INVALID_REQUEST')
      const size = source.getSize()
      if (size.width * size.height > 32 * 1024 * 1024) return fail('LIMIT_EXCEEDED')
      const height = input.height ?? Math.max(1, Math.round(size.height * input.width / size.width))
      if (height > 4096 || input.width * height > 16 * 1024 * 1024) return fail('LIMIT_EXCEEDED')
      const image = source.resize({ width: input.width, height, quality: 'best' })
      const bytes = input.format === 'jpeg' ? image.toJPEG(input.quality) : image.toPNG()
      if (bytes.length > 8 * 1024 * 1024) return fail('LIMIT_EXCEEDED')
      return { dataUrl: `data:image/${input.format};base64,${bytes.toString('base64')}`, ...image.getSize(), bytes: bytes.length }
    },
  }
}
