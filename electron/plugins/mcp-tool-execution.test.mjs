import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { PluginManager } from './plugin-manager.ts'
import { runtimeV3Manifest, runtimeV3Package } from './fixtures.mjs'
import { McpToolExecution } from './mcp-tool-execution.ts'
async function setup(t, execute) {
  const root = await mkdtemp(join(tmpdir(), 'wt-mcp-exec-'))
  const manager = new PluginManager({ userData: root, hostVersion: '0.1.0', confirm: async () => true })
  t.after(async () => { manager.close(); await rm(root, { recursive: true, force: true }) })
  await manager.initialize()
  const schema = { type: 'object', properties: { text: { type: 'string' } }, required: ['text'], additionalProperties: false }
  const manifest = runtimeV3Manifest({ runtime: { kind: 'node' }, features: [{ id: 'note', title: 'Note', entry: { kind: 'node-command', module: 'note.cjs' }, presentation: { type: 'command' }, requiredCapabilities: [], mcpTool: { inputSchema: schema, outputSchema: schema } }], runtimeFiles: [{ path: 'note.cjs', role: 'node-command', mediaType: 'text/javascript' }] })
  await manager.install(runtimeV3Package(manifest, [{ name: 'note.cjs', data: 'module.exports=()=>({text:"ok"})' }]), manager.session)
  await manager.setEnabled(manifest.id, true, manager.session)
  let allowed = true
  const execution = new McpToolExecution(manager, { execute }, () => allowed)
  t.after(() => execution.close())
  return { manager, execution, id: manifest.id, disableMcp: () => { allowed = false }, request: (callId = 'call-1', input = { text: 'hello' }) => ({ callId, toolName: `${manifest.id}.note`, arguments: input, deadlineUnixMs: Date.now() + 5000 }) }
}
test('MCP uses installed live authority and validates input and output before returning a result', async t => {
  let runs = 0
  const env = await setup(t, async (authority, context, options) => { runs++; assert.equal(authority.isCurrent(), true); assert.equal(context, undefined); return { status: 'completed', mcpResult: options.mcpInput } })
  const ok = await env.execution.execute(env.request())
  assert.equal(ok.status, 'completed'); assert.deepEqual(ok.result.structuredContent, { text: 'hello' })
  const bad = await env.execution.execute(env.request('bad', { text: 4 })); assert.equal(bad.result.isError, true); assert.equal(runs, 1)
  env.disableMcp(); const disabled = await env.execution.execute(env.request('disabled')); assert.equal(disabled.result.isError, true); assert.equal(runs, 1)
})
test('plugin disable and mismatching output are revalidated for every call', async t => {
  let runs = 0; const env = await setup(t, async () => { runs++; return { status: 'completed', mcpResult: { invalid: 1 } } })
  assert.match((await env.execution.execute(env.request())).result.content[0].text, /outputSchema/)
  await env.manager.setEnabled(env.id, false, env.manager.session)
  assert.equal((await env.execution.execute(env.request('disabled'))).result.isError, true); assert.equal(runs, 1)
})
test('duplicate active call shares execution; cancellation retains active work until executor actually finishes', async t => {
  let finish, signal, runs = 0
  const env = await setup(t, async (_a, _c, options) => { runs++; signal = options.signal; return await new Promise(resolve => { finish = resolve }) })
  const first = env.execution.execute(env.request()), duplicate = env.execution.execute(env.request())
  while (!finish) await new Promise(resolve => setImmediate(resolve))
  assert.equal(runs, 1); env.execution.cancel('call-1'); assert.equal(signal.aborted, true); assert.equal(env.execution.activeCalls, 1)
  finish({ status: 'cancelled' }); assert.equal((await first).status, 'cancelled'); assert.equal((await duplicate).status, 'cancelled'); assert.equal(env.execution.activeCalls, 0)
})
test('deadline-expired invocation never starts the plugin', async t => {
  const env = await setup(t, async () => assert.fail('expired call executed'))
  assert.equal((await env.execution.execute({ ...env.request(), deadlineUnixMs: Date.now() - 1 })).status, 'cancelled')
})
