// Run against the portable packaged Manager and external toolchain projects.
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import { copyFile, cp, mkdir, mkdtemp, readFile, realpath, writeFile } from 'node:fs/promises'
import { dirname, isAbsolute, join, relative } from 'node:path'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'
import { createRuntimeOrigin } from '../electron/plugins/web-runtime-policy.ts'
import { freePort, until } from './mcp-native-test-support.mjs'

assert.ok(isAbsolute(process.argv[2] ?? ''), 'Supply the absolute preview directory')
const delivery = await realpath(process.argv[2])
const testTemp = dirname(delivery)
const root = await mkdtemp(join(testTemp, 'wtp6-'))
const profile = join(root, 'profile')
const executable = join(delivery, 'stage', 'Manager', 'WebTools.exe')
const projects = join(delivery, 'projects')
const author = join(root, 'author')
const port = await freePort()
const sockets = new Set()
const report = { passed: false, checks: [] }
const mark = name => { report.checks.push(name); console.log(name) }
let log = ''
let manager, watcher
const capture = child => {
  child.stdout.on('data', bytes => { log += bytes })
  child.stderr.on('data', bytes => { log += bytes })
  return child
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return
  const exited = once(child, 'exit')
  child.kill()
  await exited
}
const targets = async () => (await (await fetch('http://127.0.0.1:' + port + '/json/list', { signal: AbortSignal.timeout(1500) })).json())
async function debuggerFor(predicate, label, type = 'page') {
  const target = await until(async () => (await targets()).find(row => (type === '*' || row.type === type) && predicate(row.url, row.id)), label)
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  sockets.add(socket)
  await Promise.race([once(socket, 'open'), once(socket, 'error').then(([error]) => Promise.reject(error))])
  let sequence = 0
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence
    const timer = setTimeout(() => { socket.removeEventListener('message', receive); reject(Error('CDP timed out: ' + method)) }, 12000)
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
  return { id: target.id, evaluate: async expression => (await send('Runtime.evaluate', {
    expression, awaitPromise: true, returnByValue: true, userGesture: true,
  })).result?.value, close() { socket.close(); sockets.delete(socket) } }
}

