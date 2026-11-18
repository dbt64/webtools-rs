import { BridgeWireError, measureUtf8, type JsonValue } from '../../plugin-sdk/declarative-v1/web/protocol.mjs'
import { validateCapabilityRequest, validateCapabilityResult, type CapabilityRequest } from '../../plugin-sdk/declarative-v1/web/capability-methods.mjs'
import type { CapabilityV2 } from '../../plugin-sdk/declarative-v1/runtime/manifest-v2.mjs'
import type { PluginConsent } from './permission-broker.ts'
import type { PluginStore } from './plugin-store.ts'
import { isWebFeatureAuthority, type WebFeatureAuthority, type WebCapability } from './web-feature-authority.ts'
import { isValidatedPluginPackageSnapshotV3 } from './validated-plugin-package-snapshot.ts'
import type { SharedAIService } from '../services/shared-ai-service.ts'
import { validateInvocationContext, acceptsInvocationContext, type InvocationContext } from '../../plugin-sdk/declarative-v1/web/invocation-context.mjs'

export type InvocationTextContext = InvocationContext & { readonly ageMs: number }
export interface ScreenDisplayInfo {
  id: string
  label: string
  bounds: { x: number; y: number; width: number; height: number }
  workArea: { x: number; y: number; width: number; height: number }
  scaleFactor: number
  rotation: 0 | 90 | 180 | 270
  primary: boolean
}
export interface WebCapabilityEffects {
  mediaTranscode?(input: import('../../plugin-sdk/declarative-v1/web/capability-methods.mjs').MediaTranscode, signal: AbortSignal): Promise<{ path: string } | null>
  browserOpen?(pluginId: string, url: string, lifetime: AbortSignal): Promise<{ id: string }>
  browserStep?(input: import('../../plugin-sdk/declarative-v1/web/capability-methods.mjs').BrowserStep, lifetime: AbortSignal, signal: AbortSignal): Promise<string | null>
  browserClose?(id: string, lifetime: AbortSignal): Promise<void>
  imageTransform?(input: import('../../plugin-sdk/declarative-v1/web/capability-methods.mjs').ImageTransform): Promise<{ dataUrl: string; width: number; height: number; bytes: number }>
  scheduledTasks?(method: 'request' | 'list' | 'remove', pluginId: string, featureId: string, payload: JsonValue): Promise<JsonValue>
  confirm(request: PluginConsent, signal?: AbortSignal): Promise<boolean>
  hasForegroundGesture(): boolean
  isAIConfigured(): Promise<boolean>
  completeAI(prompt: string, signal: AbortSignal): Promise<{ text: string; outputTokens?: number | null }>
  clipboardWrite(text: string): void | Promise<void>
  clipboardReadText?(): Promise<string>
  clipboardReadImage?(): Promise<string | null>
  clipboardWriteImage?(dataUrl: string): Promise<void>
  clipboardReadFiles?(): Promise<string[]>
  clipboardWriteFiles?(paths: string[]): Promise<void>
  externalOpen?(url: string): Promise<void>
  choosePaths?(kind: 'file' | 'directory', multiple: boolean): Promise<string[]>
  openPath?(path: string): Promise<string>
  notify?(title: string, body: string): Promise<void>
  pasteText?(text: string): Promise<void>
  getDisplays?(): Promise<ScreenDisplayInfo[]>
  captureScreen?(displayId?: string): Promise<string>
  getFeatureShortcut?(pluginId: string, featureId: string): string | null
}
interface Dependencies {
  authority: WebFeatureAuthority; store: PluginStore; effects: WebCapabilityEffects
  isRuntimeCurrent(): boolean; context?: InvocationTextContext; now?: () => number
  /** Node commands consume one activation gesture; the web runtime also shows a Host confirmation. */
  requireExternalConfirmation?: boolean
}
/** Reuse SharedAIService; retain only output usage internally and strip provider metadata. */
export async function completeSharedAIForWeb(service: Pick<SharedAIService, 'complete'>, prompt: string, signal: AbortSignal) {
  let outputTokens: number | null = null
  const result = await service.complete([{ role: 'user', content: prompt }], signal, 1024, tokens => { outputTokens = tokens })
  return { text: result.text, outputTokens }
}
const fail = (code: ConstructorParameters<typeof BridgeWireError>[0]): never => { throw new BridgeWireError(code) }
function abortable<T>(work: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) return Promise.reject(new BridgeWireError('CANCELLED'))
  return new Promise((resolve, reject) => {
    const abort = () => { signal.removeEventListener('abort', abort); reject(new BridgeWireError('CANCELLED')) }
    signal.addEventListener('abort', abort, { once: true })
    work.then(value => { signal.removeEventListener('abort', abort); resolve(value) }, error => { signal.removeEventListener('abort', abort); reject(error) })
  })
}

