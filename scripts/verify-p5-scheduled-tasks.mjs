// Isolated, production-shaped acceptance: actual Rust tray -> actual Electron ->
// hidden Web SDK callback / Node command -> persisted Rust execution counters.
// Run with node --experimental-transform-types. No real profile is read/written.
import assert from 'node:assert/strict'
import { spawn, execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { mkdir, readFile, writeFile, realpath, readdir } from 'node:fs/promises'
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'
import { setTimeout as delay } from 'node:timers/promises'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'
import { runtimeV3Manifest, runtimeV3Package } from '../electron/plugins/fixtures.mjs'

const repo = resolve(fileURLToPath(new URL('..', import.meta.url)))
const args = new Map()
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1])
const required = name => { const value = args.get(name); if (!value || !isAbsolute(value)) throw new Error(`${name} requires an absolute path`); return resolve(value) }
const root = required('--root')
const nativeExe = await realpath(required('--native-exe'))
const managerExe = await realpath(required('--manager-exe'))
const inside = (base, target) => { const r = relative(base, target); return r !== '' && r !== '..' && !r.startsWith(`..${sep}`) && !isAbsolute(r) }
assert.ok(inside(join(repo, '.tmp'), root) || inside('D:\\SystemDefault\\Desktop\\webtoolstest', root), 'root must be a new directory under repo/.tmp or Desktop/webtoolstest')
assert.ok(!inside(root, nativeExe) && !inside(root, managerExe), 'profile root must be separate from executable staging')
try { assert.equal((await readdir(root)).length, 0, 'refusing to reuse a nonempty acceptance root') } catch (error) { if (error.code !== 'ENOENT') throw error }
const nativeProfile = join(root, 'native-profile')
const managerProfile = join(nativeProfile, 'runtime', 'wt-manager-profile')
await mkdir(managerProfile, { recursive: true })
const playground = await readFile(args.get('--playground') ?? join(repo, 'release', 'webtools-plugin-platform-playground-1.15.0.wtplugin'))
const nodeId = 'org.webtools.acceptance.p5-node'
const nodeManifest = runtimeV3Manifest({ id: nodeId, runtime: { kind: 'node' }, requestedCapabilities: ['plugin.tasks', 'plugin.storage.read', 'plugin.storage.write'],
  features: [{ id: 'scheduled', title: 'P5 scheduled Node', entry: { kind: 'node-command', module: 'command.cjs' }, presentation: { type: 'command' },
    requiredCapabilities: ['plugin.tasks', 'plugin.storage.read', 'plugin.storage.write'] }], runtimeFiles: [{ path: 'command.cjs', role: 'node-command', mediaType: 'text/javascript' }] })
const nodePackage = runtimeV3Package(nodeManifest, [{ name: 'command.cjs', data: `module.exports=async({task,webtools})=>{
  if(!task)throw new Error('Missing stable schedule identity');
  await new Promise(r=>setTimeout(r,150));
  const previous=await webtools.storage.get('executions');
  await webtools.storage.set('executions',[...(Array.isArray(previous)?previous:[]),task]);
  if(task.code.endsWith('-fail'))throw new Error('P5 intentional Node failure');
}` }])
// Fixture provisioning only: production task creation still requires the real
// native dialog. The browser capability regression tests cover deny/approve.
const manager = new PluginManager({ userData: managerProfile, hostVersion: '0.1.0', confirm: async () => true, externalOpen: async () => {}, clipboardWrite() {},
  ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: 'unused', model: 'unused' }) } })
await manager.initialize()
const webInstalled = await manager.install(playground, manager.session)
const webId = 'org.webtools.plugin-platform-playground'
await manager.setEnabled(webId, true, manager.session)
await manager.install(nodePackage, manager.session); await manager.setEnabled(nodeId, true, manager.session)
manager.close()
const startedAt = Date.now()
const task = (pluginId, featureId, code, delayMs) => ({ pluginId, featureId, code, description: 'P5 isolated acceptance fixture', schedule: { kind: 'once', atMs: startedAt + delayMs },
  nextRunMs: startedAt + delayMs, createdAtMs: startedAt, enabled: true, running: false, successes: 0, failures: 0, lastRunMs: null, lastError: null })
