// Run with: node --experimental-transform-types scripts/verify-p5-schedule-management.mjs <absolute-delivery>
// Real packaged Rust Host -> named pipe -> Manager settings -> Rust task store.
// Seeded fixtures stand for already-confirmed tasks; creating a task's native
// consent dialog remains a separate manual acceptance step.
import assert from 'node:assert/strict'
import { spawn, execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { createServer } from 'node:net'
import { mkdir, readFile, writeFile, realpath } from 'node:fs/promises'
import { join, isAbsolute } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'
import { runtimeV3Manifest, runtimeV3Package } from '../electron/plugins/fixtures.mjs'

assert.ok(process.argv[2] && isAbsolute(process.argv[2]), 'absolute delivery directory required')
const delivery = await realpath(process.argv[2])
const nativeExe = await realpath(join(delivery, 'stage', 'WebTools.NativeHost.Rust.exe'))
const managerExe = await realpath(join(delivery, 'stage', 'Manager', 'WebTools.exe'))
// Keep utilityProcess's working directory below Windows CreateProcess's legacy
// limit even when this delivery already has a descriptive desktop directory.
const root = join(delivery, 'verification', `sm-${Date.now().toString(36)}`)
const nativeProfile = join(root, 'native')
const managerProfile = join(nativeProfile, 'runtime', 'wt-manager-profile')
await mkdir(managerProfile, { recursive: true })
const webId = 'org.webtools.plugin-platform-playground'
const nodeId = 'org.webtools.test.schedule'
const seed = new PluginManager({ userData: managerProfile, hostVersion: '0.1.0', confirm: async () => true,
  externalOpen: async () => {}, clipboardWrite() {}, ai: { complete: async () => ({ text: '' }),
    getDefaultProviderInfo: async () => ({ providerName: 'unused', model: 'unused' }) } })
await seed.initialize()
const installed = await seed.install(await readFile(join(delivery, 'webtools-plugin-platform-playground-1.18.0.wtplugin')), seed.session)
assert.equal(installed.outcome, 'installed', JSON.stringify(installed))
await seed.setEnabled(webId, true, seed.session)
const nodeManifest = runtimeV3Manifest({ id: nodeId, name: '定时管理隔离验收', runtime: { kind: 'node' },
  requestedCapabilities: ['plugin.tasks', 'plugin.storage.read', 'plugin.storage.write'],
  features: [{ id: 'scheduled', title: 'Management probe', entry: { kind: 'node-command', module: 'command.cjs' },
    presentation: { type: 'command' }, requiredCapabilities: ['plugin.tasks', 'plugin.storage.read', 'plugin.storage.write'] }],
  runtimeFiles: [{ path: 'command.cjs', role: 'node-command', mediaType: 'text/javascript' }] })
await seed.install(runtimeV3Package(nodeManifest, [{ name: 'command.cjs', data: `module.exports=async({task,webtools})=>{
  if(!task)throw new Error('Missing schedule identity');
  await webtools.storage.set('management-probe',task.code);
}` }]), seed.session)
await seed.setEnabled(nodeId, true, seed.session)
seed.close()

const startedAt = Date.now()
const task = (pluginId, featureId, code, milliseconds) => ({ pluginId, featureId, code, description: `管理验收 ${code}`,
  schedule: { kind: 'once', atMs: startedAt + milliseconds }, nextRunMs: startedAt + milliseconds, createdAtMs: startedAt,
  enabled: true, running: false, successes: 0, failures: 0, lastRunMs: null, lastError: null })
const scheduleFile = join(nativeProfile, 'plugin-scheduled-tasks.json')
await writeFile(scheduleFile, JSON.stringify({ tasks: [task(webId, 'playground', 'shared-code', 3_600_000),
  task(nodeId, 'scheduled', 'shared-code', 3_600_000), task(nodeId, 'scheduled', 'bootstrap', 3000)] }, null, 2))
const probe = createServer()
await new Promise(resolve => probe.listen(0, '127.0.0.1', resolve))
const port = probe.address().port
await new Promise(resolve => probe.close(resolve))
const config = join(root, 'launcher.json')
await writeFile(config, JSON.stringify({ managerExe, managerArgs: ['--phase4g-manager-test', '--manager-only', `--remote-debugging-port=${port}`] }))

// Invoke the existing tray Settings command, scoped to this newly spawned PID.
// This takes the same production ownership/open-page path as clicking Settings
// in the tray, rather than creating a standalone Manager or replacing its IPC.
const driver = join(root, 'tray-settings.ps1')
await writeFile(driver, `param([int]$TargetPid)
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class ScheduleTrayProbe {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc proc, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, UIntPtr w, IntPtr l);
  public static bool OpenSettings(IntPtr h) { return PostMessage(h,0x111,new UIntPtr(4),IntPtr.Zero); }
  public static IntPtr Find(uint target) {
    IntPtr found=IntPtr.Zero;
    EnumWindows((h,p)=>{ uint pid; GetWindowThreadProcessId(h,out pid);
      if(pid!=target)return true;
      var name=new StringBuilder(128); GetClassName(h,name,128);
      if(name.ToString()!="WebToolsSlintTray")return true;
      found=h;return false;
    },IntPtr.Zero);return found;
  }
}
'@
$deadline = [DateTime]::UtcNow.AddSeconds(15)
do {
  Get-Process -Id $TargetPid -ErrorAction Stop | Out-Null
  $window = [ScheduleTrayProbe]::Find([uint32]$TargetPid)
  if ($window -ne [IntPtr]::Zero) { break }
  Start-Sleep -Milliseconds 100
} while ([DateTime]::UtcNow -lt $deadline)
if ($window -eq [IntPtr]::Zero) { throw 'Owned Rust tray window was not found' }
if (-not [ScheduleTrayProbe]::OpenSettings($window)) { throw 'Tray Settings command failed' }
`, 'utf8')
let log = '', sequence = 0, ws
const child = spawn(nativeExe, ['--startup'], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'], env: {
  ...process.env, WT_PROFILE_ROOT: nativeProfile, WT_CONFIG: config, TEMP: root, TMP: root,
} })
child.stdout.on('data', data => { log += data }); child.stderr.on('data', data => { log += data })
child.on('error', error => { log += String(error) })
const report = { passed: false, startedAt: new Date(startedAt).toISOString(), nativeExe, managerExe,
  nativeProfile, managerProfile, fixtureProvisioning: 'Isolated installed packages and previously-approved task fixtures; no real user profile or task was changed.', checks: [] }