/** One active runtime; registry authority, namespaces, credentials and effects stay Main-owned. */
export class WebHostCapabilities {
  private readonly deps: Dependencies
  private readonly now: () => number
  private readonly controller = new AbortController()
  private readonly rates = new Map<string, number[]>()
  private context: InvocationContext | null = null
  private contextCreatedAt = 0
  private contextDelivered = false
  private contextExpired = false
  private contextTimer?: ReturnType<typeof setTimeout>
  private aiBusy = false
  private removeRevoke: () => void = () => {}
  constructor(deps: Dependencies) {
    if (!isWebFeatureAuthority(deps.authority) || !deps.authority.isCurrent()) fail('UNAUTHORIZED')
    this.deps = { authority: deps.authority, store: deps.store, effects: deps.effects, isRuntimeCurrent: deps.isRuntimeCurrent,
      requireExternalConfirmation: deps.requireExternalConfirmation }
    this.now = deps.now ?? (() => performance.now())
    this.setContext(deps.context)
    this.removeRevoke = deps.authority.onRevoke(() => this.dispose())
  }
  setContext(input?: InvocationTextContext): void {
    this.context = null; this.contextDelivered = false; this.contextExpired = false; clearTimeout(this.contextTimer)
    const feature = this.deps.authority.snapshot.manifest.features.find(f => f.id === this.deps.authority.featureId)!
    if (input) {
      const { ageMs, ...payload } = input
      const context = validateInvocationContext(payload)
      if (!context || !acceptsInvocationContext(feature.acceptedContexts, context)) return
      if (input.source === 'clipboard' && !this.deps.authority.isCurrent('clipboard.read')) fail('PERMISSION_DENIED')
      if (!Number.isFinite(ageMs) || ageMs < -16 || ageMs >= 30000) fail('CONTEXT_EXPIRED')
      this.context = context
      this.contextCreatedAt = this.now() - Math.max(0, ageMs); this.armContextExpiry(30000)
    }
  }
  dispose(): void {
    this.controller.abort(); this.context = null; clearTimeout(this.contextTimer); this.rates.clear(); this.removeRevoke()
  }
  private armContextExpiry(ttl: number): void {
    clearTimeout(this.contextTimer)
    this.contextTimer = setTimeout(() => {
      this.context = null
      this.contextExpired = true
    }, Math.max(0, this.contextCreatedAt + ttl - this.now()))
    this.contextTimer.unref?.()
  }
  private check(signal: AbortSignal, capability?: WebCapability): void {
    if (!this.deps.authority.isCurrent() || !this.deps.isRuntimeCurrent()) fail('STALE_RUNTIME')
    if (signal.aborted || this.controller.signal.aborted) fail('CANCELLED')
    if (capability && !this.deps.authority.isCurrent(capability)) fail('PERMISSION_DENIED')
  }
  private rate(name: string, limit: number): void {
    const now = this.now(); const times = (this.rates.get(name) ?? []).filter(time => now - time < 60000)
    if (times.length >= limit) fail('LIMIT_EXCEEDED')
    times.push(now); this.rates.set(name, times)
  }
  private async consent(kind: 'ai' | 'clipboard' | 'external', text: string, signal: AbortSignal, capability: CapabilityV2): Promise<void> {
    this.check(signal, capability)
    if (!this.deps.effects.hasForegroundGesture()) fail('USER_CANCELLED')
    const manifest = this.deps.authority.snapshot.manifest
    const preview = text.length > 4096 ? `${text.slice(0, 4096)}\n…（预览已缩略，共 ${measureUtf8(text)} UTF-8 字节）` : text
    const accepted = await abortable(this.deps.effects.confirm({ kind, pluginId: manifest.id, name: manifest.name,
      version: manifest.version, manifestVersion: manifest.manifestVersion,
      ...(manifest.manifestVersion === 3 ? { runtimeKind: manifest.runtime.kind } : {}), preview }, signal), signal)
    this.check(signal, capability)
    if (!accepted) fail('USER_CANCELLED')
  }
  async dispatch(value: CapabilityRequest, inputSignal: AbortSignal): Promise<JsonValue> {
    const request = validateCapabilityRequest(value.method, value.payload)
    const signal = AbortSignal.any([inputSignal, this.controller.signal])
    this.check(signal)
    try {
      switch (request.method) {
        case 'system.mediaTranscode': {
          this.check(signal, 'system.media.transcode')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          if (!this.deps.effects.mediaTranscode) return fail('NOT_AVAILABLE')
          const result = await this.deps.effects.mediaTranscode(request.payload, signal)
          this.check(signal, 'system.media.transcode'); validateCapabilityResult(request.method, result); return result
        }

        case 'system.browserOpen': case 'system.browserStep': case 'system.browserClose': case 'system.imageTransform': {
          const capability = request.method === 'system.imageTransform' ? 'system.media.image' : 'system.browser.automation'
          this.check(signal, capability)
          const effects = this.deps.effects
          let result: JsonValue = null
          if (request.method === 'system.browserOpen') {
            if (!effects.browserOpen) return fail('NOT_AVAILABLE')
            if (!effects.hasForegroundGesture()) return fail('USER_CANCELLED')
            result = await effects.browserOpen(this.deps.authority.snapshot.manifest.id, request.payload.url, this.controller.signal)
            if (signal.aborted && effects.browserClose) await effects.browserClose((result as { id: string }).id, this.controller.signal)
          } else if (request.method === 'system.browserStep') {
            if (!effects.browserStep) return fail('NOT_AVAILABLE')
            result = await effects.browserStep(request.payload, this.controller.signal, signal)
          } else if (request.method === 'system.browserClose') {
            if (!effects.browserClose) return fail('NOT_AVAILABLE')
            await effects.browserClose(request.payload.id, this.controller.signal)
          } else {
            if (!effects.imageTransform) return fail('NOT_AVAILABLE')
            result = await effects.imageTransform(request.payload)
          }
          this.check(signal, capability); validateCapabilityResult(request.method, result); return result
        }

        case 'tasks.request': case 'tasks.list': case 'tasks.remove': {
          this.check(signal, 'plugin.tasks')
          if (this.deps.authority.development || !this.deps.effects.scheduledTasks) return fail('NOT_AVAILABLE')
          if (request.method === 'tasks.request') {
            if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
            this.rate('schedule', 8)
            const manifest = this.deps.authority.snapshot.manifest
            const accepted = await abortable(this.deps.effects.confirm({ kind: 'schedule', pluginId: manifest.id, name: manifest.name,
              version: manifest.version, manifestVersion: manifest.manifestVersion,
              ...(manifest.manifestVersion === 3 ? { runtimeKind: manifest.runtime.kind } : {}),
              preview: `${request.payload.description}\n任务代码：${request.payload.code}\n${JSON.stringify(request.payload.schedule)}\nCron 使用本机时区。周期任务不重放退出期间错过的次数。` }, signal), signal)
            this.check(signal, 'plugin.tasks'); if (!accepted) return fail('USER_CANCELLED')
          }
          const result = await abortable(this.deps.effects.scheduledTasks(request.method.slice(6) as 'request' | 'list' | 'remove',
            this.deps.authority.snapshot.manifest.id, this.deps.authority.featureId, request.payload), signal)
          this.check(signal, 'plugin.tasks'); validateCapabilityResult(request.method, result); return result
        }
        case 'tasks.subscribe': case 'tasks.complete': return fail('NOT_AVAILABLE')
        case 'system.pasteText': {
          this.check(signal, 'system.input.paste')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          if (!this.deps.effects.pasteText) return fail('NOT_AVAILABLE')
          await abortable(this.deps.effects.pasteText(request.payload.text), signal)
          this.check(signal, 'system.input.paste'); return null
        }
        case 'system.notify': {
          this.check(signal, 'system.notification')
          if (!this.deps.effects.notify) return fail('NOT_AVAILABLE')
          await abortable(this.deps.effects.notify(request.payload.title, request.payload.body), signal)
          this.check(signal, 'system.notification'); return null
        }
        case 'system.getDisplays': {
          this.check(signal, 'system.screen.capture')
          if (!this.deps.effects.getDisplays) return fail('NOT_AVAILABLE')
          const result = await abortable(this.deps.effects.getDisplays(), signal)
          this.check(signal, 'system.screen.capture')
          validateCapabilityResult(request.method, result); return result as unknown as JsonValue
        }
        case 'system.captureScreen': {
          this.check(signal, 'system.screen.capture')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          if (!this.deps.effects.captureScreen) return fail('NOT_AVAILABLE')
          const result = await abortable(this.deps.effects.captureScreen(request.payload.displayId), signal)
          this.check(signal, 'system.screen.capture')
          validateCapabilityResult(request.method, result); return result
        }
        case 'clipboard.readText': {
          this.check(signal, 'clipboard.current.read')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          if (!this.deps.effects.clipboardReadText) return fail('NOT_AVAILABLE')
          const result = await abortable(this.deps.effects.clipboardReadText(), signal)
          this.check(signal, 'clipboard.current.read')
          validateCapabilityResult(request.method, result); return result
        }
        case 'clipboard.readImage': case 'clipboard.readFiles': {
          this.check(signal, 'clipboard.current.read')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          const effect = request.method === 'clipboard.readImage' ? this.deps.effects.clipboardReadImage : this.deps.effects.clipboardReadFiles
          if (!effect) return fail('NOT_AVAILABLE')
          const result: string | string[] | null = await abortable(Promise.resolve(effect() as Promise<string | string[] | null>), signal)
          this.check(signal, 'clipboard.current.read'); validateCapabilityResult(request.method, result); return result
        }
        case 'clipboard.writeImage': case 'clipboard.writeFiles': {
          this.check(signal, 'clipboard.write')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          if (request.method === 'clipboard.writeImage') {
            if (!this.deps.effects.clipboardWriteImage) return fail('NOT_AVAILABLE')
            await abortable(this.deps.effects.clipboardWriteImage(request.payload.dataUrl), signal)
          } else {
            if (!this.deps.effects.clipboardWriteFiles) return fail('NOT_AVAILABLE')
            await abortable(this.deps.effects.clipboardWriteFiles(request.payload.paths), signal)
          }
          this.check(signal, 'clipboard.write'); return null
        }
        case 'files.select': {
          this.check(signal, 'system.file.select')
          if (!this.deps.effects.choosePaths) return fail('NOT_AVAILABLE')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          const paths = await abortable(this.deps.effects.choosePaths(request.payload.kind, request.payload.multiple), signal)
          this.check(signal, 'system.file.select'); validateCapabilityResult(request.method, paths); return paths
        }
        case 'files.openPath': {
          this.check(signal, 'system.path.open')
          if (!this.deps.effects.openPath) return fail('NOT_AVAILABLE')
          if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          if (await this.deps.effects.openPath(request.payload.path)) return fail('NOT_AVAILABLE')
          this.check(signal, 'system.path.open'); return null
        }
        case 'context.current': {
          if (this.contextExpired || this.context && this.now() >= this.contextCreatedAt + (this.contextDelivered ? 600000 : 30000)) {
            this.context = null; this.contextExpired = true; clearTimeout(this.contextTimer); fail('CONTEXT_EXPIRED')
          }
          if (!this.context) return null
          const result = this.context.type === 'text' ? { ...this.context } : { ...this.context, items: this.context.items.map(item => ({ ...item })) }
          this.contextDelivered = true; this.armContextExpiry(600000)
          if (result.source === 'clipboard') { this.context = null; clearTimeout(this.contextTimer) }
          return result
        }
        case 'shortcuts.current': {
          const pluginId = this.deps.authority.snapshot.manifest.id
          const result = this.deps.effects.getFeatureShortcut?.(pluginId, this.deps.authority.featureId) ?? null
          validateCapabilityResult(request.method, result)
          return result
        }
        case 'storage.get': case 'storage.set': case 'storage.delete': case 'storage.list':
        case 'storage.files.list': case 'storage.files.read': case 'storage.files.write': case 'storage.files.delete':
        case 'storage.backup.export': case 'storage.backup.import': {
          const readOnly = request.method === 'storage.get' || request.method === 'storage.list'
            || request.method === 'storage.files.list' || request.method === 'storage.files.read' || request.method === 'storage.backup.export'
          const cap = readOnly ? 'plugin.storage.read' : 'plugin.storage.write'
          this.check(signal, cap); this.rate('storage', 60)
          const id = this.deps.authority.snapshot.manifest.id
          let result: unknown = null
          const mayCommit = () => !signal.aborted && this.deps.isRuntimeCurrent() && this.deps.authority.isCurrent(cap)
          if (request.method === 'storage.list') result = await abortable(this.deps.store.listDataKeys(id, request.payload.prefix), signal)
          else if (request.method === 'storage.get') result = await abortable(this.deps.store.readData(id, request.payload.key), signal)
          else if (request.method === 'storage.set') await abortable(this.deps.store.writeData(id, request.payload.key, request.payload.value, mayCommit), signal)
          else if (request.method === 'storage.delete') await abortable(this.deps.store.deleteData(id, request.payload.key, mayCommit), signal)
          else if (request.method === 'storage.files.list') result = await abortable(this.deps.store.listFiles(id, request.payload.prefix), signal)
          else if (request.method === 'storage.files.read') result = await abortable(this.deps.store.readFile(id, request.payload.path), signal)
          else if (request.method === 'storage.files.write') await abortable(this.deps.store.writeFile(id, request.payload.path, request.payload.data, mayCommit), signal)
          else if (request.method === 'storage.files.delete') await abortable(this.deps.store.deleteFile(id, request.payload.path, mayCommit), signal)
          else if (request.method === 'storage.backup.export') result = await abortable(this.deps.store.exportBackup(id), signal)
          else if (request.method === 'storage.backup.import') await abortable(this.deps.store.importBackup(id, request.payload.backup, mayCommit), signal)
          this.check(signal, cap); validateCapabilityResult(request.method, result)
          if (result === null) return null
          return result as JsonValue
        }
        case 'config.get': case 'config.set': {
          const cap = request.method === 'config.get' ? 'plugin.config.read' : 'plugin.config.write'
          this.check(signal, cap); this.rate('config', 60)
          const snapshot = this.deps.authority.snapshot
          if (!isValidatedPluginPackageSnapshotV3(snapshot)) return fail('CONFIGURATION_NOT_DECLARED')
          const manifest = snapshot.manifest
          if (!manifest.configuration) return fail('CONFIGURATION_NOT_DECLARED')
          const mayCommit = () => !signal.aborted && this.deps.isRuntimeCurrent() && this.deps.authority.isCurrent(cap)
          let result: JsonValue = null
          if (request.method === 'config.get') result = await abortable(this.deps.store.configuration.get(manifest), signal)
          else await abortable(this.deps.store.configuration.set(manifest, request.payload.key, request.payload.value, mayCommit), signal)
          this.check(signal, cap); validateCapabilityResult(request.method, result); return result
        }
        case 'ai.complete': {
          this.check(signal, 'sharedAI.complete')
          if (this.aiBusy) fail('LIMIT_EXCEEDED')
          this.rate('ai', 5); this.aiBusy = true
          try {
            if (!await abortable(this.deps.effects.isAIConfigured(), signal)) fail('NOT_AVAILABLE')
            this.check(signal, 'sharedAI.complete')
            await this.consent('ai', request.payload.prompt, signal, 'sharedAI.complete')
            this.check(signal, 'sharedAI.complete')
            const response = await abortable(this.deps.effects.completeAI(request.payload.prompt, signal), signal)
            this.check(signal, 'sharedAI.complete')
            if (typeof response.outputTokens !== 'number' || !Number.isSafeInteger(response.outputTokens) || response.outputTokens < 0) return fail('NOT_AVAILABLE')
            if (response.outputTokens > 1024) fail('LIMIT_EXCEEDED')
            const result = { text: response.text }; validateCapabilityResult(request.method, result); return result
          } finally { this.aiBusy = false }
        }
        case 'external.open':
          this.check(signal, 'external.open')
          if (!this.deps.effects.externalOpen) return fail('NOT_AVAILABLE')
          if (this.deps.requireExternalConfirmation === false) {
            if (!this.deps.effects.hasForegroundGesture()) return fail('USER_CANCELLED')
          } else {
            await this.consent('external', request.payload.url, signal, 'external.open')
          }
          this.check(signal, 'external.open')
          await this.deps.effects.externalOpen(request.payload.url)
          this.check(signal, 'external.open')
          return null
        case 'clipboard.write':
          this.check(signal, 'clipboard.write'); this.rate('clipboard', 5)
          await this.consent('clipboard', request.payload.text, signal, 'clipboard.write')
          this.check(signal, 'clipboard.write'); await abortable(Promise.resolve(this.deps.effects.clipboardWrite(request.payload.text)), signal); return null
      }
      return fail('NOT_AVAILABLE')
    } catch (error) {
      if (error instanceof BridgeWireError) throw error
      this.check(signal)
      const code = error && typeof error === 'object' && 'code' in error ? error.code : undefined
      if (code === 'CONFIGURATION_NOT_DECLARED' || code === 'CONFIGURATION_KEY_NOT_FOUND' || code === 'CONFIG_INCOMPATIBLE') return fail(code)
      if (code === 'SCREEN_CAPTURE_UNAVAILABLE') return fail('NOT_AVAILABLE')
      return fail(code === 'STORAGE_LIMIT' ? 'LIMIT_EXCEEDED' : code === 'INVALID_INPUT' ? 'INVALID_REQUEST' : 'INTERNAL_ERROR')
    }
  }
}
