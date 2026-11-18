import test from 'node:test'
import assert from 'node:assert/strict'
import { EventEmitter } from 'node:events'
import { readFile, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { stripTypeScriptTypes } from 'node:module'
import { runInNewContext } from 'node:vm'
import { PluginManager } from './plugin-manager.ts'
import { isWebFeatureAuthority } from './web-feature-authority.ts'
import { runtimeV3Manifest, runtimeV3Package } from './fixtures.mjs'
import { PluginThemeController } from './plugin-theme-controller.ts'
import { DEFAULT_HOST_HEIGHT, DEFAULT_HOST_WIDTH } from '../services/host-window-geometry.ts'

const source = stripTypeScriptTypes((await readFile(new URL('./plugin-window-manager.ts', import.meta.url), 'utf8'))
  .replace(/^import .*\r?\n/gm, '').replace('export class PluginWindowManager', 'class PluginWindowManager'), { mode: 'transform' })

function focusHarness({ deferRuntimeOpen = false, shellSetsOpacity = false } = {}) {
  const windows = []
  const handoffOrder = []
  let returned = 0
  let returnApproval = Promise.resolve(true)
  let pendingDialogs = false
  let failNextOpen = false
  let finishRuntimeOpen
  let runtimeOpenStarted = false
  const lifecycle = []
  const runtimeOpenOptions = []
  const runtimeBounds = []
  const runtimeVisibility = []
  class Window extends EventEmitter {
    constructor(options) { super(); this.options = options; this.visible = false; this.focused = false; this.minimized = false; this.destroyed = false; this.opacity = 1;
      this.webContents = Object.assign(new EventEmitter(), { setWindowOpenHandler() {}, isDestroyed() { return false }, send() {} }); windows.push(this) }
    isDestroyed() { return this.destroyed }
    isMinimized() { return this.minimized }
    isMaximized() { return false }
    isFocused() { return this.focused }
    isEnabled() { return this.enabled !== false }
    restore() { this.minimized = false }
    moveTop() {}
    show() { const changed = !this.visible; this.visible = true; if (changed) this.emit('show') }
    hide() { const changed = this.visible; this.visible = false; this.focused = false; if (changed) this.emit('hide') }
    focus() { if (this.refuseFocus) return; for (const w of windows) if (w !== this && w.focused) w.blur(); this.focused = true; this.emit('focus') }
    blur() { this.focused = false; this.emit('blur') }
    minimize() { this.blur(); this.minimized = true; this.emit('minimize') }
    setTitle(value) { this.title = value } setIcon() {} setBackgroundColor() {} setMenu() {}
    setSkipTaskbar(value) { this.skipTaskbar = value } setAlwaysOnTop(value) { this.alwaysOnTop = value }
    setOpacity(value) { this.opacity = value }
    getBounds() { return this.bounds ?? { x: 20, y: 20, width: this.options.width ?? 960, height: this.options.height ?? 680 } }
    setBounds(value) { this.bounds = value }
    getContentSize() { return [960, 680] }
    destroy() { handoffOrder.push('plugin-destroy'); this.destroyed = true; this.visible = false; this.emit('closed') }
  }
  const Windows = runInNewContext(`${source}\nPluginWindowManager`, { BrowserWindow: Window, DEFAULT_HOST_HEIGHT, DEFAULT_HOST_WIDTH,
    randomUUID: () => 'business-window-1', BridgeWireError: class extends Error {},
    nativeImage: { createFromPath: () => ({ isEmpty: () => true }), createFromBuffer: () => ({ isEmpty: () => true }) },
    PLUGIN_WINDOW_CHROME_CHANNELS: { state: 'state' },
    isWebFeatureAuthority: () => true,
    screen: { getCursorScreenPoint: () => ({}), getDisplayNearestPoint: () => ({ workArea: { x: 0, y: 0, width: 1440, height: 900 } }) }, Date, console, setImmediate })
  const owner = new Windows({ theme: { current: { backgroundColor: '#fff' }, subscribe: () => () => {} }, defaultIcon: 'app.ico', onSurfacesChanged() {},
    loadChrome: async window => { if (shellSetsOpacity) window.setOpacity(0) },
    hasPendingHostDialogs: () => pendingDialogs, returnToLauncher: () => { returned++; handoffOrder.push('launcher-shown'); return returnApproval },
    prepareLauncherFocus: async () => { handoffOrder.push('foreground-permission-granted') },
    focusLauncher: async () => { handoffOrder.push('launcher-focused') }, createRuntime: () => {
      const listeners = new Set()
      const failOpen = failNextOpen; failNextOpen = false
      return { status: { state: 'ABSENT' }, closes: 0, subscribe(fn) { listeners.add(fn); return () => listeners.delete(fn) },
        async openAuthority(_authority, _context, options) {
          runtimeOpenOptions.push(options)
          if (failOpen) throw new Error('Activation failed')
          runtimeOpenStarted = true
          if (deferRuntimeOpen) {
            this.status = { state: 'LOADING' }; for (const fn of listeners) fn(this.status)
            await new Promise(resolve => { finishRuntimeOpen = resolve })
          }
          this.status = { state: 'READY' }; for (const fn of listeners) fn(this.status)
          return this.status
        },
        async close() { this.closes++; await Promise.resolve(); this.status = { state: 'CLOSED' }; for (const fn of listeners) fn(this.status) },
        setBounds(bounds) { runtimeBounds.push(bounds) }, setVisible(visible) { runtimeVisibility.push(visible) },
        focus() {}, authorityForWindow() { return { isCurrent: () => true, snapshot: { inventory: [{ canonicalPath: 'web/child.html', mimeType: 'text/html' }] } } }, publishWindowClosed() {}, publishWindowMessage() {}, updateContext(value) { lifecycle.push(['context', value]) }, notifyEnter() { lifecycle.push(['enter']) }, notifyVisibility(value) { lifecycle.push([value ? 'show' : 'hide']) } }
    } })
  const open = (blurBehavior = 'temporary-hide', sessionId, featureId = 'home', context) => owner.open({ snapshot: {
    packageHash: 'hash', manifest: { manifestVersion: 3, id: 'org.example.focus', name: 'Focus', features: [{ id: featureId, title: featureId, presentation: { type: 'window', blurBehavior } }] } },
    featureId, ...(sessionId ? { development: { sessionId } } : {}), isCurrent: () => true, revoke() {} }, context)
  const createManager = () => { const window = new Window({}); owner.registerManagerWindow(window, async () => window.destroy()); window.show(); window.focus(); return window }
  return { owner, windows, open, createManager, lifecycle, runtimeOpenOptions, finishRuntimeOpen() { finishRuntimeOpen?.() }, runtimeBounds, runtimeVisibility, handoffOrder,
    get runtimeOpenStarted() { return runtimeOpenStarted }, failNextOpen() { failNextOpen = true },
    setReturn(value) { returnApproval = value }, setDialogs(value) { pendingDialogs = value }, get returned() { return returned } }
}
const settleBlur = () => new Promise(resolve => setImmediate(resolve))

test('ordinary plugin uses the shared default host size', async () => {
  const h = focusHarness()
  await h.open()
  assert.equal(h.windows[0].options.width, DEFAULT_HOST_WIDTH)
  assert.equal(h.windows[0].options.height, DEFAULT_HOST_HEIGHT)
  await h.owner.closeAll()
})

test('plugin business windows share the feature runtime, can be managed, and close with their owner', async () => {
  const h = focusHarness()
  await h.open()
  const parent = h.windows[0]
  const created = await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.create', payload: { title: 'Preview', width: 640, height: 420 } })
  assert.equal(typeof created.id, 'string')
  const child = h.windows[1]
  assert.equal(child.options.parent, parent)
  assert.equal(child.options.title, 'Preview')
  assert.equal(child.options.width, 640)
  assert.equal(child.options.height, 420)
  assert.equal(child.options.skipTaskbar, true)
  assert.equal(child.options.frame, false)
  assert.equal(h.owner.getChromeState(child).customWindow, true)
  assert.equal(child.visible, true)

  await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.setBounds', payload: { id: created.id, x: 90, y: 70, width: 700, height: 460 } })
  assert.deepEqual({ ...child.bounds }, { x: 90, y: 70, width: 700, height: 460 })
  await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.setAlwaysOnTop', payload: { id: created.id, enabled: true } })
  assert.equal(child.alwaysOnTop, true)
  await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.hide', payload: { id: created.id } })
  assert.equal(child.visible, false)
  await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.show', payload: { id: created.id } })
  assert.equal(child.visible, true)

  await h.owner.closePlugin('org.example.focus')
  assert.equal(child.destroyed, true)
  assert.equal(parent.destroyed, true)
  assert.equal(h.owner.hasSurfaces, false)
})

test('business windows can load a packaged child page and select frameless or native chrome', async () => {
  for (const [chrome, expectedFrame] of [['frameless', false], ['native', true]]) {
    const h = focusHarness()
    await h.open()
    const parent = h.windows[0]
    await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.create', payload: { document: 'web/child.html', chrome } })
    assert.equal(h.windows[1].options.frame, expectedFrame)
    assert.equal(h.runtimeOpenOptions[1].document, 'web/child.html')
    assert.equal(h.owner.getChromeState(h.windows[1]), null)
    await h.owner.closeAll()
  }
  const h = focusHarness()
  await h.open()
  await assert.rejects(() => h.owner.dispatchPluginWindow(h.windows[0], 'main', { method: 'window.create', payload: { document: 'web/missing.html' } }))
  assert.equal(h.windows.length, 1)
  await h.owner.closeAll()
})

test('host title bar controls only the custom window and keeps the plugin main window alive', async () => {
  const h = focusHarness()
  await h.open()
  const parent = h.windows[0]
  await h.owner.dispatchPluginWindow(parent, 'main', { method: 'window.create', payload: { title: 'Details' } })
  const child = h.windows[1]
  assert.equal(await h.owner.performChromeAction(child, 'pin'), true)
  assert.equal(child.alwaysOnTop, true)
  assert.equal(h.owner.getChromeState(child).alwaysOnTop, true)
  assert.equal(await h.owner.performChromeAction(child, 'close'), true)
  assert.equal(child.destroyed, true)
  assert.equal(parent.destroyed, false)
  await h.owner.closeAll()
})

test('business window becomes visible after shell loading sets opacity to zero', async () => {
  const h = focusHarness({ shellSetsOpacity: true })
  await h.open()
  await h.owner.dispatchPluginWindow(h.windows[0], 'main', { method: 'window.create', payload: { title: 'Preview' } })
  assert.equal(h.windows[1].visible, true)
  assert.equal(h.windows[1].opacity, 1)
  await h.owner.closeAll()
})

test('plugin activation starts immediately but the window stays hidden until its runtime is ready', async () => {
  const h = focusHarness({ deferRuntimeOpen: true })
  const opening = h.open()
  await settleBlur()
  const window = h.windows[0]
  assert.equal(h.runtimeOpenStarted, true)
  assert.equal(window.visible, false, 'WPF owns the only loading surface')
  assert.equal(h.owner.getChromeState(window).loading, true)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false, 'summon must not reveal an incomplete target')
  h.finishRuntimeOpen()
  await opening
  assert.equal(h.owner.getChromeState(window).loading, false)
  assert.equal(window.visible, true)
  assert.equal(h.runtimeVisibility.at(-1), true)
})

