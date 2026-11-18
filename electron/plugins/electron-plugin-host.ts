import { clipboard, ClipboardItem, desktopCapturer, dialog, nativeImage, Notification, screen, shell, type BrowserWindow } from 'electron'
import { open } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { extname } from 'node:path'
import type { DataStore } from '../services/data-store.ts'
import type { SharedAIService } from '../services/shared-ai-service.ts'
import { PluginManager } from './plugin-manager.ts'
import type { PluginConsent } from './permission-broker.ts'
import { formatPluginConsentDetail } from './plugin-consent-view.ts'
import { LIMITS } from './manifest.ts'
import { fail } from './errors.ts'
import type { ScreenDisplayInfo, WebCapabilityEffects } from './web-host-capabilities.ts'
import { completeSharedAIForWeb } from './web-host-capabilities.ts'
import { pluginRuntimeSession } from './plugin-runtime-session.ts'
import { transcodeMedia } from './media-transcode.ts'
import { createPluginWorkflowEffects } from './plugin-workflow-effects.ts'
import { readClipboardImage } from './clipboard-image.ts'

const consentText: Record<PluginConsent['kind'], string> = {
  'reset-config': '恢复此插件的默认设置？当前设置将保留为一份备份，并替换上一次设置备份。插件私有数据不会被删除。',
  install: '导入本地声明式插件？发布者未经验证。导入不会自动授予权限。',
  replace: '同一插件版本的内容不同，确认替换当前版本？原内容保留用于回滚。',
  downgrade: '确认降级插件？不执行插件迁移代码；已有私有数据保持不变。',
  grant: '授予以下宿主能力？插件仅能使用列出的声明式操作。',
  uninstall: '卸载此插件？将停止操作并删除该插件的受管程序文件。',
  'delete-data': '还要删除此插件的配置和私有数据吗？默认保留。',
  schedule: '允许此插件创建以下定时任务？关闭插件窗口后仍会由托盘宿主按计划执行。',
  external: '打开插件声明的 HTTPS 链接？', clipboard: '将以下内容写入剪贴板？', ai: '确认发送以下内容至当前共享 AI 服务？',
}
interface PluginRuntimeAcceptanceOverrides {
  confirm(request: PluginConsent): Promise<boolean>
  choosePackage(): Promise<Uint8Array | null>
  webEffects?: Partial<Omit<WebCapabilityEffects, 'hasForegroundGesture' | 'confirm'>>
}

