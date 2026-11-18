// Host-owned, invocation-scoped Node execution entry. Never loaded as a page.
import type {} from 'electron'
import { readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { loadNodePluginPreload, type NodePluginRuntimeDescriptor } from './node-plugin-preload-bootstrap.ts'
import { parseFeatureTextResult } from '../../src/shared/feature-quick-result-contracts.ts'

const parentPort = process.parentPort
const controller = new AbortController()
const pending = new Map<number, { resolve(value: unknown): void; reject(error: Error): void }>()
let sequence = 0
let started = false
function request(method: string, payload: unknown): Promise<unknown> {
  if (controller.signal.aborted) return Promise.reject(new Error('Command cancelled.'))
  if (pending.size >= 16) return Promise.reject(new Error('Too many pending Host requests.'))
  const id = ++sequence
  return new Promise((resolve, reject) => { pending.set(id, { resolve, reject }); parentPort.postMessage({ type: 'capability', id, method, payload }) })
}
function cancel(): void {
  controller.abort()
  for (const waiter of pending.values()) waiter.reject(new Error('Command cancelled.'))
  pending.clear()
}
function encodeBytes(value: Uint8Array | ArrayBuffer): string {
  const bytes = value instanceof Uint8Array ? value : value instanceof ArrayBuffer ? new Uint8Array(value) : null
  if (!bytes) throw new TypeError('Expected Uint8Array or ArrayBuffer.')
  return Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString('base64')
}
function decodeBytes(value: unknown): Uint8Array | null {
  if (value === null) return null
  if (typeof value !== 'string' || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(value)) {
    throw new Error('Host returned invalid file data.')
  }
  const bytes = Buffer.from(value, 'base64')
  if (bytes.toString('base64') !== value) throw new Error('Host returned invalid file data.')
  return new Uint8Array(bytes.buffer, bytes.byteOffset, bytes.byteLength)
}
parentPort.on('message', event => {
  const message = event.data
  if (!message || typeof message !== 'object') return
  if (message.type === 'cancel') { cancel(); return }
  if (message.type === 'capability-result') {
    const waiter = pending.get(message.id)
    if (!waiter) return
    pending.delete(message.id)
    if (message.ok) waiter.resolve(message.value)
    else waiter.reject(new Error(typeof message.code === 'string' ? message.code : 'Host operation failed.'))
    return
  }
  if (message.type !== 'start' || started) return
  started = true
  void (async () => {
    const descriptor: NodePluginRuntimeDescriptor = message.descriptor
    let handler: unknown
    loadNodePluginPreload([`--webtools-plugin-runtime=${Buffer.from(JSON.stringify(descriptor)).toString('base64url')}`], {
      readFile: readFileSync,
      setPluginData: path => { process.env.WEBTOOLS_PLUGIN_DATA = path },
      requirePlugin: path => { handler = createRequire(path)(path) },
    })
    if (typeof handler !== 'function') throw new Error('Node command module must export a command handler function.')
    const grants: readonly string[] = message.capabilities
    const webtools = Object.freeze({
      context: Object.freeze({ current: () => request('context.current', null) }),
      shortcuts: Object.freeze({ current: () => request('shortcuts.current', null) }),
      tasks: Object.freeze({ request: (input: unknown) => request('tasks.request', input), list: () => request('tasks.list', null), remove: (code: string) => request('tasks.remove', { code }) }),
      config: Object.freeze({ get: () => request('config.get', null),
        set: async (key: string, value: string | boolean | number) => { await request('config.set', { key, value }) } }),
      storage: Object.freeze({ list: (prefix = '') => request('storage.list', { prefix }), get: (key: string) => request('storage.get', { key }),
        set: (key: string, value: unknown) => request('storage.set', { key, value }), delete: (key: string) => request('storage.delete', { key }),
        files: Object.freeze({
          list: (prefix = '') => request('storage.files.list', { prefix }),
          read: async (path: string) => decodeBytes(await request('storage.files.read', { path })),
          write: (path: string, data: Uint8Array | ArrayBuffer) => request('storage.files.write', { path, data: encodeBytes(data) }),
          delete: (path: string) => request('storage.files.delete', { path }),
        }),
        backup: Object.freeze({
          export: () => request('storage.backup.export', null),
          import: (backup: unknown) => request('storage.backup.import', { backup }),
        }),
      }),
      ...(grants.includes('system.media.image') ? { system: Object.freeze({ image: Object.freeze({ transform: (input: unknown) => request('system.imageTransform', { quality: 85, ...(input as object) }) }) }) } : {}),
      ...(grants.includes('sharedAI.complete') ? { ai: Object.freeze({ complete: (input: { prompt: string }) => request('ai.complete', { prompt: input.prompt }) }) } : {}),
      ...(grants.includes('clipboard.write') ? { clipboard: Object.freeze({ writeText: (text: string) => request('clipboard.write', { text }) }) } : {}),
      ...(grants.includes('external.open') ? { external: Object.freeze({ open: (url: string) => request('external.open', { url }) }) } : {}),
    })
    const context = await webtools.context.current()
    if (controller.signal.aborted) return
    const result = await handler(Object.freeze({ context, signal: controller.signal, webtools,
      ...(message.task ? { task: Object.freeze({ ...message.task }) } : {}),
      ...(message.mcpInput !== undefined ? { mcpInput: Object.freeze(message.mcpInput) } : {}),
      ...(message.resultTarget === 'super-panel' ? { resultTarget: 'super-panel' } : {}) }))
    if (message.mcpInput !== undefined) {
      if (controller.signal.aborted) { parentPort.postMessage({ type: 'cancelled' }); return }
      const json = JSON.stringify(result)
      if (!json || Buffer.byteLength(json, 'utf8') > 128 * 1024) throw new Error('Invalid MCP tool result size.')
      const output = JSON.parse(json)
      if (!output || typeof output !== 'object' || Array.isArray(output)) throw new Error('MCP tool must return a JSON object.')
      parentPort.postMessage({ type: 'completed', mcpResult: output })
      return
    }
    const parsed = result === undefined ? undefined : parseFeatureTextResult(result)
    if (parsed === null) throw new Error('Invalid command text result.')
    parentPort.postMessage(controller.signal.aborted ? { type: 'cancelled' } : { type: 'completed', ...(parsed ? { result: parsed } : {}) })
  })().catch(error => {
    parentPort.postMessage({ type: controller.signal.aborted ? 'cancelled' : 'failed',
      message: error instanceof Error ? error.message.slice(0, 4096) : 'Command execution failed.',
      stack: error instanceof Error ? error.stack?.slice(0, 8192) : undefined })
  })
})