test('Esc waits for WPF acknowledgement, closes a temporary runtime, and retains a background window', async () => {
  const h = focusHarness()
  await h.open(); const temporary = h.windows[0]
  h.setDialogs(true)
  assert.equal(h.owner.handleEscape(temporary), false)
  assert.equal(h.returned, 0)
  h.setDialogs(false)
  let acknowledge
  h.setReturn(new Promise(resolve => { acknowledge = resolve }))
  assert.equal(h.owner.handleEscape(temporary), true)
  assert.equal(temporary.destroyed, false, 'WPF must acknowledge before teardown')
  acknowledge(true); await settleBlur()
  assert.equal(temporary.destroyed, true)
  assert.equal(h.owner.hasSurfaces, false, 'Esc teardown must release the idle-quit surface lease')
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  await h.open('background', undefined, 'background')
  assert.equal(h.owner.handleEscape(h.windows[1]), true)
  await settleBlur()
  assert.equal(h.windows[1].visible, true)
  assert.equal(h.returned, 2)
  await h.owner.closeAll()
})

test('Esc returns foreground permission before destroying the temporary plugin window', async () => {
  const h = focusHarness()
  await h.open()
  const plugin = h.windows[0]
  h.handoffOrder.length = 0

  assert.equal(h.owner.handleEscape(plugin), true)
  await new Promise(resolve => setTimeout(resolve, 10))

  assert.deepEqual(h.handoffOrder, [
    'launcher-shown',
    'foreground-permission-granted',
    'plugin-destroy',
    'launcher-focused',
  ])
  await h.owner.closeAll()
})

