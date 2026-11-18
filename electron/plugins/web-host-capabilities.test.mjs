import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { PluginManager } from './plugin-manager.ts'
import { runtimeV3Manifest, runtimeV3Package } from './fixtures.mjs'
import { WebHostCapabilities } from './web-host-capabilities.ts'

const caps = ['plugin.storage.read', 'plugin.storage.write', 'sharedAI.complete', 'clipboard.write']

test('workflow capabilities enforce grants, gestures and dispose their browser lifetime', async t => {
  let lifetime
  const effects = { browserOpen: async (_pluginId, _url, signal) => { lifetime = signal; return { id: 'owned-browser' } } }
  const env = await setup(t, effects, ['system.browser.automation'])
  assert.deepEqual(await env.call('system.browserOpen', { url: 'https://example.com' }), { id: 'owned-browser' })
  assert.equal(lifetime.aborted, false)
  await env.manager.setEnabled(env.m.id, false, env.manager.session)
  assert.equal(lifetime.aborted, true)
  const denied = await setup(t, effects, [])
  await assert.rejects(() => denied.call('system.browserOpen', { url: 'https://example.com' }), e => e.code === 'PERMISSION_DENIED')
  const background = await setup(t, { ...effects, hasForegroundGesture: () => false }, ['system.browser.automation'])
  await assert.rejects(() => background.call('system.browserOpen', { url: 'https://example.com' }), e => e.code === 'USER_CANCELLED')
  const imageDenied = await setup(t, { imageTransform: () => assert.fail('must not decode') }, [])
  await assert.rejects(() => imageDenied.call('system.imageTransform', { dataUrl: 'data:image/png;base64,AAAA', width: 10, format: 'png', quality: 85 }), e => e.code === 'PERMISSION_DENIED')
})
test('scheduled tasks bind host identity and require explicit confirmation before persistence', async t => {
  const calls = []
  const env = await setup(t, { scheduledTasks: async (...args) => { calls.push(args); return { ...args[3], pluginId: args[1] } } }, ['plugin.tasks'])
  const request = { code: 'reminder', description: 'Run reminder', schedule: { kind: 'once', atMs: Date.now() + 30000 } }
  assert.equal((await env.call('tasks.request', request)).code, 'reminder')
  assert.equal(env.prompts[0].kind, 'schedule'); assert.match(env.prompts[0].preview, /reminder/)
  assert.equal(calls[0][1], env.m.id); assert.equal(calls[0][2], env.m.features[0].id)
  const cancelled = await setup(t, { confirm: async () => false, scheduledTasks: async () => { assert.fail('must not persist rejected task') } }, ['plugin.tasks'])
  await assert.rejects(() => cancelled.call('tasks.request', request), e => e.code === 'USER_CANCELLED')
  const background = await setup(t, { hasForegroundGesture: () => false, scheduledTasks: async () => null }, ['plugin.tasks'])
  await assert.rejects(() => background.call('tasks.request', request), e => e.code === 'USER_CANCELLED')
  await env.manager.setEnabled(env.m.id, false, env.manager.session)
  await assert.rejects(() => env.call('tasks.list', null), e => ['STALE_RUNTIME', 'CANCELLED'].includes(e.code))
})
test('paste sends text to the original application only with permission and a foreground user gesture', async t => {
  const sent = []
  const env = await setup(t, { pasteText: async text => sent.push(text) }, ['system.input.paste'])
  assert.equal(await env.call('system.pasteText', { text: 'Hello 中文' }), null)
  assert.deepEqual(sent, ['Hello 中文'])
  const denied = await setup(t, { pasteText: async () => {} }, [])
  await assert.rejects(() => denied.call('system.pasteText', { text: 'x' }), e => e.code === 'PERMISSION_DENIED')
  const background = await setup(t, { pasteText: async () => {}, hasForegroundGesture: () => false }, ['system.input.paste'])
  await assert.rejects(() => background.call('system.pasteText', { text: 'x' }), e => e.code === 'USER_CANCELLED')
})
test('system notifications require the declared grant and deliver title and body', async t => {
  const notifications = []
  const env = await setup(t, { notify: async (title, body) => notifications.push({ title, body }) }, ['system.notification'])
  assert.equal(await env.call('system.notify', { title: '验收通知', body: '插件发送的消息' }), null)
  assert.deepEqual(notifications, [{ title: '验收通知', body: '插件发送的消息' }])
  const denied = await setup(t, { notify: async () => {} }, [])
  await assert.rejects(() => denied.call('system.notify', { title: '测试', body: '' }), e => e.code === 'PERMISSION_DENIED')
})
test('screen information requires its grant; screen capture also requires a foreground gesture', async t => {
  const display = { id: 'display-1', label: '主显示器', bounds: { x: 0, y: 0, width: 1920, height: 1080 }, workArea: { x: 0, y: 0, width: 1920, height: 1040 }, scaleFactor: 1, rotation: 0, primary: true }
  const captures = []
  const env = await setup(t, { getDisplays: async () => [display], captureScreen: async id => { captures.push(id); return 'data:image/png;base64,iVBORw0KGgo=' } }, ['system.screen.capture'])
  assert.deepEqual(await env.call('system.getDisplays', null), [display])
  assert.equal(await env.call('system.captureScreen', { displayId: 'display-1' }), 'data:image/png;base64,iVBORw0KGgo=')
  assert.deepEqual(captures, ['display-1'])
  const denied = await setup(t, { getDisplays: async () => [display] }, [])
  await assert.rejects(() => denied.call('system.getDisplays', null), e => e.code === 'PERMISSION_DENIED')
  const background = await setup(t, { captureScreen: async () => 'data:image/png;base64,iVBORw0KGgo=', hasForegroundGesture: () => false }, ['system.screen.capture'])
  await assert.rejects(() => background.call('system.captureScreen', {}), e => e.code === 'USER_CANCELLED')
})
test('clipboard image and file operations exchange real payloads with the host', async t => {
  const png = 'data:image/png;base64,iVBORw0KGgo='
  const writes = []
  const env = await setup(t, { clipboardReadImage: async () => png, clipboardReadFiles: async () => ['D:\\a.txt', 'D:\\文件夹'],
    clipboardWriteImage: async data => writes.push(data), clipboardWriteFiles: async paths => writes.push(paths) }, ['clipboard.current.read', 'clipboard.write'])
  assert.equal(await env.call('clipboard.readImage', null), png)
  assert.deepEqual(await env.call('clipboard.readFiles', null), ['D:\\a.txt', 'D:\\文件夹'])
  assert.equal(await env.call('clipboard.writeImage', { dataUrl: png }), null)
  assert.equal(await env.call('clipboard.writeFiles', { paths: ['D:\\a.txt', 'D:\\文件夹'] }), null)
  assert.deepEqual(writes, [png, ['D:\\a.txt', 'D:\\文件夹']])
  await assert.rejects(() => env.call('clipboard.writeFiles', { paths: ['https://example.com'] }), e => e.code === 'INVALID_REQUEST')
})
test('clipboard text read requires permission and a user gesture', async t => {
  const env = await setup(t, { clipboardReadText: async () => '中文 clipboard' }, ['clipboard.current.read'])
  assert.equal(await env.call('clipboard.readText', null), '中文 clipboard')
  const denied = await setup(t, { clipboardReadText: () => 'secret' }, [])
  await assert.rejects(() => denied.call('clipboard.readText', null), e => e.code === 'PERMISSION_DENIED')
  const background = await setup(t, { clipboardReadText: () => 'secret', hasForegroundGesture: () => false }, ['clipboard.current.read'])
  await assert.rejects(() => background.call('clipboard.readText', null), e => e.code === 'USER_CANCELLED')
})
test('desktop file selection and opening use the plugin capability and user gesture', async t => {
  const opened = []
  const env = await setup(t, { choosePaths: async kind => kind === 'file' ? ['D:\\example.txt'] : ['D:\\example'], openPath: async path => { opened.push(path); return '' } }, ['system.file.select', 'system.path.open'])
  assert.deepEqual(await env.call('files.select', { kind: 'file', multiple: false }), ['D:\\example.txt'])
  assert.deepEqual(await env.call('files.select', { kind: 'directory', multiple: false }), ['D:\\example'])
  assert.equal(await env.call('files.openPath', { path: 'D:\\example.txt' }), null)
  assert.deepEqual(opened, ['D:\\example.txt'])
  await assert.rejects(() => env.call('files.openPath', { path: 'https://example.com' }), e => e.code === 'INVALID_REQUEST')
  const denied = await setup(t, { choosePaths: async () => [] }, [])
  await assert.rejects(() => denied.call('files.select', { kind: 'file', multiple: false }), e => e.code === 'PERMISSION_DENIED')
  const cancelled = await setup(t, { choosePaths: async () => [] }, ['system.file.select'])
  assert.deepEqual(await cancelled.call('files.select', { kind: 'file', multiple: false }), [])
  const background = await setup(t, { choosePaths: async () => ['D:\\example.txt'], hasForegroundGesture: () => false }, ['system.file.select'])
  await assert.rejects(() => background.call('files.select', { kind: 'file', multiple: false }), e => e.code === 'USER_CANCELLED')
})
async function setup(t, overrides = {}, capabilities = caps) {
  const root = await mkdtemp(join(tmpdir(), 'webtools-capability-')); t.after(() => rm(root, { recursive: true, force: true }))
  const base = runtimeV3Manifest(); const m = runtimeV3Manifest({ requestedCapabilities: capabilities,
    features: [{ ...base.features[0], requiredCapabilities: capabilities, acceptedContexts: [{ type: 'text', sources: ['launcher-input'] }] }] })
  const manager = new PluginManager({ userData: root, hostVersion: '0.1.0', confirm: async () => true,
    externalOpen: async () => {}, clipboardWrite: () => {}, ai: { complete: async () => ({ text: 'unused' }), getDefaultProviderInfo: async () => ({ providerName: 'test', model: 'test' }) } })
  await manager.initialize(); t.after(() => manager.close())
  await manager.install(runtimeV3Package(m), manager.session); await manager.setEnabled(m.id, true, manager.session)
  let authority; const ticket = await manager.authorizeWebFeature({ pluginId: m.id, featureId: m.features[0].id }, manager.session, async a => { authority = a }); await ticket.completion
  const writes = []; const prompts = []; let now = 0; let aiCalls = 0
  const effects = { confirm: async request => { prompts.push(request); return true }, hasForegroundGesture: () => true,
    isAIConfigured: async () => true, completeAI: async (_prompt, signal) => { aiCalls++; return { text: signal.aborted ? 'late' : 'answer', outputTokens: 1, providerId: 'NEVER EXPOSE' } },
    clipboardWrite: text => writes.push(text), ...overrides }
  const service = new WebHostCapabilities({ authority, store: manager.store, effects, isRuntimeCurrent: () => true,
    context: { type: 'text', source: 'launcher-input', text: ' 中文 exact 😀 ', ageMs: 0 }, now: () => now })
  t.after(() => service.dispose())
  const call = (method, payload, signal = new AbortController().signal) => service.dispatch({ method, payload }, signal)
  return { manager, authority, m, service, writes, prompts, call, setNow: n => { now = n }, aiCalls: () => aiCalls }
}
test('external browser action requires declared grant and one-use explicit activation, and rejects unsafe URLs', async t => {
  const opened = []; let gesture = true
  const env = await setup(t, { externalOpen: async url => opened.push(url), hasForegroundGesture: () => { const value = gesture; gesture = false; return value } }, ['external.open'])
  for (const url of ['file:///D:/x', 'javascript:alert(1)', 'https://user:password@example.com', 'https://example.com/\npath'])
    await assert.rejects(() => env.call('external.open', { url }), error => error.code === 'INVALID_REQUEST')
  assert.equal(opened.length, 0)
  assert.equal(await env.call('external.open', { url: 'https://www.bing.com/search?q=selected%20text' }), null)
  assert.deepEqual(opened, ['https://www.bing.com/search?q=selected%20text'])
  assert.equal(env.prompts.length, 1)
  assert.equal(env.prompts[0].kind, 'external')
  assert.match(env.prompts[0].preview, /bing\.com/)
  await assert.rejects(() => env.call('external.open', { url: 'https://example.com' }), error => error.code === 'USER_CANCELLED')
  const denied = await setup(t, { externalOpen: async url => opened.push(url) })
  await assert.rejects(() => denied.call('external.open', { url: 'https://example.com' }), error => error.code === 'PERMISSION_DENIED')
  env.authority.revoke()
  await assert.rejects(() => env.call('external.open', { url: 'https://example.com' }))
  assert.equal(opened.length, 1)
})
test('context exposes only immutable invocation text and expires; revoked context cannot be retrieved', async t => {
  const env = await setup(t)
  assert.deepEqual(await env.call('context.current', null), { type: 'text', source: 'launcher-input', text: ' 中文 exact 😀 ' })
  env.setNow(600000); await assert.rejects(() => env.call('context.current', null), e => e.code === 'CONTEXT_EXPIRED')
  env.authority.revoke(); await assert.rejects(() => env.call('context.current', null))
})
test('undelivered context expires without revoking the running feature', async t => {
  const env = await setup(t); env.setNow(30000)
  await assert.rejects(() => env.call('context.current', null), e => e.code === 'CONTEXT_EXPIRED')
  assert.equal(env.authority.isCurrent(), true)
  assert.equal(await env.call('clipboard.write', { text: 'still usable' }), null)
})
test('AI byte excess and raw provider failures are rejected without truncation or secret exposure', async t => {
  const oversized = await setup(t, { completeAI: async () => ({ text: 'x'.repeat(32769), outputTokens: 1 }) })
  await assert.rejects(() => oversized.call('ai.complete', { prompt: 'safe' }), e => e.code === 'LIMIT_EXCEEDED')
  const failure = await setup(t, { completeAI: async () => { throw new Error('TOKEN SECRET PROVIDER ENDPOINT') } })
  await assert.rejects(() => failure.call('ai.complete', { prompt: 'safe' }), e => e.code === 'INTERNAL_ERROR' && !e.message.includes('SECRET'))
  await assert.rejects(() => failure.call('ai.complete', { prompt: 'x'.repeat(32769) }), e => e.code === 'LIMIT_EXCEEDED')
})
test('AI output rejects token overflow even with short text, and missing token usage fails closed', async t => {
  const overflow = await setup(t, { completeAI: async () => ({ text: 'short', outputTokens: 1025 }) })
  await assert.rejects(() => overflow.call('ai.complete', { prompt: 'safe' }), e => e.code === 'LIMIT_EXCEEDED')
  const missing = await setup(t, { completeAI: async () => ({ text: 'short' }) })
  await assert.rejects(() => missing.call('ai.complete', { prompt: 'safe' }), e => e.code === 'NOT_AVAILABLE')
  const exact = await setup(t, { completeAI: async () => ({ text: 'short', outputTokens: 1024 }) })
  assert.deepEqual(await exact.call('ai.complete', { prompt: 'safe' }), { text: 'short' })
})
test('storage lists only matching keys in its own namespace and requires read permission', async t => {
  const env = await setup(t)
  await env.call('storage.set', { key: 'note-b', value: 'b' })
  await env.call('storage.set', { key: 'note-a', value: 'a' })
  await env.call('storage.set', { key: 'other', value: 'c' })
  assert.deepEqual(await env.call('storage.list', { prefix: 'note-' }), ['note-a', 'note-b'])
  assert.deepEqual(await env.call('storage.list', { prefix: 'missing' }), [])
  await assert.rejects(() => env.call('storage.list', { prefix: '../' }))
  const denied = await setup(t, {}, ['plugin.storage.write'])
  await assert.rejects(() => denied.call('storage.list', { prefix: '' }), e => e.code === 'PERMISSION_DENIED')
})
test('storage uses Main namespace, read/write/delete and rolling quota; no caller identity/path survives schema', async t => {
  const env = await setup(t)
  await env.call('storage.set', { key: 'note', value: { saved: true } })
  assert.deepEqual(await env.call('storage.get', { key: 'note' }), { saved: true })
  await env.call('storage.delete', { key: 'note' }); assert.equal(await env.call('storage.get', { key: 'note' }), null)
  await assert.rejects(() => env.call('storage.get', { key: 'note', pluginId: 'forged' }))
  for (let i = 4; i < 60; i++) await env.call('storage.get', { key: 'note' })
  await assert.rejects(() => env.call('storage.get', { key: 'note' }), e => e.code === 'LIMIT_EXCEEDED')
  env.setNow(60000); assert.equal(await env.call('storage.get', { key: 'note' }), null)
})
test('AI requires foreground intent plus confirmation on every call; returns text only, caps rates and safe errors', async t => {
  const env = await setup(t)
  for (let i = 0; i < 5; i++) assert.deepEqual(await env.call('ai.complete', { prompt: 'hello' }), { text: 'answer' })
  assert.equal(env.prompts.length, 5); assert.equal(env.aiCalls(), 5)
  await assert.rejects(() => env.call('ai.complete', { prompt: 'hello' }), e => e.code === 'LIMIT_EXCEEDED')
  const background = await setup(t, { hasForegroundGesture: () => false })
  await assert.rejects(() => background.call('ai.complete', { prompt: 'hello' }), e => e.code === 'USER_CANCELLED')
  assert.equal(background.prompts.length, 0); assert.equal(background.aiCalls(), 0)
  const unavailable = await setup(t, { isAIConfigured: async () => false })
  await assert.rejects(() => unavailable.call('ai.complete', { prompt: 'hello' }), e => e.code === 'NOT_AVAILABLE')
})
test('confirmation deny/cancel and revoke during asynchronous confirmation leave clipboard unchanged', async t => {
  const denied = await setup(t, { confirm: async () => false })
  await assert.rejects(() => denied.call('clipboard.write', { text: 'safe' }), e => e.code === 'USER_CANCELLED')
  assert.deepEqual(denied.writes, [])
  let resolve; const env = await setup(t, { confirm: () => new Promise(done => { resolve = done }) })
  const pending = env.call('clipboard.write', { text: 'safe' }); pending.catch(() => {})
  await new Promise(done => setImmediate(done)); env.authority.revoke(); resolve(true)
  await assert.rejects(pending); assert.deepEqual(env.writes, [])
})
test('capability confirmation receives the composed cancellation signal so Host modal and preview can close', async t => {
  let observed
  const env = await setup(t, { confirm: (_request, signal) => { observed = signal; return new Promise(() => {}) } })
  const controller = new AbortController(); const pending = env.call('clipboard.write', { text: 'safe' }, controller.signal); pending.catch(() => {})
  await new Promise(done => setImmediate(done))
  assert.ok(observed, 'Host confirmation must receive an AbortSignal')
  controller.abort(); await assert.rejects(pending, e => e.code === 'CANCELLED'); assert.equal(observed.aborted, true)
  assert.deepEqual(env.writes, [])
})
test('AI cancellation/revocation aborts pending local work; late response never returns and concurrent calls deny', async t => {
  let resolve; let observed
  const env = await setup(t, { completeAI: (_prompt, signal) => { observed = signal; return new Promise(done => { resolve = done }) } })
  const controller = new AbortController(); const pending = env.call('ai.complete', { prompt: 'hello' }, controller.signal); pending.catch(() => {})
  await new Promise(done => setImmediate(done))
  await assert.rejects(() => env.call('ai.complete', { prompt: 'second' }), e => e.code === 'LIMIT_EXCEEDED')
  controller.abort(); await assert.rejects(pending, e => e.code === 'CANCELLED'); assert.equal(observed.aborted, true)
  resolve({ text: 'late' })
})
test('clipboard successful bounded write, byte excess, consent quota, and storage revocation before commit', async t => {
  const env = await setup(t)
  await env.call('clipboard.write', { text: 'x'.repeat(65536) }); assert.equal(env.writes[0].length, 65536)
  await assert.rejects(() => env.call('clipboard.write', { text: 'x'.repeat(65537) }), e => e.code === 'LIMIT_EXCEEDED')
  for (let i = 1; i < 5; i++) await env.call('clipboard.write', { text: 'safe' })
  await assert.rejects(() => env.call('clipboard.write', { text: 'safe' }), e => e.code === 'LIMIT_EXCEEDED')
  const original = env.manager.store.fs.atomicJson.bind(env.manager.store.fs)
  env.manager.store.fs.atomicJson = async (...args) => { env.authority.revoke(); return original(...args) }
  await assert.rejects(() => env.call('storage.set', { key: 'note', value: 'stale' }))
  assert.equal(await env.manager.store.readData(env.m.id, 'note'), null)
})
