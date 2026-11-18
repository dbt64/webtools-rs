import assert from 'node:assert/strict'
import { spawn, execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { createServer } from 'node:net'
import { setTimeout as sleep } from 'node:timers/promises'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'
import { runtimeV3Manifest, runtimeV3Package } from '../electron/plugins/fixtures.mjs'
const exec = promisify(execFile)
export async function freePort() { const server = createServer(); await new Promise(resolve => server.listen(0, '127.0.0.1', resolve)); const port = server.address().port; await new Promise(resolve => server.close(resolve)); return port }
export async function until(fn, label, timeoutMs = 30_000, abortReason = () => undefined) {
  const end = Date.now() + timeoutMs
  while (Date.now() < end) { const reason = abortReason(); if (reason) throw Error(reason); const result = await fn().catch(() => false); if (result) return result; await sleep(150) }
  throw Error(`Timed out: ${label}`)
}
export async function processGroup(rootPid) {
  assert.ok(Number.isInteger(rootPid) && rootPid > 0)
  const script = `$all=Get-CimInstance Win32_Process; $ids=[System.Collections.Generic.HashSet[int]]::new(); [void]$ids.Add(${rootPid}); do {$changed=$false; foreach($p in $all){if($ids.Contains([int]$p.ParentProcessId) -and $ids.Add([int]$p.ProcessId)){$changed=$true}}}while($changed); $rows=@(foreach($p in $all){if($ids.Contains([int]$p.ProcessId)){$live=Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue; if($live){[pscustomobject]@{pid=[int]$p.ProcessId;parent=[int]$p.ParentProcessId;name=$p.Name;privateBytes=$live.PrivateMemorySize64;workingSet=$live.WorkingSet64;cpu=$live.CPU;handles=$live.HandleCount}}}}); ConvertTo-Json -InputObject $rows -Compress`
  const { stdout } = await exec('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', script], { windowsHide: true, maxBuffer: 1024 * 1024 })
  return JSON.parse(stdout.trim() || '[]')
}
export async function killOwnedEngine(nativePid) {
  const group = await processGroup(nativePid)
  const main = group.find(p => p.parent === nativePid && p.name.toLowerCase() === 'webtools.exe')
  assert.ok(main, 'isolated Electron main process must exist')
  process.kill(main.pid)
  return main.pid
}
export const testTool = 'org.webtools.acceptance.mcp.counter'
export async function fixture(delivery, pluginPackage, { cache = true, cooldownMs, old = false, enabled = true, inspector = false } = {}) {
  delivery = resolve(delivery); pluginPackage = resolve(pluginPackage)
  const root = await mkdtemp(join(tmpdir(), 'wt-mcp-'))
  const nativeRoot = join(root, 'native-profile'), profile = join(nativeRoot, 'runtime', 'wt-manager-profile')
  await mkdir(profile, { recursive: true })
  const port = await freePort(), settings = { enabled, port, token: 'isolated-mcp-native-token-1234567890123456789', disabledPlugins: [] }
  await writeFile(join(profile, 'plugin-mcp-settings.json'), JSON.stringify(settings))
  await writeFile(join(nativeRoot, 'plugin-mcp-settings.json'), JSON.stringify({ enabled: old && enabled }))
  const seed = new PluginManager({ userData: profile, hostVersion: '0.1.0', confirm: async () => true, externalOpen: async () => {}, clipboardWrite() {}, ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: 'unused', model: 'unused' }) } })
  await seed.initialize()
  const installed = await seed.install(await readFile(pluginPackage), seed.session)
  assert.equal(installed.outcome, 'installed'); await seed.setEnabled(installed.plugin.id, true, seed.session)
  const manifest = runtimeV3Manifest({ id: 'org.webtools.acceptance.mcp', runtime: { kind: 'node' }, requestedCapabilities: ['plugin.storage.read', 'plugin.storage.write'], features: [{ id: 'counter', title: 'MCP lifecycle test', entry: { kind: 'node-command', module: 'counter.cjs' }, presentation: { type: 'command' }, requiredCapabilities: ['plugin.storage.read', 'plugin.storage.write'], mcpTool: { inputSchema: { type: 'object', properties: { text: { type: 'string' }, delayMs: { type: 'integer', minimum: 0, maximum: 25000 } }, additionalProperties: false }, outputSchema: { type: 'object', properties: { text: { type: 'string' }, count: { type: 'integer' } }, required: ['text', 'count'] } } }], runtimeFiles: [{ path: 'counter.cjs', role: 'node-command', mediaType: 'text/javascript' }] })
  const command = `module.exports=async({mcpInput,webtools})=>{let count=await webtools.storage.get('count')||0;let text=await webtools.storage.get('text')||'';if(typeof mcpInput.text==='string'){text=mcpInput.text;count++;await webtools.storage.set('count',count);await webtools.storage.set('text',text)}if(mcpInput.delayMs)await new Promise(r=>setTimeout(r,mcpInput.delayMs));return{text,count}}`
  const custom = await seed.install(runtimeV3Package(manifest, [{ name: 'counter.cjs', data: command }]), seed.session)
  assert.equal(custom.outcome, 'installed'); await seed.setEnabled(manifest.id, true, seed.session)
  if (cache) await writeFile(join(profile, 'mcp-tools-projection.json'), JSON.stringify({ schemaVersion: 1, revision: 1, tools: seed.listMcpTools() }))
  seed.close()
  const config = join(root, 'launcher.json')
  const nodePort = inspector ? await freePort() : undefined, debugPort = inspector ? await freePort() : undefined
  await writeFile(config, JSON.stringify({ managerExe: join(delivery, 'stage', 'Manager', 'WebTools.exe'), managerArgs: ['--manager-only', '--phase4g-manager-test', ...(inspector ? [`--inspect=127.0.0.1:${nodePort}`, `--remote-debugging-port=${debugPort}`] : [])] }))
  const env = { ...process.env, WT_PROFILE_ROOT: nativeRoot, WT_CONFIG: config, TEMP: root, TMP: root }
  if (cooldownMs !== undefined) { env.WT_MCP_GATEWAY_ACCEPTANCE = '1'; env.WT_MCP_ACCEPTANCE_COOLDOWN_MS = String(cooldownMs) }
  let log = ''
  const child = spawn(join(delivery, 'stage', 'WebTools.NativeHost.Rust.exe'), ['--app', '--startup'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env })
  child.stdout.on('data', data => { log += data }); child.stderr.on('data', data => { log += data }); child.on('error', error => { log += error.stack })
  const url = `http://127.0.0.1:${port}/mcp`, clients = []
  try {
    await until(async () => enabled ? (await fetch(url, { method: 'POST', signal: AbortSignal.timeout(1000) })).status === 401 : (await processGroup(child.pid)).some(row => row.pid === child.pid), enabled ? 'MCP listener' : 'Native Host startup', 30_000, () => child.exitCode !== null ? `Native Host exited: ${log}` : undefined)
  } catch (error) { child.kill(); await writeFile(join(root, 'native.log'), log); throw error }
  return { root, profile, nativeRoot, settings, child, url, plugin: installed.plugin.id, nodePort, debugPort,
    async client() { const client = new Client({ name: 'WebTools native acceptance', version: '1.0.0' }); await client.connect(new StreamableHTTPClientTransport(new URL(url), { requestInit: { headers: { Authorization: `Bearer ${settings.token}` } } })); clients.push(client); return client },
    async stop() { await Promise.allSettled(clients.map(client => client.close())); child.kill(); await writeFile(join(root, 'native.log'), log); await sleep(300) },
    log: () => log,
  }
}