await mkdir(profile)
await mkdir(join(root, 'fixtures'))
try {
  const packagePath = name => join(projects, name, name + '.wtplugin')
  const seed = new PluginManager({ userData: profile, hostVersion: '0.1.0', confirm: async () => true, externalOpen: async () => {},
    clipboardWrite() {}, ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: '', model: '' }) } })
  await seed.initialize()
  try {
    for (const name of ['vanilla-web', 'vue-node', 'react-web', 'svelte-node']) {
      const result = await seed.install(await readFile(packagePath(name)), seed.session)
      assert.equal(result.outcome, 'installed')
      await seed.setEnabled(result.plugin.id, true, seed.session)
    }
  } finally { seed.close() }
  for (const name of ['fixture', 'update', 'isolation']) {
    await copyFile(packagePath('vanilla-web'), join(root, 'fixtures', 'plugin-runtime-' + name + '.wtplugin'))
  }
  await writeFile(join(profile, 'plugin-runtime-controls.json'), '{"package":"fixture","deleteData":false}')
  manager = capture(spawn(executable, ['--plugin-runtime-acceptance'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
    ...process.env, TEMP: testTemp, TMP: testTemp, WEBTOOLS_PLUGIN_RUNTIME_ACCEPTANCE: '1', WEBTOOLS_PLUGIN_RUNTIME_STANDALONE: '1',
    WEBTOOLS_PLUGIN_RUNTIME_TEST_ROOT: root, WEBTOOLS_PLUGIN_RUNTIME_TEST_PROFILE: profile, WEBTOOLS_PLUGIN_RUNTIME_TEST_DEBUG_PORT: String(port),
  } }))
  const settings = await debuggerFor(url => url.endsWith('/out/renderer/index.html'), 'packaged Manager')
  await until(() => settings.evaluate('!!window.desktop?.pluginCatalog'), 'Manager API ready')
  await settings.evaluate("window.desktop.managerWindow.perform('toggle-detach')")
  async function openHome(name) {
    const ref = { kind: 'web', id: 'org.example.' + name, featureId: 'home' }
    const result = await settings.evaluate('window.desktop.pluginCatalog.open(' + JSON.stringify(ref) + ')')
    assert.equal(result.ok, true, JSON.stringify(result))
    const expectedUrl = createRuntimeOrigin(ref.id).origin + '/web/index.html'
    const page = await debuggerFor(url => url === expectedUrl, name + ' plugin page', '*')
    await until(() => page.evaluate('document.documentElement.dataset.framework === ' + JSON.stringify(name.split('-')[0]) +
      " && document.querySelector('#open-search:not(:disabled)') && document.querySelector('#open-child:not(:disabled)') ? true : false"), name + ' page and public SDK')
    return page
  }
  const page = await openHome('vanilla-web')
  mark('vanilla-web mounted with public SDK and controls')
  await page.evaluate("document.querySelector('#open-search').click()")
  const chrome = await debuggerFor(url => url.endsWith('/plugin-window.html'), 'plugin window chrome')
  await until(() => chrome.evaluate("document.querySelector('#plugin-window-search-input')?.getAttribute('placeholder')?.includes('剪贴板')"), 'host sub-input')
  await chrome.evaluate("(() => {const field = document.querySelector('#plugin-window-search-input'); field.value = '文件'; field.dispatchEvent(new Event('input', {bubbles:true}));})()")
  await until(() => chrome.evaluate("document.querySelector('.plugin-search-result-title')?.textContent === '文件'"), 'dynamic File result')
  await chrome.evaluate("document.querySelector('.plugin-search-result').click()")
  await until(() => page.evaluate("document.querySelector('#search-status').textContent === '已选择：文件'"), 'selected result reaches plugin')
  chrome.close()
  mark('Host sub-input publishes a live query and selecting the result changes the plugin page')

  await page.evaluate("document.querySelector('#open-child').click()")
  const child = await debuggerFor(url => url.endsWith('/web/child.html'), 'plugin child document', '*')
  await until(() => child.evaluate("!!document.querySelector('#send:not(:disabled)')"), 'child SDK ready')
  await child.evaluate("document.querySelector('#send').click()")
  await until(() => page.evaluate("document.querySelector('#window-status').textContent === '子窗口消息：来自子窗口的问候'"), 'child message reaches owner')
  await page.evaluate("document.querySelector('#close-child').click()")
  await until(async () => !(await targets()).some(target => target.id === child.id), 'child removed')
  await until(() => page.evaluate("document.querySelector('#window-status').textContent === '子窗口已关闭'"), 'child closed event')
  child.close()
  page.close()
  mark('Child opened its own built document, sent a message and closed')
  for (const name of ['vue-node', 'react-web', 'svelte-node']) {
    const opened = await openHome(name)
    if (name.endsWith('-node')) assert.match(await opened.evaluate("document.querySelector('.wt-tag').textContent"), /Plugin preload: node/u)
    opened.close()
    mark(name + ' mounted with public SDK and controls')
  }

  const source = join(projects, 'vue-node')
  await cp(source, author, { recursive: true, dereference: true,
    filter: file => !['.webtools', 'dist'].includes(relative(source, file).split(/[\\/]/u)[0]) && !file.endsWith('.wtplugin') })
  const manifestPath = join(author, 'manifest.json')
  const manifest = JSON.parse(await readFile(manifestPath, 'utf8'))
  manifest.id = 'org.webtools.acceptance.runtime'
  await writeFile(manifestPath, JSON.stringify(manifest))
  watcher = capture(spawn(process.execPath, [join(author, 'node_modules/@webtools/plugin-sdk/bin/webtools-plugin.mjs'), 'dev', author],
    { cwd: author, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] }))
  await until(async () => Boolean(JSON.parse(await readFile(join(author, '.webtools/development.json'), 'utf8')).sessionId), 'external template development watcher')
  assert.equal((await settings.evaluate('window.desktop.pluginDevelopment.setEnabled(true)')).ok, true)
  assert.equal((await settings.evaluate('window.desktop.pluginDevelopment.connectFromDialog()')).ok, true)
  const filePath = join(root, '文件上下文样例.txt')
  await writeFile(filePath, 'Test fixture')
  const fileInvocation = { surface: 'super-panel', context: { type: 'files', source: 'explorer-selection', items: [{ kind: 'file', path: filePath }] } }
  const existing = new Set((await targets()).map(target => target.id))
  const loaded = await settings.evaluate("window.desktop.pluginDevelopment.openFeature('selected-files'," + JSON.stringify(fileInvocation) + ')')
  assert.equal(loaded.ok, true, JSON.stringify(loaded))
  const filePage = await debuggerFor((url, id) => !existing.has(id) && url.startsWith('webtools-plugin:') && url.endsWith('/web/index.html'), 'file context plugin page', '*')
  await until(() => filePage.evaluate("document.querySelector('#selected-files li')?.textContent === " + JSON.stringify(filePath)), 'rendered file context')
  filePage.close()
  mark('Packaged development host passes a simulated Explorer file selection to the generated Vue Node template')
  const command = await settings.evaluate("window.desktop.pluginDevelopment.openFeature('normalize-selection', {surface:'super-panel', context:{type:'text',source:'selected-text',text:'  WebTools  test  '}})")
  if (!command.ok) {
    const state = await settings.evaluate('window.desktop.pluginDevelopment.get()')
    assert.fail(JSON.stringify({ command, diagnostics: state.data?.diagnostics }))
  }
  mark('Generated Node command runs in the packaged development host with simulated selected text')
  report.passed = true
} catch (error) {
  report.error = String(error.stack ?? error)
  throw error
} finally {
  for (const socket of sockets) socket.close()
  await Promise.all([stop(manager), stop(watcher)])
  await writeFile(join(root, 'host.log'), log)
  await writeFile(join(delivery, 'template-runtime-verification.json'), JSON.stringify(report, null, 2) + '\n')
}
