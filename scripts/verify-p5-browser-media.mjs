
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createServer } from 'node:net'
import { copyFile, mkdir, writeFile, readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'

const delivery = process.argv[2]
const exe = join(delivery, 'stage/Manager/WebTools.exe')
const root = join(delivery, 'verification', 'isolated-' + Date.now())
const profile = join(root, 'profile')
await mkdir(profile, { recursive: true })
await mkdir(join(root, 'fixtures'))
for (const name of ['fixture', 'update', 'isolation']) {
  await copyFile(join(delivery, 'webtools-plugin-platform-playground-1.18.0.wtplugin'), join(root, 'fixtures', 'plugin-runtime-' + name + '.wtplugin'))
}
const plugin = 'org.webtools.plugin-platform-playground'
const seed = new PluginManager({ userData: profile, hostVersion: '0.1.0', confirm: async () => true, externalOpen: async () => {}, clipboardWrite() {},
  ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: 'unused', model: 'unused' }) } })
await seed.initialize()
const installed = await seed.install(await readFile(join(delivery, 'webtools-plugin-platform-playground-1.18.0.wtplugin')), seed.session)
assert.equal(installed.outcome, 'installed', JSON.stringify(installed))
await seed.setEnabled(plugin, true, seed.session)
seed.close()
const portProbe = createServer()
await new Promise(resolve => portProbe.listen(0, '127.0.0.1', resolve))
const port = portProbe.address().port
await new Promise(resolve => portProbe.close(resolve))
function launch() { return spawn(exe, ['--plugin-runtime-acceptance'], { stdio: ['ignore', 'pipe', 'pipe'], env: {
  ...process.env, TEMP: delivery, TMP: delivery, WEBTOOLS_PLUGIN_RUNTIME_ACCEPTANCE: '1',
  WEBTOOLS_PLUGIN_RUNTIME_STANDALONE: '1', WEBTOOLS_PLUGIN_RUNTIME_TEST_ROOT: root,
  WEBTOOLS_PLUGIN_RUNTIME_TEST_PROFILE: profile, WEBTOOLS_PLUGIN_RUNTIME_TEST_DEBUG_PORT: String(port),
} })
}
let child = launch()
let output = ''
child.stdout.on('data', data => { output += data })
child.stderr.on('data', data => { output += data })
let ws, client, sequence = 0
const checks = []
function mark(check) { checks.push(check); console.log(check) }
async function until(fn) {
  for (let i = 0; i < 100; i++) {
    if (child.exitCode !== null) throw Error('Packaged Manager exited: ' + child.exitCode)
    const result = await fn().catch(() => undefined)
    if (result) return result
    await sleep(200)
  }
  throw Error('Packaged Manager readiness timed out.')
}
async function evalPage(expression) {
  const id = ++sequence
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.removeEventListener('message', listener); reject(Error('CDP timeout')) }, 15000)
    function listener(event) {
      const message = JSON.parse(event.data)
      if (message.id !== id) return
      ws.removeEventListener('message', listener)
      clearTimeout(timer)
      if (message.error || message.result.exceptionDetails) reject(Error('CDP evaluate failed: ' + JSON.stringify(message.error ?? message.result.exceptionDetails)))
      else resolve(message.result.result.value)
    }
    ws.addEventListener('message', listener)
    ws.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true, userGesture: true } }))
  })
}
async function rpc(method, params = {}) {
  const id = ++sequence
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.removeEventListener('message', listener); reject(Error(method + ' timeout')) }, 20000)
    function listener(event) { const m = JSON.parse(event.data); if (m.id !== id) return; clearTimeout(timer); ws.removeEventListener('message', listener); m.error ? reject(Error(JSON.stringify(m.error))) : resolve(m.result) }
    ws.addEventListener('message', listener); ws.send(JSON.stringify({ id, method, params }))
  })
}
async function click(id) {
  const point = await evalPage(`(() => { const e = document.getElementById(${JSON.stringify(id)}); e.scrollIntoView({block:'center'}); const r=e.getBoundingClientRect(); return { x:r.x+r.width/2,y:r.y+r.height/2 } })()`)
  await rpc('Input.dispatchMouseEvent', {type:'mousePressed',button:'left',clickCount:1,...point})
  await rpc('Input.dispatchMouseEvent', {type:'mouseReleased',button:'left',clickCount:1,...point})
}
async function targets() { return await (await fetch('http://127.0.0.1:' + port + '/json/list')).json() }
try {
  const target = await until(async () => (await targets()).find(item => item.type === 'page' && item.url.includes('index.html')))
  ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise(resolve => ws.addEventListener('open', resolve, {once:true}))
  await until(() => evalPage('!!window.desktop?.pluginCatalog'))
  await evalPage('void window.desktop.pluginCatalog.open({kind:"web",id:' + JSON.stringify(plugin) + ',featureId:"playground"}); true')
  const managerWs = ws
  const ui = await until(async () => (await targets()).find(item => item.url.includes('web/index.html')))
  ws = new WebSocket(ui.webSocketDebuggerUrl)
  await new Promise(resolve => ws.addEventListener('open', resolve, {once:true}))
  await until(() => evalPage('document.querySelector("#runtime-badge")?.textContent.includes("已连接")'))
  assert.equal(await evalPage('typeof window.workflowDemo?.start'), 'function')
  await click('workflow-browser')
  await until(() => evalPage('!document.getElementById("workflow-browser").disabled'))
  const status = await evalPage('document.getElementById("workflow-browser-status").textContent')
  assert.equal(status, '已收到：WebTools 网页验收')
  assert.match(await evalPage('document.getElementById("workflow-browser-image").src'), /^data:image\/png;base64,/)
  mark('Packaged plugin public SDK opens a real browser, fills input, clicks, reads and captures screenshot')
  await evalPage(`(() => { const canvas=document.createElement('canvas');canvas.width=640;canvas.height=400;canvas.getContext('2d').fillRect(0,0,640,400); const raw=atob(canvas.toDataURL().split(',')[1]);const bytes=Uint8Array.from(raw,c=>c.charCodeAt(0));const transfer=new DataTransfer();transfer.items.add(new File([bytes],'test.png',{type:'image/png'}));document.getElementById('workflow-image-file').files=transfer.files;document.getElementById('workflow-image-format').value='jpeg'; })()`)
  await click('workflow-image-run')
  await until(() => evalPage('document.getElementById("workflow-image-status").textContent.includes("字节")'))
  assert.match(await evalPage('document.getElementById("workflow-image-status").textContent'), /320 × 200/)
  assert.match(await evalPage('document.getElementById("workflow-image-result").src'), /^data:image\/jpeg;base64,/)
  mark('Packaged plugin public SDK converts real 640x400 PNG to 320x200 JPEG')
  await click('workflow-media-sample')
  const wavPath = await until(() => evalPage('document.getElementById("workflow-media-path").textContent.endsWith(".wav") && document.getElementById("workflow-media-path").textContent'))
  assert.equal((await readFile(wavPath)).subarray(0,4).toString(),'RIFF')
  mark('Plugin Node preload creates an actual one-second WAV fixture')
  const shot = await rpc('Page.captureScreenshot', {format:'png'})
  await writeFile(join(delivery,'工作流验收预览.png'),Buffer.from(shot.data,'base64'))
  await click('workflow-close')
  assert.equal((await targets()).filter(item => item.url.startsWith('http://127.0.0.1:') && item.type === 'page').length,0)
  mark('Explicit close removes the real browser')

} finally {
  ws?.close(); child.kill()
  await writeFile(join(root,'manager.log'),output)
  await writeFile(join(delivery,'workflow-verification.json'), JSON.stringify({checks,isolatedProfile:profile},null,2))
}