test('manual minimize and modal blur preserve the window until final owner focus settles', async () => {
  const h = focusHarness()
  await h.open(); const window = h.windows[0]
  window.minimize(); await settleBlur()
  assert.equal(window.visible, true)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  assert.equal(window.minimized, false)
  h.setDialogs(true); window.blur(); await settleBlur()
  assert.equal(window.visible, true)
  h.setDialogs(false); window.focus(); h.owner.hostDialogsClosed(window); await settleBlur()
  assert.equal(window.visible, true)
  window.blur(); h.owner.hostDialogsClosed(window); await settleBlur()
  assert.equal(window.visible, false)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  await h.owner.closeAll()
})

test('native file chooser disables its parent without hiding the plugin; ordinary blur still hides', async () => {
  const h = focusHarness(); await h.open(); const window = h.windows[0]
  window.enabled = false; window.blur(); await settleBlur()
  assert.equal(window.visible, true)
  window.enabled = true; window.focus()
  window.blur(); await settleBlur()
  assert.equal(window.visible, false)
  await h.owner.closeAll()
})

test('development reload clears only that session target before closing its old runtime', async () => {
  const h = focusHarness()
  await h.open('background', 'dev'); const dev = h.windows[0]
  await h.open(); const installed = h.windows[1]
  const reload = h.owner.beginDevelopmentReload('dev')
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  assert.equal(installed.focused, true)
  await reload.ready
  await h.open('background', 'dev')
  await reload.finish()
  assert.equal(h.windows.length, 2)
  dev.blur(); await settleBlur()
  await h.open('temporary-hide', 'dev', 'temporary')
  const second = h.owner.beginDevelopmentReload('dev')
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  await second.ready; await second.finish()
  assert.equal(dev.destroyed, true)
  assert.equal(installed.destroyed, true, 'successful background activation explicitly ends the previous temporary interface')
  await h.owner.closeAll()
})

