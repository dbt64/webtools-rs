import test from 'node:test'
import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { EventEmitter } from 'node:events'
import { stripTypeScriptTypes } from 'node:module'
import { runInNewContext } from 'node:vm'

const source = stripTypeScriptTypes((await readFile(new URL('./plugin-runtime-session.ts', import.meta.url), 'utf8'))
  .replace(/^import .*\r?\n/gm, '').replace(/^export /gm, ''), { mode: 'transform' })
function deferred() { let resolve; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }
async function setup() {
  const permission = deferred(); const save = deferred()
  const browserSession = Object.assign(new EventEmitter(), {
    setPermissionRequestHandler(fn) { this.permission = fn }, setPermissionCheckHandler() {},
    protocol: { handle() {} }, closeAllConnections: async () => {},
  })
  const Sessions = runInNewContext(`${source}\nPluginRuntimeSession`, {
    session: { fromPartition: () => browserSession },
    dialog: { showMessageBox: () => permission.promise, showSaveDialogSync: () => 'D:/example.txt' },
    setTimeout, clearTimeout,
  })
  const sessions = new Sessions(); const window = {}; const contents = { id: 1 }
  const release = await sessions.attach({ window, contents, host: 'example', snapshot: { packageHash: 'hash', manifest: { manifestVersion: 3, name: 'Example' } },
    current: () => true, foregroundGesture: () => true })
  let settled = 0; sessions.onHostDialogsClosed = owner => { assert.equal(owner, window); settled++ }
  await sessions.get()
  return { sessions, window, contents, release, browserSession, permission, save, get settled() { return settled } }
}

test('download sets its path synchronously while preserving other pending dialog ownership', async () => {
  const h = await setup()
  let allowed; let path
  h.browserSession.permission(h.contents, 'notifications', value => { allowed = value })
  h.browserSession.emit('will-download', {}, { getFilename: () => 'example.txt', cancel() {}, setSavePath(value) { path = value } }, h.contents)
  assert.equal(path, 'D:/example.txt')
  assert.equal(h.sessions.hasPendingHostDialogs(h.window), true)
  await new Promise(resolve => setImmediate(resolve))
  assert.equal(h.settled, 0)
  h.permission.resolve({ response: 1 }); await new Promise(resolve => setImmediate(resolve))
  assert.equal(allowed, true); assert.equal(h.settled, 1)
  assert.equal(h.sessions.hasPendingHostDialogs(h.window), false)
  await h.release()
})

test('Host consent rejection and owner teardown release dialog counts without affecting a later owner', async () => {
  const h = await setup()
  await assert.rejects(h.sessions.withHostDialog(h.window, async () => { throw new Error('cancelled') }), /cancelled/)
  assert.equal(h.settled, 1)
  const pending = deferred()
  const work = h.sessions.withHostDialog(h.window, () => pending.promise)
  assert.equal(h.sessions.hasPendingHostDialogs(h.window), true)
  await h.release()
  pending.resolve(); await work
  assert.equal(h.settled, 1)
  assert.equal(h.sessions.hasPendingHostDialogs(h.window), false)
})

test('Manager modal ownership is tracked without attaching Manager to the plugin browser partition', async () => {
  const h = await setup(); const manager = {}; const settled = []
  h.sessions.onHostDialogsClosed = window => settled.push(window)
  const detach = h.sessions.registerManagerWindow(manager)
  const first = deferred(); const second = deferred()
  const firstDialog = h.sessions.withHostDialog(manager, () => first.promise)
  const secondDialog = h.sessions.withHostDialog(manager, () => second.promise)
  assert.equal(h.sessions.hasPendingHostDialogs(manager), true)
  first.resolve(); await firstDialog
  assert.deepEqual(settled, [])
  second.resolve(); await secondDialog
  assert.deepEqual(settled, [manager])
  detach(); await h.release()
})
