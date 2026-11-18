import test from 'node:test'
import assert from 'node:assert/strict'
import { parseMcpGatewayStatus } from './plugin-mcp-contracts.ts'
test('enabled preference survives a failed listener without valid connection credentials', () => {
  const status = { enabled: true, running: false, phase: 'error', catalogReady: false, activeCalls: 0, error: { code: 'MCP_START_FAILED', message: 'port occupied' } }
  assert.deepEqual(parseMcpGatewayStatus(status), status)
  assert.equal(parseMcpGatewayStatus({ ...status, running: true }), null)
})
test('running status requires a loopback URL and existing token; invalid counts and phases are rejected', () => {
  const status = { enabled: true, running: true, phase: 'running', catalogReady: true, activeCalls: 1, url: 'http://127.0.0.1:24459/mcp', token: 'test-token-123456789012345678901234' }
  assert.deepEqual(parseMcpGatewayStatus(status), status)
  for (const changed of [{ url: 'https://evil.test/mcp' }, { activeCalls: -1 }, { activeCalls: 9 }, { phase: 'disabled' }, { token: '' }]) assert.equal(parseMcpGatewayStatus({ ...status, ...changed }), null)
})