test('temporary blur retains the exact installed/development window and restores it on summon', async () => {
  const { owner, windows, open } = focusHarness()
  await open(); const original = windows[0]
  original.blur(); await settleBlur()
  assert.equal(original.visible, false)
  assert.equal(owner.hasSurfaces, true)
  assert.equal(owner.focusCurrentTemporaryWindow(), true)
  assert.equal(original.visible, true)
  await open('background', undefined, 'background'); const installed = windows[1]
  await open('temporary-hide', 'development-1'); const development = windows[2]
  await settleBlur()
  await owner.closePlugin('org.example.focus')
  assert.equal(installed.destroyed, true)
  development.blur(); await settleBlur()
  assert.equal(owner.focusCurrentTemporaryWindow(), true)
  assert.equal(development.focused, true)
  await owner.closeAll()
  assert.equal(owner.focusCurrentTemporaryWindow(), false)
})

test('background activation clears the temporary target and does not hide on blur', async () => {
  const { owner, windows, open } = focusHarness()
  await open(); await open('background', undefined, 'background')
  windows[1].blur(); await settleBlur()
  assert.equal(windows[0].destroyed, true)
  assert.equal(windows[1].visible, true)
  assert.equal(owner.focusCurrentTemporaryWindow(), false)
  await owner.closeAll()
})

test('summon reports a failed foreground activation so WPF can show search', async () => {
  const h = focusHarness()
  await h.open()
  h.windows[0].blur(); await settleBlur()
  h.windows[0].refuseFocus = true
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  h.windows[0].refuseFocus = false
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  await h.owner.closeAll()
})

