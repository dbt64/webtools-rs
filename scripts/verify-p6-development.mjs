// Real installed SDK watcher -> existing authenticated development pipe ->
// packaged Manager -> public Web SDK. Never touches the user's profile/project.
// node --experimental-transform-types scripts/verify-p6-development.mjs DELIVERY [EXTERNAL_PROJECT]
import assert from 'node:assert/strict'
import { spawn, spawnSync } from 'node:child_process'
import { createServer } from 'node:net'
import { cp, mkdir, readFile, writeFile, copyFile, realpath } from 'node:fs/promises'
import { join, relative, isAbsolute, dirname } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'

assert.ok(isAbsolute(process.argv[2] ?? ''), 'Supply an absolute delivery directory')
const delivery = await realpath(process.argv[2])
const executable = await realpath(join(delivery, 'stage/Manager/WebTools.exe'))
const source = await realpath(process.argv[3] ?? 'D:/SystemDefault/Desktop/webtoolstest/P6-Toolchain-20261116-r1/projects/vanilla-web')
const root = join(delivery, 'verification', 'dev-' + Date.now().toString(36))
const profile = join(root, 'profile'), author = join(root, 'author'), fixtureRoot = join(root, 'fixtures')
await mkdir(profile, { recursive: true }); await mkdir(fixtureRoot)
await cp(source, author, { recursive: true, dereference: true,
  filter: p => !['.webtools', '.git', 'dist'].includes(relative(source, p).split(/[\\/]/)[0]) && !p.endsWith('.wtplugin') })
const npm = spawnSync(process.execPath, [join(dirname(process.execPath), 'node_modules/npm/bin/npm-cli.js'), 'install', '--save-dev', '--ignore-scripts', join(delivery, 'artifacts/webtools-plugin-sdk-1.16.1.tgz')], { cwd: author, encoding: 'utf8', windowsHide: true, timeout: 90000 })
assert.equal(npm.status, 0, `Install delivered SDK: ${npm.stdout}\n${npm.stderr}`)
const manifestPath = join(author, 'manifest.json')
const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
manifest.id = 'org.webtools.acceptance.runtime'; manifest.name = 'P6 Development Acceptance'; manifest.version = '1.0.0'
manifest.requestedCapabilities = ['plugin.storage.read', 'plugin.storage.write']
manifest.features[0].requiredCapabilities = manifest.requestedCapabilities
manifest.features[0].acceptedContexts = [
  { type: 'none', sources: [] },
  { type: 'text', sources: ['launcher-input', 'selected-text'] },
  { type: 'files', sources: ['explorer-selection'], maxItems: 64 },
  { type: 'directories', sources: ['explorer-selection'], maxItems: 64 },
]
await writeFile(manifestPath, JSON.stringify(manifest, null, 2))
const sdkCli = join(author, 'node_modules/@webtools/plugin-sdk/bin/webtools-plugin.mjs')
await realpath(sdkCli)
const mainPath = join(author, 'web/src/main.ts')
const code = label => `import { connectWebTools } from '@webtools/plugin-sdk/web'
const host = connectWebTools()
document.body.innerHTML = '<h1>${label}</h1><pre id="context"></pre><p id="ready">connecting</p>'
const update = async () => { document.querySelector('#context')!.textContent = JSON.stringify(await host.context.current()) }
host.lifecycle.onEnter(update)
host.ready.then(async () => { await update(); document.querySelector('#ready')!.textContent = 'ready' })
`
await writeFile(mainPath, code('P6 initial'))
function cli(args) {
  const result = spawnSync(process.execPath, [sdkCli, ...args], { cwd: author, encoding: 'utf8', windowsHide: true, timeout: 90000, maxBuffer: 8 * 1024 * 1024 })
  assert.equal(result.status, 0, `${args.join(' ')}\n${result.stdout}\n${result.stderr}`)
  return result.stdout
}
cli(['build'])
cli(['pack', 'dist', '--out', join(fixtureRoot, 'plugin-runtime-fixture.wtplugin'), '--host-version', '0.1.0', '--project-root', author])
for (const kind of ['update', 'isolation']) await copyFile(join(fixtureRoot, 'plugin-runtime-fixture.wtplugin'), join(fixtureRoot, `plugin-runtime-${kind}.wtplugin`))
await writeFile(join(profile, 'plugin-runtime-controls.json'), JSON.stringify({ package: 'fixture', deleteData: false }))
const portProbe = createServer()
await new Promise(resolve => portProbe.listen(0, '127.0.0.1', resolve))
const port = portProbe.address().port
await new Promise(resolve => portProbe.close(resolve))
let dev, manager, managerCdp, pluginCdp, pluginTargetId, output = ''
const sockets = new Set()
const report = { passed: false, startedAt: new Date().toISOString(), root, source, executable,
  author, pluginId: manifest.id, sdkVersion: '1.16.1',
  consentSeam: 'Existing packaged acceptance seam only selects root/author and approves its fixed fixture identity. Development marker/token/build validation remains production code.', checks: [] }
