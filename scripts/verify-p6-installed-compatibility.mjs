// Usage: node --experimental-transform-types scripts/verify-p6-installed-compatibility.mjs <delivery> <old-stage> <old-v3-wtplugin>
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { once } from 'node:events'
import { copyFile, mkdir, mkdtemp, readFile, realpath, writeFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { freePort, until } from './mcp-native-test-support.mjs'
import { buildFixtures, legacyId, modernId } from './p6-compatibility-fixtures.mjs'
import { createRuntimeOrigin } from '../electron/plugins/web-runtime-policy.ts'

const [out, oldStage, oldPlugin] = process.argv.slice(2)
assert.ok(out && oldStage && oldPlugin)
const delivery = await realpath(out)
const base = dirname(delivery)
const root = await mkdtemp(join(base, 'p6compat-'))
const profile = join(root, 'profile')
const fixtures = join(root, 'fixtures')
await mkdir(profile)
await mkdir(fixtures)
const packages = await buildFixtures(join(delivery, 'artifacts'), oldPlugin)
const slot = name => join(fixtures, 'plugin-runtime-' + name + '.wtplugin')
const controls = (name, deleteData = false) => writeFile(join(profile, 'plugin-runtime-controls.json'), JSON.stringify({ package: name, deleteData }))
await copyFile(packages.legacy, slot('fixture'))
await copyFile(packages.legacyUpdate, slot('update'))
await copyFile(packages.modern, slot('isolation'))
await controls('fixture')
const report = { passed: false, checks: [], builds: [], fixtureV3Sha256: createHash('sha256').update(await readFile(oldPlugin)).digest('hex') }
const mark = name => { report.checks.push(name); console.log(name) }
const timeout = (label, ms) => delay(ms, null, { ref: false }).then(() => { throw Error('Timed out: ' + label) })
const note = '宿主升级前保存的值'
const legacyNote = '旧版声明式便笺'
const legacyLabel = '旧标签'

async function openManager(stage, label) {
  const executable = await realpath(join(stage, 'Manager/WebTools.exe'))
  const asar = await readFile(join(dirname(executable), 'resources/app.asar'))
  const asarSha256 = createHash('sha256').update(asar).digest('hex')
  report.builds.push({ label, asarSha256 })
  const port = await freePort()
  const child = spawn(executable, ['--plugin-runtime-acceptance'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
    ...process.env, TEMP: base, TMP: base, WEBTOOLS_PLUGIN_RUNTIME_ACCEPTANCE: '1', WEBTOOLS_PLUGIN_RUNTIME_STANDALONE: '1',
    WEBTOOLS_PLUGIN_RUNTIME_TEST_ROOT: root, WEBTOOLS_PLUGIN_RUNTIME_TEST_PROFILE: profile, WEBTOOLS_PLUGIN_RUNTIME_TEST_DEBUG_PORT: String(port),
  } })
  const exit = once(child, 'exit')
  let log = ''
  child.stdout.on('data', bytes => { log += bytes })
  child.stderr.on('data', bytes => { log += bytes })
  const sockets = new Set()
  async function connect(url, name) {
    const target = await until(async () => {
      const response = await fetch('http://127.0.0.1:' + port + '/json/list', { signal: AbortSignal.timeout(1500) })
      return (await response.json()).find(row => url(row.url))
    }, name, 25000, () => child.exitCode !== null ? 'Manager exited: ' + log.slice(-2000) : undefined)
    const socket = new WebSocket(target.webSocketDebuggerUrl)
    sockets.add(socket)
    await Promise.race([once(socket, 'open'), timeout('connect ' + name, 5000)])
    let sequence = 0
    const send = (method, params = {}) => new Promise((resolve, reject) => {
      const id = ++sequence
      const timer = setTimeout(() => { socket.removeEventListener('message', receive); reject(Error('CDP timed out: ' + method)) }, 10000)
      const receive = event => {
        const response = JSON.parse(event.data)
        if (response.id !== id) return
        clearTimeout(timer)
        socket.removeEventListener('message', receive)
        if (response.error || response.result?.exceptionDetails) reject(Error(JSON.stringify(response.error ?? response.result.exceptionDetails)))
        else resolve(response.result)
      }
      socket.addEventListener('message', receive)
      socket.send(JSON.stringify({ id, method, params }))
    })
    return async expression => (await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true })).result?.value
  }
  const close = async () => {
    for (const socket of sockets) socket.close()
    if (child.exitCode === null && child.signalCode === null) child.kill()
    await exit
    await writeFile(join(root, label + '.log'), log)
  }
  try {
    const evaluate = await connect(url => url.endsWith('/out/renderer/index.html'), 'Manager')
    await until(() => evaluate('!!window.desktop?.plugins && document.readyState === "complete"'), 'Manager public API')
    const call = async (expression, allowFailure = false) => {
      const result = await evaluate('window.desktop.' + expression)
      if (allowFailure) return result
      assert.equal(result?.ok, true, JSON.stringify(result))
      return result.data
    }
    const list = () => call('plugins.list()')
    const legacy = async (actionId, input = null) => {
      const plugin = (await list()).find(plugin => plugin.id === legacyId)
      const result = await call('plugins.invoke(' + JSON.stringify({ pluginId: plugin.id, version: plugin.version, hash: plugin.hash, actionId, input }) + ')')
      assert.equal(result.status, 'success')
      return result.value
    }
    const openModern = async () => {
      const ref = { kind: 'web', id: modernId, featureId: 'home' }
      await call('pluginCatalog.open(' + JSON.stringify(ref) + ')')
      const url = createRuntimeOrigin(modernId).origin + '/web/index.html'
      const page = await connect(candidate => candidate === url, 'v3 runtime')
      await until(() => page("document.querySelector('#status')?.textContent === '宿主已连接'"), 'older SDK connected')
      return page
    }
    return { call, list, legacy, openModern, close, async finish() {
      await call('plugins.stopRuntime(' + JSON.stringify(modernId) + ')')
      await evaluate("void window.desktop.managerWindow.perform('end-run'); true")
      const [code] = await Promise.race([exit, timeout('graceful Manager exit', 15000)])
      assert.equal(code, 0, log.slice(-2000))
    } }
  } catch (error) { await close(); throw error }
}