test('explicit switching ends the previous temporary interface and Esc leaves no hidden runtime history', async () => {
  const h = focusHarness()
  await h.open('temporary-hide', undefined, 'a')
  await h.open('temporary-hide', undefined, 'b')
  await settleBlur()
  assert.equal(h.windows[0].destroyed, true)
  h.owner.handleEscape(h.windows[1])
  await settleBlur()
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  await h.open('temporary-hide', undefined, 'b')
  assert.equal(h.windows.length, 3)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  h.windows[2].emit('close', { preventDefault() {} })
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  await settleBlur()
  await h.open('temporary-hide', undefined, 'b')
  assert.equal(h.windows.length, 4)
  assert.equal(h.windows[1].destroyed, true)
  await h.owner.closeAll()
})

test('Manager shares the temporary target, modal blur, summon, and acknowledged Esc teardown', async () => {
  const h = focusHarness()
  const manager = h.createManager()
  manager.blur(); await settleBlur()
  assert.equal(manager.visible, false)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  manager.minimize(); await settleBlur()
  assert.equal(manager.visible, true)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  assert.equal(manager.minimized, false)
  h.setDialogs(true); manager.blur(); await settleBlur()
  assert.equal(manager.visible, true)
  h.setDialogs(false); manager.focus()
  h.owner.handleEscape(manager); await settleBlur()
  assert.equal(manager.destroyed, true)
  assert.equal(h.owner.hasSurfaces, false)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  await h.owner.closeAll()
})

test('failed replacement activation retains the previous temporary interface', async () => {
  const h = focusHarness()
  await h.open()
  h.failNextOpen()
  await assert.rejects(h.open('temporary-hide', undefined, 'failed'), /Activation failed/)
  assert.equal(h.windows[0].destroyed, false)
  assert.equal(h.windows[1].destroyed, true)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  await h.owner.closeAll()
})

test('failed development restoration closes only that session and preserves an installed temporary target', async () => {
  const h = focusHarness()
  await h.open('background', 'dev')
  await h.open()
  const reload = h.owner.beginDevelopmentReload('dev')
  await reload.ready; await reload.finish()
  assert.equal(h.windows[0].destroyed, true)
  assert.equal(h.windows[1].destroyed, false)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  await h.owner.closeAll()
})

test('switching Manager, ordinary plugin, and Translation-style background tears down only the previous temporary target', async () => {
  const h = focusHarness()
  const manager = h.createManager()
  await h.open(); await settleBlur()
  assert.equal(manager.destroyed, true)
  assert.equal(h.windows[1].options.frame, false)
  const nextManager = h.createManager(); await settleBlur()
  assert.equal(h.windows[1].destroyed, true)
  h.owner.activateBackgroundWindow(); await settleBlur()
  assert.equal(nextManager.destroyed, true)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), false)
  await h.owner.closeAll()
})

test('failed WPF return retains the current temporary interface', async () => {
  const h = focusHarness()
  await h.open(); h.setReturn(Promise.resolve(false))
  h.owner.handleEscape(h.windows[0]); await settleBlur()
  assert.equal(h.windows[0].destroyed, false)
  assert.equal(h.owner.focusCurrentTemporaryWindow(), true)
  await h.owner.closeAll()
})

