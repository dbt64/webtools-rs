import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { ScheduledTaskExecution } from './scheduled-task-execution.ts'
import { PluginManager } from './plugin-manager.ts'
import { runtimeV3Manifest, runtimeV3Package } from './fixtures.mjs'

function deferred() { let resolve; const promise = new Promise(done => { resolve = done }); return { promise, resolve } }

test('authorization queued behind a real install confirmation expires and can never execute late or overlap a replacement trigger', async t => {
  const root = await mkdtemp(join(tmpdir(), 'wt-p5-deadline-')); t.after(() => rm(root, { recursive: true, force: true }))
  const entered = deferred(); const release = deferred()
  const deps = { userData: root, hostVersion: '0.1.0', confirm: async () => true, externalOpen: async () => {}, clipboardWrite() {},
    ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: 'test', model: 'test' }) } }
  const manager = new PluginManager(deps); await manager.initialize(); t.after(() => manager.close())
  const base = runtimeV3Manifest(); const grants = ['plugin.tasks']
  const target = runtimeV3Manifest({ requestedCapabilities: grants, features: [{ ...base.features[0], requiredCapabilities: grants }] })
  await manager.install(runtimeV3Package(target), manager.session); await manager.setEnabled(target.id, true, manager.session)
  deps.confirm = async () => { entered.resolve(); return release.promise }
  const installation = manager.install(runtimeV3Package(runtimeV3Manifest({ id: 'org.webtools.unrelated.install' })), manager.session)
  await entered.promise
  let executions = 0; let inFlight = 0; let maxConcurrent = 0
  const dispatch = async (_authority, signal) => { assert.equal(signal.aborted, false); executions++; inFlight++; maxConcurrent = Math.max(maxConcurrent, inFlight); await delay(5); inFlight-- }
  const authorize = callback => manager.authorizeWebFeature({ pluginId: target.id, featureId: 'hello' }, null, callback)
  const executor = new ScheduledTaskExecution(500)
  await assert.rejects(executor.execute('plugin:stable-code', authorize, dispatch), error => error.code === 'SCHEDULE_TIMEOUT')
  assert.equal(executions, 0)
  // The old authorization is still in the actual PluginManager queue. A new
  // trigger can queue, but releasing the prompt must execute only this new one.
  const replacement = executor.execute('plugin:stable-code', authorize, dispatch)
  release.resolve(false); await installation; await replacement
  await delay(10)
  assert.equal(executions, 1); assert.equal(maxConcurrent, 1)
})

test('active timeout aborts work and keeps same-code execution locked until owned teardown finishes', async () => {
  const started = deferred(); const aborted = deferred(); const cleanup = deferred()
  let revoked = 0
  const authority = { revoke() { revoked++ } }
  const authorize = async callback => ({ completion: callback(authority) })
  const executor = new ScheduledTaskExecution(25)
  const pending = executor.execute('plugin:stable-code', authorize, async (_authority, signal) => {
    started.resolve()
    await new Promise(resolve => signal.addEventListener('abort', () => { aborted.resolve(); resolve() }, { once: true }))
    await cleanup.promise
  })
  const rejection = assert.rejects(pending, error => error.code === 'SCHEDULE_TIMEOUT')
  await started.promise; await aborted.promise
  await assert.rejects(executor.execute('plugin:stable-code', authorize, async () => assert.fail('must not overlap teardown')), /already running/)
  cleanup.resolve(); await rejection
  assert.ok(revoked > 0)
  await executor.execute('plugin:stable-code', authorize, async () => {})
})