const scheduleFile = join(nativeProfile, 'plugin-scheduled-tasks.json')
const tasks = [task(webId, 'playground', 'web-success', 4000), task(webId, 'playground', 'web-fail', 6000),
  task(nodeId, 'scheduled', 'node-success', 8000), task(nodeId, 'scheduled', 'node-fail', 10000)]
await writeFile(scheduleFile, JSON.stringify({ tasks }, null, 2))
const config = join(root, 'launcher.json')
await writeFile(config, JSON.stringify({ managerExe, managerArgs: ['--phase4g-manager-test', '--manager-only'] }))
let log = ''
const child = spawn(nativeExe, ['--startup'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
  ...process.env, WT_PROFILE_ROOT: nativeProfile, WT_CONFIG: config, TEMP: root, TMP: root,
} })
child.stdout.on('data', data => { log += data }); child.stderr.on('data', data => { log += data })
let exited = false; child.once('exit', () => { exited = true })
const execFileAsync = promisify(execFile)
async function managerChildren() {
  const script = `$items = @(Get-CimInstance Win32_Process -Filter 'ParentProcessId = ${child.pid}' | Select-Object ProcessId, ExecutablePath); ConvertTo-Json -InputObject $items -Compress`
  const { stdout } = await execFileAsync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', script], { windowsHide: true })
  return JSON.parse(stdout || '[]').filter(p => p.ExecutablePath?.toLowerCase() === managerExe.toLowerCase())
}
async function until(check, timeoutMs, label) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (exited) throw new Error(`Native Host exited during ${label}: ${log.slice(-6000)}`)
    const value = await check(); if (value) return value
    await delay(250)
  } throw new Error(`Timed out: ${label}\n${log.slice(-6000)}`)
}
const report = { startedAt: new Date(startedAt).toISOString(), nativeExe, managerExe, nativeProfile, managerProfile, webInstalled,
  fixtureProvisioning: 'Installed and enabled through PluginManager with test consent; task records seeded as previously approved user tasks. Real create-task dialog is a separate manual check.', checks: [] }
try {
  const state = await until(async () => {
    let value; try { value = JSON.parse(await readFile(scheduleFile, 'utf8')) } catch { return false }
    return value.tasks.every(t => t.successes + t.failures === 1 && !t.running) && value
  }, 100000, 'all four real callback attempts')
  for (const code of ['web-success', 'node-success']) { const row = state.tasks.find(t => t.code === code); assert.equal(row.successes, 1, JSON.stringify(row)); assert.equal(row.failures, 0) }
  for (const code of ['web-fail', 'node-fail']) { const row = state.tasks.find(t => t.code === code); assert.equal(row.failures, 1, JSON.stringify(row)); assert.match(row.lastError, /intentional/i) }
  report.checks.push('Rust tray starts schedule timer without any UI interaction', 'Rust due tasks wake real Electron over the existing named pipe',
    'Hidden Web callback awaits async work and records successful/failed outcomes', 'Node command receives stable task identity and records successful/failed outcomes')
  await until(async () => (await managerChildren()).length === 0, 15000, 'Electron cooperative idle exit')
  report.checks.push('Electron exits cooperatively after the last scheduled owner completes')
  const webData = JSON.parse(await readFile(join(managerProfile, 'plugins', 'data', `${webId}.json`), 'utf8'))
  const nodeData = JSON.parse(await readFile(join(managerProfile, 'plugins', 'data', `${nodeId}.json`), 'utf8'))
  assert.equal(webData['schedule-history'].length, 2); assert.equal(nodeData.executions.length, 2)
  assert.deepEqual(new Set(webData['schedule-history'].map(e => e.code)), new Set(['web-success', 'web-fail']))
  assert.deepEqual(new Set(nodeData.executions.map(e => e.code)), new Set(['node-success', 'node-fail']))
  report.checks.push('Both runtime kinds persisted callback side effects in isolated per-plugin storage')
  report.state = state; report.webHistory = webData['schedule-history']; report.nodeHistory = nodeData.executions
  report.passed = true
  console.log(JSON.stringify(report, null, 2))
} catch (error) { report.passed = false; report.error = String(error?.stack ?? error); throw error }
finally {
  // Only this runner's owned isolated tray process is stopped; the Host job owns
  // any remaining test Electron descendants. No system-wide process cleanup.
  child.kill(); await writeFile(join(root, 'native-host.log'), log)
  await writeFile(join(root, 'p5-scheduled-tasks-evidence.json'), JSON.stringify(report, null, 2))
}
