import test from 'node:test'
import assert from 'node:assert/strict'
import { toMcpPanelState } from './plugin-mcp-view-model.ts'
test('saved enablement and listener readiness stay separate; port error can retry', () => {
  const status = { enabled: true, running: false, phase: 'error', catalogReady: true, activeCalls: 0, error: { code: 'MCP_START_FAILED', message: '端口 25555 被占用' } }
  const ui = toMcpPanelState(status, '')
  assert.equal(ui.enabled, true); assert.equal(ui.canConnect, false); assert.equal(ui.canRetry, true); assert.equal(ui.label, status.error.message)
})
test('initial load failure is unavailable rather than a successful disabled status', () => {
  assert.equal(toMcpPanelState(null, '无法连接服务').label, '无法连接服务')
  assert.equal(toMcpPanelState(null, '').label, '正在读取本地 MCP 服务状态…')
})
test('listener remains running while catalog or execution state changes', () => {
  const status = { enabled: true, running: true, phase: 'running', catalogReady: false, activeCalls: 1, url: 'http://127.0.0.1:24459/mcp', token: 'token' }
  const ui = toMcpPanelState(status, '')
  assert.equal(ui.canConnect, true); assert.match(ui.catalogLabel, /同步/); assert.match(ui.label, /服务运行中/)
})