const identity = plugins => plugins.map(({ id, version, hash, manifestVersion, runtimeKind, enabled, granted }) =>
  ({ id, version, hash, manifestVersion, runtimeKind, enabled, granted })).sort((a, b) => a.id.localeCompare(b.id))
const legacyPages = app => app.call('plugins.getPages(' + JSON.stringify(legacyId) + ')')
const modernValues = page => page("JSON.parse(document.querySelector('#result').textContent)")
const expected = config => ({ json: note, file: note, config, node: note, browser: note })
const setupPlugin = async (app, name, id) => {
  await controls(name)
  const installed = await app.call('plugins.installFromUserDialog()')
  assert.equal(installed.outcome, 'installed')
  assert.equal(installed.plugin.id, id)
  await app.call('plugins.setEnabled(' + JSON.stringify(id) + ',true)')
}

let app
try {
  app = await openManager(await realpath(oldStage), 'before')
  await setupPlugin(app, 'fixture', legacyId)
  await setupPlugin(app, 'isolation', modernId)
  const translation = (await app.call('pluginCatalog.list()')).entries.find(item => item.id === 'webtools.translation')
  assert.equal(translation?.kind, 'builtin')
  assert.equal(translation.entry.kind, 'builtin-window')
  await app.legacy('save-note', { value: legacyNote })
  await app.legacy('set-label', { value: legacyLabel })
  const originalIdentity = identity(await app.list())
  assert.deepEqual(originalIdentity.map(plugin => plugin.manifestVersion), [1, 3])
  await app.call("managerWindow.perform('toggle-detach')")
  const originalPage = await app.openModern()
  await originalPage("document.querySelector('#note').value = " + JSON.stringify(note) + "; document.querySelector('#save').click()")
  await until(() => originalPage("document.querySelector('#status').textContent === '操作完成'"), 'old host saves v3 data')
  assert.deepEqual(await modernValues(originalPage), expected({ label: note }))
  mark('Older packaged Manager installs v1/v3 and writes their data through production APIs')
  await app.finish()
  await app.close()

  app = await openManager(join(delivery, 'stage'), 'after')
  assert.notEqual(report.builds[0].asarSha256, report.builds[1].asarSha256, 'Use two different packaged Manager builds')
  assert.deepEqual(identity(await app.list()), originalIdentity)
  const reopenedTranslation = (await app.call('pluginCatalog.list()')).entries.find(item => item.id === 'webtools.translation')
  assert.equal(reopenedTranslation?.kind, 'builtin')
  assert.equal(await app.legacy('load-note'), legacyNote)
  assert.deepEqual((await legacyPages(app)).config, { label: legacyLabel })
  await app.call("managerWindow.perform('toggle-detach')")
  const current = await app.openModern()
  assert.deepEqual(await modernValues(current), expected({ label: note }))
  mark('New packaged Manager reads both original archives, enabled state, grants and all saved stores without reinstalling')
  mark('Built-in Translation remains a separate catalog entry in both builds')

  await controls('update')
  assert.equal((await app.call('plugins.installFromUserDialog()')).plugin.version, '1.1.0')
  assert.equal(await app.legacy('load-note'), legacyNote)
  assert.deepEqual((await legacyPages(app)).config, { label: legacyLabel, compact: false })
  mark('Same-ID v1 upgrade retains stored values and populates the new setting default')
  await copyFile(packages.modernUpdate, slot('update'))
  assert.equal((await app.call('plugins.installFromUserDialog()')).plugin.version, '1.1.0')
  const upgraded = await app.openModern()
  assert.deepEqual(await modernValues(upgraded), expected({ label: note, compact: false }))
  mark('Same-ID v3 upgrade retains private JSON, file, config, Node data and browser storage')

  await app.call('plugins.uninstall(' + JSON.stringify(modernId) + ')')
  await setupPlugin(app, 'update', modernId)
  assert.deepEqual(await modernValues(await app.openModern()), expected({ label: note, compact: false }))
  mark('Explicit keep-data uninstall plus same-ID v3 reinstall restores the five stores')
  await controls('update', true)
  await app.call('plugins.uninstall(' + JSON.stringify(modernId) + ')')
  await setupPlugin(app, 'update', modernId)
  assert.deepEqual(await modernValues(await app.openModern()), {
    json: null, file: null, config: { label: '初始标签', compact: false }, node: null, browser: null,
  })
  assert.equal(await app.legacy('load-note'), legacyNote)
  mark('Explicit delete clears all five v3 stores but preserves the separate v1 plugin')
  await app.finish()
  report.passed = true
} catch (error) {
  report.error = String(error.stack ?? error)
  throw error
} finally {
  await app?.close()
  await writeFile(join(delivery, 'compatibility-verification.json'), JSON.stringify(report, null, 2) + '\n')
}