export function createElectronPluginHost(deps: { userData: string; hostVersion: string; sharedAI: SharedAIService; dataStore: DataStore; getWindow(): BrowserWindow | null; nativeHostExecutable?: string; pasteText?(text: string): Promise<void>; deleteRuntimeData?(pluginId: string): Promise<void>; closeFeatureWindows?(pluginId: string): Promise<void>; clearTemporaryWindowTarget?(pluginId: string): void; getFeatureShortcut?(pluginId: string, featureId: string): string | null }, acceptance?: PluginRuntimeAcceptanceOverrides) {
  const window = () => { const w = deps.getWindow(); if (!w || w.isDestroyed() || w.webContents.isDestroyed()) fail('SESSION_EXPIRED'); return w }
  const confirmForWindow = async (getWindow: () => BrowserWindow | null, request: PluginConsent, signal?: AbortSignal, allowNoWindow = false): Promise<boolean> => {
    if (acceptance) return acceptance.confirm(request)
    const w = getWindow()
    if (w?.isDestroyed() || !w && !allowNoWindow) fail('SESSION_EXPIRED')
    // Native dialogs are the temporary 5C consent surface. Very large AI previews
    // stay unavailable until 5D can present the full request in a dedicated view.
    if ((request.preview?.length ?? 0) > 24_000) fail('USER_CONFIRMATION_REQUIRED')
    const deleting = request.kind === 'delete-data'
    const detail = formatPluginConsentDetail(request)
    const message = request.manifestVersion === 3 && request.runtimeKind === 'node' && ['install', 'replace', 'downgrade'].includes(request.kind)
      ? '安装或更新 Node 插件？该插件将以当前 Windows 用户身份运行本地代码。仅在信任来源和代码时继续。'
      : request.manifestVersion === 3 && request.kind === 'install'
      ? '导入本地 Web 插件？该插件包含未经验证发布者提供的网页代码。导入不会自动授予宿主能力。'
      : request.manifestVersion === 2 && request.kind === 'install'
      ? '导入本地 Web 插件？该插件包含未经验证发布者提供的可执行网页代码。导入不会自动授予宿主能力。'
      : (request.manifestVersion === 2 || request.manifestVersion === 3) && request.kind === 'grant' ? '授予以下 WebTools 宿主能力？插件仅能在声明的 Feature 中使用已授权的能力。' : consentText[request.kind]
    const options: Electron.MessageBoxOptions = { type: 'question', title: 'WebTools 插件', message, detail, buttons: deleting ? ['保留数据', '删除插件私有数据'] : ['取消', '确认'], defaultId: 0, cancelId: 0, noLink: true, signal }
    const result = await (w ? pluginRuntimeSession.withHostDialog(w, () => dialog.showMessageBox(w, options)) : dialog.showMessageBox(options))
    return !signal?.aborted && (!w || !w.isDestroyed() && getWindow() === w) && result.response === 1
  }
  const confirm = (request: PluginConsent, signal?: AbortSignal) => confirmForWindow(deps.getWindow, request, signal)
  const manager = new PluginManager({
    userData: deps.userData, hostVersion: deps.hostVersion, confirm,
    externalOpen: url => shell.openExternal(url), clipboardWrite: text => clipboard.writeText(text),
    pngDecoder: bytes => { const image = nativeImage.createFromBuffer(bytes); const size = image.getSize(); return !image.isEmpty() && size.width > 0 && size.height > 0 && size.width <= 256 && size.height <= 256 },
    deleteRuntimeData: deps.deleteRuntimeData,
    closeFeatureWindows: deps.closeFeatureWindows,
    clearTemporaryWindowTarget: deps.clearTemporaryWindowTarget,
    projectFeatureIcon: (snapshot, featureId) => {
      const features = snapshot.manifest.features
      const target = features.find(feature => feature.id === featureId)?.icon
      if (target?.kind === 'host') return undefined // Keep the declared Host token ahead of package asset fallback.
      const icon = target?.kind === 'asset' ? target : features.find(feature => feature.icon?.kind === 'asset')?.icon
      if (icon?.kind !== 'asset') return undefined
      const bytes = snapshot.getResource(icon.path)?.bytes
      if (!bytes) return undefined
      const image = nativeImage.createFromBuffer(bytes)
      if (image.isEmpty()) return undefined
      const png = image.resize({ width: 24, height: 24 }).toPNG()
      return png.length <= 4096 ? `data:image/png;base64,${png.toString('base64')}` : undefined
    },
    ai: {
      complete: (messages, signal, tokens) => deps.sharedAI.complete(messages, signal, tokens),
      getDefaultProviderInfo: async () => {
        const info = await deps.sharedAI.getDefaultProviderInfo()
        const settings = deps.dataStore.snapshot().settings.sharedAI
        const selected = settings.providers[settings.defaultProviderId]
        return { providerName: info.providerName, model: info.model, identity: JSON.stringify([settings.defaultProviderId, selected]) }
      },
    },
  })
  const choosePackage = async (): Promise<Uint8Array | null> => {
    if (acceptance) return acceptance.choosePackage()
    const w = window(); const result = await pluginRuntimeSession.withHostDialog(w, () => dialog.showOpenDialog(w, { title: '导入声明式插件', properties: ['openFile'], filters: [{ name: 'WebTools 插件', extensions: ['wtplugin'] }] }))
    if (w.isDestroyed() || deps.getWindow() !== w) fail('SESSION_EXPIRED')
    if (result.canceled || result.filePaths.length !== 1) return null
    const path = result.filePaths[0]; if (extname(path).toLowerCase() !== '.wtplugin') fail('INVALID_PACKAGE')
    const file = await open(path, 'r')
    try {
      const stat = await file.stat(); if (!stat.isFile() || !stat.size || stat.size > LIMITS.archive) fail('INVALID_PACKAGE')
      const chunks: Buffer[] = []; let total = 0
      for (;;) { const buffer = Buffer.alloc(Math.min(65536, LIMITS.archive + 1 - total)); const { bytesRead } = await file.read(buffer); if (!bytesRead) break; total += bytesRead; if (total > LIMITS.archive) fail('INVALID_PACKAGE'); chunks.push(buffer.subarray(0, bytesRead)) }
      return Buffer.concat(chunks)
    } finally { await file.close() }
  }
  const notifications = new Set<Electron.Notification>()
  const getDisplays = async (): Promise<ScreenDisplayInfo[]> => {
    const displays = screen.getAllDisplays()
    const primaryId = screen.getPrimaryDisplay().id
    return displays.map((display, index) => ({
      id: String(display.id),
      label: display.label || (display.id === primaryId ? '主显示器' : `显示器 ${index + 1}`),
      bounds: { x: display.bounds.x, y: display.bounds.y, width: display.bounds.width, height: display.bounds.height },
      workArea: { x: display.workArea.x, y: display.workArea.y, width: display.workArea.width, height: display.workArea.height },
      scaleFactor: display.scaleFactor,
      rotation: display.rotation as ScreenDisplayInfo['rotation'],
      primary: display.id === primaryId,
    }))
  }
  const captureScreen = async (displayId?: string): Promise<string> => {
    const displays = screen.getAllDisplays()
    const display = displayId === undefined
      ? screen.getPrimaryDisplay()
      : displays.find(candidate => String(candidate.id) === displayId)
    if (!display) throw Object.assign(new Error('Requested display is unavailable'), { code: 'INVALID_INPUT' })

    const pixelWidth = Math.max(1, Math.round(display.bounds.width * display.scaleFactor))
    const pixelHeight = Math.max(1, Math.round(display.bounds.height * display.scaleFactor))
    const scale = Math.min(1, 2560 / pixelWidth, 1600 / pixelHeight)
    const sources = await desktopCapturer.getSources({
      types: ['screen'],
      thumbnailSize: { width: Math.max(1, Math.round(pixelWidth * scale)), height: Math.max(1, Math.round(pixelHeight * scale)) },
      fetchWindowIcons: false,
    })
    const source = sources.find(candidate => candidate.display_id === String(display.id))
    if (!source || source.thumbnail.isEmpty()) throw Object.assign(new Error('Screen capture source unavailable'), { code: 'SCREEN_CAPTURE_UNAVAILABLE' })
    return source.thumbnail.toDataURL()
  }
  const webEffects: Omit<WebCapabilityEffects, 'hasForegroundGesture'> = {
    confirm, clipboardWrite: text => clipboard.writeText(text), clipboardReadText: () => clipboard.readText(), externalOpen: url => shell.openExternal(url),
    clipboardReadImage: () => readClipboardImage({ readItems: () => clipboard.read(), readFiles: async () => await clipboardFiles(deps.nativeHostExecutable, { operation: 'read' }) as string[], imageFromPath: path => { const image = nativeImage.createFromPath(path); return image.isEmpty() ? null : image.toDataURL() } }),
    clipboardWriteImage: async dataUrl => { const image = nativeImage.createFromDataURL(dataUrl); if (image.isEmpty()) throw new Error('invalid image'); const bytes = Uint8Array.from(image.toPNG()); await clipboard.write([new ClipboardItem({ 'image/png': new Blob([bytes], { type: 'image/png' }) })]) },
    clipboardReadFiles: async () => await clipboardFiles(deps.nativeHostExecutable, { operation: 'read' }) as string[],
    clipboardWriteFiles: async paths => { await clipboardFiles(deps.nativeHostExecutable, { operation: 'write', paths }) },
    openPath: path => shell.openPath(path),
    pasteText: deps.pasteText,
    notify: async (title, body) => { if (!Notification.isSupported()) throw new Error('Notifications unavailable'); const notification = new Notification({ title, body }); notifications.add(notification); notification.once('close', () => notifications.delete(notification)); notification.show() },
    getDisplays,
    captureScreen,
    ...createPluginWorkflowEffects(),
    getFeatureShortcut: deps.getFeatureShortcut,
    isAIConfigured: async () => (await deps.sharedAI.getDefaultProviderInfo()).configured,
    completeAI: (prompt, signal) => completeSharedAIForWeb(deps.sharedAI, prompt, signal),
    ...acceptance?.webEffects,
  }
  const mediaBusy = new WeakSet<BrowserWindow>()
  const webEffectsForWindow = (getWindow: () => BrowserWindow | null): Omit<WebCapabilityEffects, 'hasForegroundGesture'> => ({
    ...webEffects, confirm: (request, signal) => confirmForWindow(getWindow, request, signal),
    mediaTranscode: async (input, signal) => {
      const w = getWindow()
      if (!w || w.isDestroyed()) throw new Error('Window closed')
      if (mediaBusy.has(w)) throw new Error('Media conversion already running')
      mediaBusy.add(w)
      try {
        const result = await pluginRuntimeSession.withHostDialog(w, () => dialog.showSaveDialog(w, {
          title: '保存转换后的音视频', defaultPath: `converted.${input.format}`,
          filters: [{ name: input.format.toUpperCase(), extensions: [input.format] }],
        }))
        if (result.canceled || !result.filePath) return null
        if (signal.aborted || w.isDestroyed()) throw new Error('Window closed')
        return await transcodeMedia(input, result.filePath, signal)
      } finally { mediaBusy.delete(w) }
    },
    choosePaths: async (kind, multiple) => {
      const w = getWindow()
      if (!w || w.isDestroyed()) throw new Error('Window closed')
      const properties: Electron.OpenDialogOptions['properties'] = [kind === 'file' ? 'openFile' : 'openDirectory']
      if (multiple) properties.push('multiSelections')
      const result = await pluginRuntimeSession.withHostDialog(w, () => dialog.showOpenDialog(w, { properties }))
      return result.canceled ? [] : result.filePaths
    },
  })
  const commandEffects = { ...webEffects, confirm: (request: PluginConsent, signal?: AbortSignal) => confirmForWindow(() => null, request, signal, true) }
  return { manager, choosePackage, webEffects, webEffectsForWindow, commandEffects }
}

function clipboardFiles(executable: string | undefined, input: object): Promise<unknown> {
  if (!executable) return Promise.reject(new Error('NativeHost unavailable'))
  return new Promise((resolve, reject) => {
    const child = spawn(executable, ['--clipboard-files-helper'], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] })
    let output = ''; let error = ''
    child.stdout.setEncoding('utf8'); child.stderr.setEncoding('utf8')
    child.stdout.on('data', chunk => { output += chunk }); child.stderr.on('data', chunk => { error += chunk })
    child.on('error', reject)
    child.stdin.on('error', reject)
    child.on('close', code => { if (code !== 0) { reject(new Error(error || 'Clipboard helper failed')); return }; try { resolve(JSON.parse(output)) } catch (e) { reject(e) } })
    child.stdin.end(JSON.stringify(input))
  })
}
