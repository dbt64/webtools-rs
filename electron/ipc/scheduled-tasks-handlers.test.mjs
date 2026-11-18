import test from 'node:test'
import assert from 'node:assert/strict'
import { registerScheduledTasksIpc } from './scheduled-tasks-handlers.ts'
import { IPC_CHANNELS } from '../../src/shared/ipc.ts'

function fixture() {
  const handlers = new Map(), calls = []
  const owner = {}
  registerScheduledTasksIpc({ handle: (k, h) => handlers.set(k, h) }, {
    isManagerMainFrame: e => e === owner,
    list: async () => [{ pluginId: 'test.plugin', code: 'once' }],
    remove: async (p, c) => { calls.push([p, c]); return true },
  })
  return { handlers, calls, owner }
}
test('task administration rejects plugin windows and subframes before accessing host', async () => {
  const { handlers, calls } = fixture()
  for (const channel of [IPC_CHANNELS.pluginTasksList, IPC_CHANNELS.pluginTasksRemove]) {
    assert.throws(() => handlers.get(channel)({}, 'test.plugin', 'once'), /UNAUTHORIZED_MANAGER_IPC/)
  }
  assert.deepEqual(calls, [])
})
test('manager can list all tasks and delete an exact plugin/code pair; malformed requests do not reach host', async () => {
  const { handlers, calls, owner } = fixture()
  assert.equal((await handlers.get(IPC_CHANNELS.pluginTasksList)(owner)).data[0].pluginId, 'test.plugin')
  for (const args of [[null, 'once'], ['test.plugin', '../once'], ['test.plugin', ''], ['test.plugin', 'x'.repeat(65)]]) {
    assert.equal((await handlers.get(IPC_CHANNELS.pluginTasksRemove)(owner, ...args)).ok, false)
  }
  assert.deepEqual(calls, [])
  assert.deepEqual(await handlers.get(IPC_CHANNELS.pluginTasksRemove)(owner, 'test.plugin', 'once'), { ok: true, data: true })
  assert.deepEqual(calls, [['test.plugin', 'once']])
})
test('disconnected host reports failure rather than an empty successful list', async () => {
  const handlers = new Map()
  registerScheduledTasksIpc({ handle: (k,h) => handlers.set(k,h) }, {
    isManagerMainFrame: () => true,
    list: async () => { throw new Error('托盘未连接') }, remove: async () => false,
  })
  const result = await handlers.get(IPC_CHANNELS.pluginTasksList)({})
  assert.equal(result.ok, false); assert.equal(result.error.message, '托盘未连接')
})
