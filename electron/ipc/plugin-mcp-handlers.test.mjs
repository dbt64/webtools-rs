import test from 'node:test'
import assert from 'node:assert/strict'
import { registerPluginMcpIpc } from './plugin-mcp-handlers.ts'
import { IPC_CHANNELS } from '../../src/shared/ipc.ts'
function setup(request) {
  const handlers = new Map(), manager = {}
  registerPluginMcpIpc({ handle: (channel, handler) => handlers.set(channel, handler) }, {
    isManagerMainFrame: event => event === manager,
    request, listTools: () => [{ pluginId: 'org.test.note', featureId: 'note', title: 'Note', description: '' }],
    agents: { list: async () => [], install: async () => assert.fail('unexpected config write') },
  })
  return { handlers, manager }
}
const state = { enabled: true, running: false, phase: 'error', catalogReady: true, activeCalls: 0, error: { code: 'MCP_START_FAILED', message: 'port 25555 occupied' } }
test('MCP management is Manager-main-frame only and preserves backend error state', async () => {
  const { handlers, manager } = setup(async () => state)
  assert.throws(() => handlers.get(IPC_CHANNELS.pluginMcpGet)({}), /UNAUTHORIZED/)
  assert.deepEqual(await handlers.get(IPC_CHANNELS.pluginMcpGet)(manager), { ok: true, data: state })
})
test('retry sends the same true preference; disconnected native service returns failure', async () => {
  const calls = []; const { handlers, manager } = setup(async (type, payload) => { calls.push([type, payload]); throw Error('无法连接本地 MCP 服务。') })
  const result = await handlers.get(IPC_CHANNELS.pluginMcpSetEnabled)(manager, true)
  assert.equal(result.ok, false); assert.deepEqual(calls, [['mcp-admin-set-enabled-v1', { enabled: true }]])
  assert.equal((await handlers.get(IPC_CHANNELS.pluginMcpConfigureAgent)(manager, 'codex')).ok, false)
})
test('per-plugin tool switch retains the installed plugin and rejects unknown identity', async () => {
  const calls = []; const { handlers, manager } = setup(async (type, payload) => { calls.push([type, payload]); return null })
  assert.deepEqual(await handlers.get(IPC_CHANNELS.pluginMcpPluginEnabled)(manager, 'org.test.note', false), { ok: true, data: null })
  assert.equal((await handlers.get(IPC_CHANNELS.pluginMcpPluginEnabled)(manager, 'unknown', false)).ok, false)
  assert.equal(calls.length, 1); assert.equal(calls[0][0], 'mcp-admin-set-plugin-enabled-v1')
})
