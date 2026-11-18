import assert from 'node:assert/strict'
import { parseArgs } from 'node:util'
import { mkdir, writeFile } from 'node:fs/promises'
import { createServer } from 'node:net'
import { join } from 'node:path'
import { fixture, until, processGroup } from './mcp-native-test-support.mjs'
const { values } = parseArgs({ options: { delivery: { type: 'string' }, 'plugin-package': { type: 'string' } } })
assert.ok(values.delivery && values['plugin-package'])
const env = await fixture(values.delivery, values['plugin-package'], { inspector: true })
const checks = [], sockets = [], output = join(values.delivery, 'verification', 'manager-ui')
await mkdir(output, { recursive: true })
const mark = value => { checks.push(value); console.log(value) }
async function debuggerAt(port, predicate = () => true) {
  const target = await until(async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(predicate), 'debug target')
  const socket = new WebSocket(target.webSocketDebuggerUrl); await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) }); sockets.push(socket)
  let sequence = 0
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence, timer = setTimeout(() => reject(Error(`CDP timeout: ${method}`)), 15000)
    const receive = event => { const value = JSON.parse(event.data); if (value.id !== id) return; clearTimeout(timer); socket.removeEventListener('message', receive); if (value.error || value.result?.exceptionDetails) reject(Error(JSON.stringify(value.error ?? value.result.exceptionDetails))); else resolve(value.result) }
    socket.addEventListener('message', receive); socket.send(JSON.stringify({ id, method, params }))
  })
  return { send, eval: async expression => (await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true })).result?.value }
}
let occupied
try {
  const client = await env.client(); await client.callTool({ name: `${env.plugin}.agent-note`, arguments: { text: 'UI acceptance' } })
  const main = await debuggerAt(env.nodePort)
  await until(() => main.eval(`process.mainModule.require('electron').app.listenerCount('activate') > 0`), 'Main ready')
  await main.eval(`process.mainModule.require('electron').app.emit('activate')`)
  const page = await debuggerAt(env.debugPort, target => target.type === 'page' && target.url.includes('index.html'))
  await until(() => page.eval(`!!window.desktop?.pluginMcp && !!document.querySelector('.app-frame')`), 'Manager renderer')
  await page.eval(`document.querySelector('.sidebar-bottom .nav-item').click()`)
  await until(() => page.eval(`[...document.querySelectorAll('button')].find(b => b.textContent.trim() === 'AI')?.click() || !!document.querySelector('.mcp-settings')`), 'AI settings')
  await until(() => page.eval(`document.querySelector('.mcp-settings .switch')?.getAttribute('aria-checked') === 'true'`), 'MCP status')
  mark('Existing Manager → settings → AI shows the Rust-owned running service')
  await page.eval(`[...document.querySelectorAll('.mcp-settings button')].find(b=>b.textContent.trim()==='连接 AI Agent').click()`)
  const config = JSON.parse(await until(() => page.eval(`document.querySelector('.connect-dialog textarea')?.value`), 'connection config')).mcpServers.webtools
  assert.equal(config.url, env.url); assert.equal(config.headers.Authorization, `Bearer ${env.settings.token}`)
  await until(() => page.eval(`document.activeElement?.closest('.connect-dialog') !== null`), 'dialog focus')
  await page.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await page.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await until(() => page.eval(`!document.querySelector('.connect-dialog') && document.activeElement?.textContent.trim()==='连接 AI Agent'`), 'Escape and focus restoration')
  mark('Connection dialog preserves URL/token and Escape returns keyboard focus')
  const pluginName = await page.eval(`window.desktop.plugins.list().then(r=>r.data.find(p=>p.id===${JSON.stringify(env.plugin)}).name)`)
  const rowClick = `[...document.querySelectorAll('.mcp-settings .plugin-row')].find(r=>r.querySelector('strong')?.textContent===${JSON.stringify(pluginName)}).querySelector('.switch').click()`
  await page.eval(rowClick)
  await until(async () => !(await client.listTools()).tools.some(tool => tool.name === `${env.plugin}.agent-note`), 'plugin MCP switch')
  assert.equal(await page.eval(`window.desktop.plugins.list().then(r=>r.data.find(p=>p.id===${JSON.stringify(env.plugin)}).enabled)`), true)
  await until(() => page.eval(`[...document.querySelectorAll('.mcp-settings .plugin-row')].find(r=>r.querySelector('strong')?.textContent===${JSON.stringify(pluginName)})?.querySelector('.switch:not(:disabled)')?.getAttribute('aria-checked') === 'false'`), 'plugin switch ready after refresh')
  await page.eval(rowClick); await until(async () => (await client.listTools()).tools.some(tool => tool.name === `${env.plugin}.agent-note`), 'plugin tools restored')
  await until(() => page.eval(`!!document.querySelector('.mcp-settings .service-card .switch:not(:disabled)')`), 'service switch ready')
  mark('Plugin tool switch changes discovery without disabling the installed plugin')
  await page.eval(`document.querySelector('.mcp-settings .service-card .switch').click()`)
  await until(() => page.eval(`document.querySelector('.mcp-settings .service-card .switch').getAttribute('aria-checked')==='false'`), 'listener disabled')
  occupied = createServer(); await new Promise(resolve => occupied.listen(env.settings.port, '127.0.0.1', resolve))
  await page.eval(`document.querySelector('.mcp-settings .service-card .switch').click()`)
  await until(() => page.eval(`[...document.querySelectorAll('.mcp-settings button')].some(b=>b.textContent.trim()==='重试' && !b.disabled)`), 'port error and retry')
  await new Promise(resolve => occupied.close(resolve)); occupied = undefined
  await page.eval(`[...document.querySelectorAll('.mcp-settings button')].find(b=>b.textContent.trim()==='重试').click()`)
  await until(() => page.eval(`window.desktop.pluginMcp.get().then(r=>r.ok && r.data.running)`), 'retry started listener')
  mark('Service stop, occupied-port error and retry work through the same settings page')
  const palettes = []
  for (const theme of ['light', 'dark']) {
    await page.eval(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='外观').click()`)
    const themeLabel = theme === 'light' ? '浅色' : '深色'
    await until(() => page.eval(`!![...document.querySelectorAll('.theme-options button')].find(b=>b.textContent.trim()===${JSON.stringify(themeLabel)} && !b.disabled)`), 'theme controls')
    await page.eval(`[...document.querySelectorAll('.theme-options button')].find(b=>b.textContent.trim()===${JSON.stringify(themeLabel)}).click()`)
    await until(() => page.eval(`[...document.querySelectorAll('.theme-options button')].find(b=>b.textContent.trim()===${JSON.stringify(themeLabel)})?.getAttribute('aria-pressed')==='true'`), 'saved theme')
    await page.eval(`[...document.querySelectorAll('button')].find(b=>b.textContent.trim()==='AI').click()`)
    await until(() => page.eval(`!!document.querySelector('.mcp-settings .switch:not(:disabled)')`), 'AI settings after theme change')
    await page.send('Emulation.setDeviceMetricsOverride', { width: 1000, height: 760, deviceScaleFactor: 1, mobile: false })
    palettes.push(await page.eval(`getComputedStyle(document.querySelector('.mcp-settings .service-card')).backgroundColor`))
    const shot = await page.send('Page.captureScreenshot', { format: 'png' }); await writeFile(join(output, `mcp-${theme}.png`), Buffer.from(shot.data, 'base64'))
  }
  assert.notEqual(palettes[0], palettes[1], 'Actual light/dark surfaces must differ')
  mark('Actual settings renderer captured in both saved light and dark themes')
  await main.eval(`process.mainModule.require('electron').BrowserWindow.getAllWindows().forEach(window=>window.close())`)
  // Node intentionally waits for an attached debugger on exit; detach before measuring lifecycle.
  sockets.forEach(socket => socket.close())
  await until(async () => !(await processGroup(env.child.pid)).some(p => p.name.toLowerCase() === 'webtools.exe'), 'Manager close and cooldown', 35_000)
  await client.listTools(); assert.equal((await processGroup(env.child.pid)).filter(p => p.name.toLowerCase() === 'webtools.exe').length, 0)
  mark('Closing Manager lets Electron exit while the same MCP endpoint keeps serving tools')
} finally {
  occupied?.close(); sockets.forEach(socket => socket.close()); await env.stop(); await writeFile(join(output, 'report.json'), JSON.stringify({ checks, root: env.root }, null, 2)); console.log(`UI evidence: ${output}`)
}