const mark = text => { report.checks.push(text); console.log(text) }
async function until(check, label, milliseconds = 30000) {
  const deadline = Date.now() + milliseconds
  while (Date.now() < deadline) {
    if (child.exitCode !== null) throw Error(`Owned Rust Host exited during ${label}: ${log.slice(-5000)}`)
    const value = await check().catch(() => false)
    if (value) return value
    await delay(200)
  }
  throw Error(`Timed out: ${label}\n${log.slice(-5000)}`)
}
function rpc(method, params = {}) {
  const id = ++sequence
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.removeEventListener('message', receive); reject(Error(`${method} timed out`)) }, 15000)
    function receive(event) {
      const message = JSON.parse(event.data)
      if (message.id !== id) return
      clearTimeout(timer); ws.removeEventListener('message', receive)
      message.error ? reject(Error(JSON.stringify(message.error))) : resolve(message.result)
    }
    ws.addEventListener('message', receive)
    ws.send(JSON.stringify({ id, method, params }))
  })
}
async function evaluate(expression) {
  const result = await rpc('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true })
  if (result.exceptionDetails) throw Error(JSON.stringify(result.exceptionDetails))
  return result.result.value
}
async function list() {
  const result = await evaluate('window.desktop.pluginTasks.list()')
  assert.equal(result.ok, true, JSON.stringify(result))
  return result.data
}
async function screenshot(name) {
  const captured = await rpc('Page.captureScreenshot', { format: 'png' })
  await writeFile(join(delivery, name), Buffer.from(captured.data, 'base64'))
}
const findCard = `[...document.querySelectorAll('.scheduled-tasks .task-card')].find(e => e.textContent.includes(${JSON.stringify(webId)}) && e.textContent.includes('shared-code'))`
try {
  await promisify(execFile)('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-File', driver, '-TargetPid', String(child.pid)], { windowsHide: true, timeout: 20000 })
  const target = await until(async () => (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find(t => t.type === 'page' && t.url.endsWith('/index.html')), 'native-connected Manager renderer')
  ws = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => { ws.addEventListener('open', resolve, { once: true }); ws.addEventListener('error', reject, { once: true }) })
  await until(() => evaluate('!!window.desktop?.pluginTasks && !!document.querySelector(".app-frame")'), 'Manager preload and renderer')
  mark('Owned Rust tray Settings command opens its genuinely connected packaged Manager')
  await until(async () => {
    const tasks = await list()
    return tasks.length === 3 && tasks.find(t => t.code === 'bootstrap')?.successes === 1 && tasks
  }, 'Rust-triggered Node callback and Manager task list', 60000)
  const bootstrap = (await list()).find(t => t.code === 'bootstrap')
  assert.equal(bootstrap.logs[0].success, true)
  assert.ok(bootstrap.logs[0].durationMs >= 0)
  const storedData = JSON.parse(await readFile(join(managerProfile, 'plugins', 'data', `${nodeId}.json`), 'utf8'))
  assert.equal(storedData['management-probe'], 'bootstrap')
  mark('Rust invokes a real scheduled Node command and Manager reads the persisted execution result')
  await evaluate(`document.querySelector('.sidebar-bottom .nav-item')?.click(); true`)
  await until(() => evaluate(`[...document.querySelectorAll('button')].find(b => b.textContent.trim() === '定时任务')?.click(); !!document.querySelector('.scheduled-tasks')`), 'scheduled tasks Settings category')
  await until(() => evaluate(`document.querySelectorAll('.scheduled-tasks .task-card').length === 3 && !document.querySelector('.scheduled-tasks .inline-error')`), 'all task cards')
  assert.match(await evaluate('document.querySelector(".scheduled-tasks").textContent'), /管理验收 shared-code/)
  assert.match(await evaluate('document.querySelector(".scheduled-tasks").textContent'), /成功 1/)
  await screenshot('定时任务设置验收.png')
  await evaluate(`[...document.querySelectorAll('.scheduled-tasks .task-card')].find(e=>e.textContent.includes('bootstrap')).querySelector('details.task-logs').open=true`)
  const logText = await evaluate(`[...document.querySelectorAll('.scheduled-tasks .task-card')].find(e=>e.textContent.includes('bootstrap')).querySelector('.execution-list').textContent`)
  assert.match(logText, /成功/); assert.match(logText, /耗时 \d+ 毫秒/)
  await evaluate(`new Promise(resolve=>{ const list=[...document.querySelectorAll('.scheduled-tasks .task-card')].find(e=>e.textContent.includes('bootstrap')).querySelector('.execution-list');list.scrollIntoView({block:'center'});requestAnimationFrame(()=>requestAnimationFrame(resolve)); })`)
  await screenshot('定时任务执行日志验收.png')
  mark('Completed task execution log visibly shows success, execution time, and nonnegative duration')
  mark('Settings renders both plugin identities, future tasks, and completed execution statistics')
  await until(() => evaluate(`!!(${findCard})?.querySelector('footer button:not(:disabled)')`), 'delete control readiness')
  await evaluate(`(${findCard}).querySelector('footer button').click()`)
  await until(() => evaluate(`!!document.querySelector('.delete-confirmation-dialog')`), 'delete confirmation')
  await evaluate(`[...document.querySelectorAll('.delete-confirmation-dialog button')].find(b=>b.textContent.trim()==='取消').click()`)
  assert.equal((await list()).length, 3)
  mark('Cancelling deletion leaves every real Rust task intact')
  await evaluate(`(${findCard}).querySelector('footer button').click()`)
  await until(() => evaluate(`!!document.querySelector('.delete-confirmation-dialog button[type="submit"]')`), 'second delete confirmation')
  await evaluate(`document.querySelector('.delete-confirmation-dialog button[type="submit"]').click()`)
  await until(async () => {
    const tasks = await list()
    return tasks.length === 2 && !tasks.some(t => t.pluginId === webId && t.code === 'shared-code')
  }, 'Manager delete reaches Rust')
  const state = JSON.parse(await readFile(scheduleFile, 'utf8'))
  assert.equal(state.tasks.some(t => t.pluginId === webId && t.code === 'shared-code'), false)
  assert.equal(state.tasks.some(t => t.pluginId === nodeId && t.code === 'shared-code'), true)
  await until(() => evaluate(`document.querySelectorAll('.scheduled-tasks .task-card').length === 2 && !document.querySelector('.delete-confirmation-dialog')`), 'updated task cards')
  await screenshot('定时任务删除验收.png')
  mark('Confirmed UI deletion persists in Rust and preserves another plugin task with the identical code')
  report.finalTasks = state.tasks
  report.passed = true
} catch (error) {
  report.error = String(error?.stack ?? error)
  if (ws?.readyState === WebSocket.OPEN) await screenshot('定时任务验收失败.png').catch(() => {})
  throw error
} finally {
  ws?.close()
  // Only this script's new Rust process is stopped. Its KILL_ON_JOB_CLOSE job
  // owns this Manager and descendants; existing user processes are untouched.
  child.kill()
  await writeFile(join(root, 'native-host.log'), log)
  await writeFile(join(delivery, 'schedule-management-verification.json'), JSON.stringify(report, null, 2))
}