const mark = text => { report.checks.push(text); console.log(text) }
async function until(fn, label, timeout = 40000) {
  const end = Date.now() + timeout
  let last
  while (Date.now() < end) {
    if (manager?.exitCode != null) throw Error(`Manager exited during ${label}: ${output.slice(-5000)}`)
    try { const value = await fn(); if (value) return value } catch (error) { last = error }
    await sleep(150)
  }
  throw Error(`Timeout ${label}: ${last ?? ''}\n${output.slice(-3000)}`)
}
function attachOutput(child, label) {
  child.stdout.on('data', b => { output += `[${label}] ${b}` }); child.stderr.on('data', b => { output += `[${label}] ${b}` })
}
async function startWatcher(previousSession) {
  dev = spawn(process.execPath, [sdkCli, 'dev', author], { cwd: author, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
  attachOutput(dev, 'dev')
  return until(async () => {
    if (dev.exitCode !== null) throw Error('Watcher exited: ' + output.slice(-3000))
    const marker = JSON.parse(await readFile(join(author, '.webtools/development.json'), 'utf8'))
    return marker.sessionId !== previousSession && marker
  }, 'new official SDK watcher marker')
}
async function targets() { return (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()) }
async function connect(target) {
  const ws = new WebSocket(target.webSocketDebuggerUrl); sockets.add(ws)
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }) })
  let sequence = 0
  const command = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence
    const timer = setTimeout(() => { ws.removeEventListener('message', receive); reject(Error('CDP ' + method + ' timed out')) }, 15000)
    const receive = event => { const m = JSON.parse(event.data); if (m.id !== id) return; clearTimeout(timer); ws.removeEventListener('message', receive); m.error ? reject(Error(JSON.stringify(m.error))) : resolve(m.result) }
    ws.addEventListener('message', receive); ws.send(JSON.stringify({ id, method, params }))
  })
  return { command, async evaluate(expression) {
    await command('Page.bringToFront')
    const r = await command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true })
    if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails))
    return r.result.value
  }, close() { ws.close(); sockets.delete(ws) } }
}
const api = expression => managerCdp.evaluate('window.desktop.pluginDevelopment.' + expression)
async function openWith(invocation, expectedLabel = 'P6 initial') {
  console.log('Opening simulation:', JSON.stringify(invocation))
  await managerCdp.evaluate(`window.__p6OpenResult = null; void window.desktop.pluginDevelopment.openFeature('home',${JSON.stringify(invocation)}).then(r => window.__p6OpenResult = r); true`)
  await inspectPage(expectedLabel, invocation.context)
  const result = await until(() => managerCdp.evaluate('window.__p6OpenResult'), 'openFeature completion')
  assert.equal(result.ok, true, JSON.stringify(result))
}
async function inspectPage(expectedLabel, expectedContext) {
  await until(async () => {
    const target = (await targets()).find(t => t.type === 'page' && t.url.includes('/web/index.html'))
    if (!target) return false
    if (target.id !== pluginTargetId) {
      pluginCdp?.close(); pluginCdp = await connect(target); pluginTargetId = target.id
    }
    return pluginCdp.evaluate(`document.querySelector('#ready')?.textContent === 'ready' && document.querySelector('h1')?.textContent === ${JSON.stringify(expectedLabel)}`)
  }, 'SDK page and expected revision')
  const received = await pluginCdp.evaluate(`document.querySelector('#context')?.textContent`)
  assert.deepEqual(JSON.parse(received), expectedContext)
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return
  await new Promise(resolve => { child.once('exit', resolve); child.kill() })
}
try {
  const marker = await startWatcher()
  manager = spawn(executable, ['--plugin-runtime-acceptance'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
    ...process.env, TEMP: delivery, TMP: delivery, WEBTOOLS_PLUGIN_RUNTIME_ACCEPTANCE: '1', WEBTOOLS_PLUGIN_RUNTIME_STANDALONE: '1',
    WEBTOOLS_PLUGIN_RUNTIME_TEST_ROOT: root, WEBTOOLS_PLUGIN_RUNTIME_TEST_PROFILE: profile, WEBTOOLS_PLUGIN_RUNTIME_TEST_DEBUG_PORT: String(port),
  } })
  attachOutput(manager, 'Manager')
  const target = await until(async () => (await targets()).find(t => t.type === 'page' && t.url.endsWith('/index.html')), 'packaged Manager')
  managerCdp = await connect(target)
  await until(() => managerCdp.evaluate('!!window.desktop?.pluginDevelopment'), 'Manager public preload')
  // Keep the developer controls alive using the same detachable Manager window
  // action exposed by its chrome; ordinary temporary Manager tabs close on switch.
  await managerCdp.evaluate("window.desktop.managerWindow.perform('toggle-detach')")
  assert.equal((await api('setEnabled(true)')).ok, true)
  assert.equal((await api('connectFromDialog()')).ok, true)
  assert.equal((await api('get()')).data.connected, true)
  mark('Official installed SDK dev CLI connects through the real authenticated marker/pipe and packaged Manager')
  assert.equal((await api(`openFeature('home', {surface:'launcher',context:{type:'text',source:'selected-text',text:'wrong surface'}})`)).error?.code, 'DEV_INVALID_CONTEXT')
  assert.equal((await api(`openFeature('missing-feature', {surface:'manager',context:null})`)).error?.code, 'DEV_CONTEXT_UNSUPPORTED')
  mark('Invalid simulated entry/context and an unavailable Feature return stable validation errors')
  const contexts = [
    { surface: 'manager', context: null },
    { surface: 'launcher', context: { type: 'text', source: 'launcher-input', text: 'Launcher 入口文字' } },
    { surface: 'super-panel', context: { type: 'text', source: 'selected-text', text: '面板选中文字' } },
    { surface: 'super-panel', context: { type: 'files', source: 'explorer-selection', items: [{ kind: 'file', path: join(root, 'fixture.txt') }] } },
    { surface: 'super-panel', context: { type: 'directories', source: 'explorer-selection', items: [{ kind: 'directory', path: author }] } },
  ]
  await writeFile(join(root, 'fixture.txt'), 'test context')
  for (const invocation of contexts) await openWith(invocation)
  mark('Manager, Launcher text, selected text, file and directory simulations arrive through the public SDK')
  const revision = (await api('get()')).data.project.revision
  await writeFile(mainPath, code('P6 reloaded'))
  await until(async () => (await api('get()')).data.project.revision > revision, 'successful watcher rebuild')
  await inspectPage('P6 reloaded', contexts.at(-1).context)
  mark('Changing source automatically reloads the active real page and preserves the last simulated directory context without another openFeature call')
  await writeFile(mainPath, 'const = ;')
  await until(async () => (await api('get()')).data.diagnostics.some(d => d.phase === 'build'), 'build failure diagnostic')
  await writeFile(mainPath, code('P6 recovered'))
  await until(async () => (await api('get()')).data.project.revision > revision + 1, 'fixed rebuild')
  await openWith(contexts[1], 'P6 recovered')
  mark('An invalid source produces a build diagnostic; fixing it restores a usable development page')
  await stop(dev)
  await until(async () => { const s = (await api('get()')).data; return !s.connected && s.reconnectAvailable }, 'disconnected reconnect state')
  await startWatcher(marker.sessionId)
  assert.equal((await api('reconnect()')).ok, true)
  await openWith(contexts[2], 'P6 recovered')
  mark('Stopping and restarting the watcher reconnects to its rotated session and executes the current build')
  const image = await pluginCdp.command('Page.captureScreenshot', { format: 'png' })
  await writeFile(join(delivery, '开发上下文验收.png'), Buffer.from(image.data, 'base64'))
  assert.equal((await api('disconnect()')).ok, true)
  await stop(dev)
  cli(['build'])
  const formalV1 = join(fixtureRoot, 'formal-v1.wtplugin'), formalV2 = join(fixtureRoot, 'formal-v2.wtplugin')
  cli(['pack', 'dist', '--out', formalV1, '--host-version', '0.1.0', '--project-root', author])
  const installProfile = join(root, 'install-profile')
  const installed = new PluginManager({ userData: installProfile, hostVersion: '0.1.0', confirm: async () => true, externalOpen: async () => {}, clipboardWrite() {}, ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: '', model: '' }) } })
  await installed.initialize()
  try {
    assert.equal((await installed.install(await readFile(formalV1), installed.session)).outcome, 'installed')
    mark('Official SDK build/pack output installs through the production PluginManager')
    await installed.store.writeData(manifest.id, 'upgrade-proof', 'retained', () => true)
    manifest.version = '1.0.1'; await writeFile(manifestPath, JSON.stringify(manifest, null, 2))
    cli(['build']); cli(['pack', 'dist', '--out', formalV2, '--host-version', '0.1.0', '--project-root', author])
    const upgraded = await installed.install(await readFile(formalV2), installed.session)
    assert.equal(upgraded.outcome, 'installed'); assert.equal(upgraded.plugin.version, '1.0.1')
    assert.equal(await installed.store.readData(manifest.id, 'upgrade-proof'), 'retained')
  } finally { installed.close() }
  mark('Official SDK build/pack output installs and upgrades through PluginManager while preserving private data')
  report.passed = true
} catch (error) { report.error = String(error.stack ?? error); throw error }
finally {
  for (const ws of sockets) ws.close()
  await stop(dev); await stop(manager)
  await writeFile(join(root, 'runtime.log'), output)
  await writeFile(join(delivery, 'development-verification.json'), JSON.stringify(report, null, 2))
}
