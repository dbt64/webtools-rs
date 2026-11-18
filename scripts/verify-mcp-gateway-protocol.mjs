import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { parseArgs } from 'node:util'
import { request as httpRequest } from 'node:http'
import { Client } from '@modelcontextprotocol/sdk/client/index.js'
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js'

const { values } = parseArgs({ options: { url: { type: 'string' }, token: { type: 'string' }, probe: { type: 'string' } } })
const token = values.token ?? 'isolated-probe-token-12345678901234567890'
let url = values.url, child
const clients = [], checks = []
try {
  if (values.probe) {
    child = spawn(values.probe, [], { windowsHide: true, env: { ...process.env, WT_MCP_PROBE_TOKEN: token }, stdio: ['ignore', 'pipe', 'pipe'] })
    url = await new Promise((resolve, reject) => {
      let text = ''
      const timer = setTimeout(() => reject(Error('Probe did not become ready')), 15_000)
      child.once('error', error => { clearTimeout(timer); reject(error) })
      child.once('exit', code => { clearTimeout(timer); reject(Error(`Probe exited: ${code}`)) })
      child.stderr.on('data', data => process.stderr.write(data))
      child.stdout.on('data', data => { text += data; const line = text.split('\n').find(line => line.startsWith('{')); if (line) { try { const ready = JSON.parse(line); clearTimeout(timer); resolve(ready.url) } catch {} } })
    })
  }
  assert.ok(url, 'Pass --url or --probe')
  const headers = { Authorization: `Bearer ${token}`, Accept: 'application/json, text/event-stream', 'Content-Type': 'application/json', 'MCP-Protocol-Version': '2025-11-25' }
  const post = (body, extra = {}) => fetch(url, { method: 'POST', headers: { ...headers, ...extra }, body: typeof body === 'string' ? body : JSON.stringify(body), signal: AbortSignal.timeout(5000) })
  const mark = check => { checks.push(check); console.log(check) }
  const badAuth = await post({}, { Authorization: 'Bearer invalid' }); assert.equal(badAuth.status, 401); mark('Bearer authentication enforced')
  const origin = await post({}, { Origin: 'https://example.invalid' }); assert.equal(origin.status, 403); mark('Browser Origin rejected')
  const hostStatus = await new Promise((resolve, reject) => {
    // fetch may normalize Host to the target URL; raw HTTP exercises the actual wire header.
    const request = httpRequest(url, { method: 'POST', headers: { ...headers, Host: 'example.invalid' } }, response => { response.resume(); resolve(response.statusCode) })
    request.on('error', reject); request.end('{}')
  })
  assert.equal(hostStatus, 403); mark('Unexpected Host rejected')
  assert.equal((await post('x'.repeat(65537))).status, 413); mark('64 KiB body limit enforced')
  assert.equal((await post('{')).status, 400); mark('Malformed JSON rejected')
  assert.equal((await fetch(url, { headers })).status, 405); mark('No standalone GET stream')
  for (let i = 0; i < 2; i++) {
    const client = new Client({ name: `webtools-probe-${i}`, version: '1.0.0' })
    await client.connect(new StreamableHTTPClientTransport(new URL(url), { requestInit: { headers: { Authorization: `Bearer ${token}` } } }))
    clients.push(client)
    assert.ok((await client.listTools()).tools.some(tool => tool.name === 'probe.echo'))
    await client.ping()
  }
  mark('TS SDK 1.32.2 initializes, lists tools and pings without sessions')
  const results = await Promise.all(clients.map((client, i) => client.callTool({ name: 'probe.echo', arguments: { text: `client-${i}` } })))
  results.forEach((result, i) => assert.deepEqual(result.structuredContent, { text: `client-${i}` }))
  mark('Two clients with independent request sequences receive their own results')
  const req = (text, id) => post({ jsonrpc: '2.0', id, method: 'tools/call', params: { name: 'probe.echo', arguments: { text } } })
  const a = req('first', 42), b = req('second', 42)
  const cancel = await post({ jsonrpc: '2.0', method: 'notifications/cancelled', params: { requestId: 42, reason: 'unassociated notification' } })
  assert.equal(cancel.status, 202)
  const both = await Promise.all([a, b]); const contents = await Promise.all(both.map(response => response.json()))
  assert.deepEqual(contents.map(value => value.result.structuredContent.text), ['first', 'second'])
  mark('Unassociated cancellation cannot cancel another stateless request with the same ID')
  console.log(JSON.stringify({ passed: checks.length, sdk: '1.32.2', checks }))
} finally {
  await Promise.allSettled(clients.map(client => client.close()))
  child?.kill()
}
