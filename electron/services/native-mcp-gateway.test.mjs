import test from 'node:test'
import assert from 'node:assert/strict'
import { NativeMcpGateway } from './native-mcp-gateway.ts'
test('tool publishing requires negotiated capability, serializes revisions and rejects failed persistence', async () => {
  let supported = false; const sent = []
  const client = { get supportsMcpGateway() { return supported }, request: async (type, payload) => { sent.push([type, payload]); if (payload.revision === 2) throw Error('disk unavailable'); return {} } }
  const gateway = new NativeMcpGateway(client, { execute() {}, cancel() {}, close() {} })
  await gateway.publishTools([]); assert.equal(sent.length, 0)
  supported = true; await gateway.publishTools([]); await assert.rejects(gateway.publishTools([]), /disk unavailable/)
  assert.deepEqual(sent.map(item => item[1].revision), [1, 2]); assert.equal(sent[0][0], 'mcp-tools-update-v1')
})
test('only MCP messages are handled and closed generations cannot execute delayed commands', async () => {
  let calls = 0, cancelled
  const gateway = new NativeMcpGateway({ supportsMcpGateway: true }, { execute: async request => { calls++; return { callId: request.callId, status: 'cancelled' } }, cancel: id => { cancelled = id }, close() {} })
  assert.equal(await gateway.handleCommand({ type: 'open-page' }), undefined)
  assert.equal((await gateway.handleCommand({ type: 'mcp-call-v1', payload: { callId: 'one', toolName: 'org.test.note', arguments: {}, deadlineUnixMs: Date.now() + 5000 } })).result.callId, 'one')
  await gateway.handleCommand({ type: 'mcp-call-cancel-v1', payload: { callId: 'one' } }); assert.equal(cancelled, 'one')
  gateway.close(); await assert.rejects(gateway.handleCommand({ type: 'mcp-call-v1', payload: {} })); assert.equal(calls, 1)
})