test('Plugin Window follows theme without replacing the window, altering geometry or retaining observers after closeAll', async t => {
  const root = await mkdtemp(join(tmpdir(), 'webtools-themed-window-'))
  const manager = new PluginManager({ userData: root, hostVersion: '0.1.0', confirm: async () => true,
    externalOpen: async () => {}, clipboardWrite() {}, ai: { complete: async () => ({ text: 'unused' }), getDefaultProviderInfo: async () => ({ providerName: 'test', model: 'test' }) } })
  await manager.initialize()
  t.after(async () => { manager.close(); await rm(root, { recursive: true, force: true }) })
  await manager.install(runtimeV3Package(), manager.session)
  await manager.setEnabled(runtimeV3Manifest().id, true, manager.session)
  let preference = 'light'
  const theme = new PluginThemeController(() => preference, Object.assign(new EventEmitter(), { shouldUseDarkColors: false }))
  t.after(() => theme.dispose())
  const windows = []
  class Window extends EventEmitter {
    constructor(options) { super(); this.options = options; this.background = options.backgroundColor; this.destroyed = false; this.visible = false;
      this.webContents = { setWindowOpenHandler() {}, on() {}, isDestroyed() { return false }, send() {} }; windows.push(this) }
    isDestroyed() { return this.destroyed }
    isMinimized() { return false }
    isMaximized() { return false }
    isFocused() { return this.visible }
    moveTop() {}
    show() { this.visible = true }
    focus() {}
    setTitle() {} setMenu() {}
    setIcon() {}
    setBackgroundColor(value) { this.background = value }
    getContentSize() { return [this.options.width, this.options.height] }
    destroy() { this.destroyed = true; this.emit('closed') }
  }
  const Windows = runInNewContext(`${source}\nPluginWindowManager`, {
    BrowserWindow: Window, DEFAULT_HOST_HEIGHT, DEFAULT_HOST_WIDTH, nativeImage: { createFromPath: () => ({ isEmpty: () => true }), createFromBuffer: () => ({ isEmpty: () => true }) },
    PLUGIN_WINDOW_CHROME_CHANNELS: { state: 'state' }, isWebFeatureAuthority,
    screen: { getCursorScreenPoint: () => ({ x: 0, y: 0 }), getDisplayNearestPoint: () => ({ workArea: { x: 0, y: 0, width: 1440, height: 900 } }) },
    Date, console,
  })
  const owner = new Windows({ theme, defaultIcon: 'app.ico', onSurfacesChanged() {}, createRuntime: () => {
    const listeners = new Set()
    return { status: { state: 'ABSENT' }, authority: undefined,
      subscribe(listener) { listeners.add(listener); return () => listeners.delete(listener) },
      async openAuthority(authority) { this.authority = authority; this.status = { state: 'READY' }; for (const listener of listeners) listener(this.status) },
      async close() { this.authority?.revoke(); await Promise.resolve(); this.status = { state: 'CLOSED' }; for (const listener of listeners) listener(this.status) },
      setBounds() {}, setVisible() {}, focus() {}, updateContext() {}, notifyEnter() {}, notifyVisibility() {},
    }
  } })
  const open = async () => { const dispatch = await manager.authorizeWebFeature({ pluginId: runtimeV3Manifest().id, featureId: 'hello' }, manager.session, authority => owner.open(authority)); await dispatch.completion }
  await open()
  assert.equal(windows[0].options.backgroundColor, '#f3f5f8')
  assert.equal(windows[0].options.width, DEFAULT_HOST_WIDTH)
  assert.equal(windows[0].options.height, DEFAULT_HOST_HEIGHT)
  assert.equal(windows[0].visible, true)
  preference = 'dark'; theme.refresh()
  assert.equal(windows[0].background, '#0b0c0e')
  await open()
  assert.equal(windows.length, 1)
  assert.equal(owner.hasSurfaces, true)
  await owner.closeAll()
  assert.equal(owner.hasSurfaces, false)
  preference = 'light'; theme.refresh()
  assert.equal(windows[0].background, '#0b0c0e')
})


test('retained window receives refreshed context before enter, while summon only changes visibility', async () => {
  const h = focusHarness()
  await h.open()
  assert.deepEqual(h.lifecycle, [['enter'], ['show']])
  const window = h.windows[0]
  window.blur(); await settleBlur()
  assert.equal(h.lifecycle.at(-1)[0], 'hide')
  await h.owner.focusCurrentTemporaryWindowAndWait()
  assert.equal(h.lifecycle.at(-1)[0], 'show')
  assert.equal(h.lifecycle.filter(item => item[0] === 'enter').length, 1)
  window.blur(); await settleBlur()
  h.lifecycle.length = 0
  await h.open('temporary-hide', undefined, 'home', { type: 'text', source: 'selected-text', text: 'new selection', ageMs: 0 })
  assert.equal(h.lifecycle[0][0], 'context')
  assert.equal(h.lifecycle[0][1].text, 'new selection')
  assert.deepEqual(h.lifecycle.slice(1), [['enter'], ['show']])
  assert.equal(h.windows.length, 1)
  await h.owner.closeAll()
})
