import assert from 'node:assert/strict'
import { parseArgs } from 'node:util'
import { writeFile, readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { performance } from 'node:perf_hooks'
import { setTimeout as sleep } from 'node:timers/promises'
import { fixture, processGroup, killOwnedEngine, until, testTool } from './mcp-native-test-support.mjs'
const { values } = parseArgs({ options: { delivery: { type: 'string' }, 'plugin-package': { type: 'string' }, cycles: { type: 'string', default: '2' }, 'cooldown-ms': { type: 'string' }, 'no-cache': { type: 'boolean', default: false } } })
assert.ok(values.delivery && values['plugin-package'], 'Pass --delivery and --plugin-package')
const cycles = Number(values.cycles); assert.ok(Number.isInteger(cycles) && cycles >= 1 && cycles <= 50)
const cooldownMs = values['cooldown-ms'] === undefined ? undefined : Number(values['cooldown-ms'])
const checks = [], timings = [], warmTimings = [], coldIdleSamples = [], report = { checks, timings, warmTimings, coldIdleSamples, cycles, cooldownMs: cooldownMs ?? 15000 }
const env = await fixture(values.delivery, values['plugin-package'], { cache: !values['no-cache'], cooldownMs })
const mark = check => { checks.push(check); console.log(check) }
let lastGroup = []
const engines = async () => (lastGroup = await processGroup(env.child.pid)).filter(p => p.name.toLowerCase() === 'webtools.exe')
try {
  assert.equal((await engines()).length, 0); mark('Enabled MCP listener starts without Electron')
  const client = await env.client(); const list = await client.listTools(); await client.ping()
  assert.ok(list.tools.some(tool => tool.name === `${env.plugin}.agent-note`)); assert.ok(list.tools.some(tool => tool.name === testTool))
  if (!values['no-cache']) assert.equal((await engines()).length, 0)
  else await until(async () => !(await engines()).length, 'first-sync engine exit', 30_000)
  mark(values['no-cache'] ? 'Missing snapshot synchronized once and released its temporary owner' : 'Initialize, list and ping keep Electron at zero')
  for (let cycle = 0; cycle < cycles; cycle++) {
    await until(async () => !(await engines()).length, 'cold engine exit', 35_000)
    coldIdleSamples.push({ cycle, processes: lastGroup })
    const launchesBefore = (env.log().match(/host: spawned=/g) ?? []).length
    const start = performance.now()
    const result = await client.callTool({ name: `${env.plugin}.agent-note`, arguments: { text: `cold-${cycle}` } })
    assert.deepEqual(result.structuredContent, { text: `cold-${cycle}` }, JSON.stringify(result)); timings.push(performance.now() - start)
    const warmStart = performance.now()
    const warm = await client.callTool({ name: `${env.plugin}.agent-note`, arguments: {} }); assert.deepEqual(warm.structuredContent, { text: `cold-${cycle}` }); warmTimings.push(performance.now() - warmStart)
    assert.equal((env.log().match(/host: spawned=/g) ?? []).length, launchesBefore + 1, 'cold and immediate warm calls share exactly one managed engine')
    if ((cycle + 1) % 10 === 0 || cycle === cycles - 1) console.log(`Cold/warm cycles: ${cycle + 1}/${cycles}`)
  }
  await until(async () => !(await engines()).length, 'final cooldown exit', 35_000); mark('Cold calls share one engine, warm calls reuse it, cooldown releases the process tree')
  const parallel = Array.from({ length: 8 }, () => client.callTool({ name: testTool, arguments: { delayMs: 4000 } }))
  await sleep(500)
  const busy = await client.callTool({ name: testTool, arguments: {} })
  assert.equal(busy.isError, true); assert.match(busy.content[0].text, /繁忙/)
  const completed = await Promise.all(parallel); assert.ok(completed.every(result => !result.isError))
  mark('Eight concurrent calls complete; the ninth is rejected without another engine')
  const abort = new AbortController()
  const cancelled = client.callTool({ name: testTool, arguments: { text: 'cancel-once', delayMs: 5000 } }, undefined, { signal: abort.signal })
  const observedCancellation = cancelled.then(() => false, () => true)
  await until(async () => JSON.parse(await readFile(join(env.profile, 'plugins', 'data', 'org.webtools.acceptance.mcp.json'), 'utf8')).text === 'cancel-once', 'cancelled call started')
  abort.abort(); assert.equal(await observedCancellation, true)
  await sleep(5200)
  mark('Client cancellation stops waiting and later execution cleanup does not duplicate the operation')
  // Keep one response in flight, then prove a crashed execution is not replayed.
  const before = await client.callTool({ name: testTool, arguments: {} }); const count = before.structuredContent.count
  const crash = client.callTool({ name: testTool, arguments: { text: 'crash-once', delayMs: 10000 } })
  await until(async () => JSON.parse(await readFile(join(env.profile, 'plugins', 'data', 'org.webtools.acceptance.mcp.json'), 'utf8')).text === 'crash-once', 'side effect committed before crash')
  await killOwnedEngine(env.child.pid)
  const unknown = await crash; assert.equal(unknown.isError, true); assert.match(unknown.content[0].text, /结果未知/)
  const after = await client.callTool({ name: testTool, arguments: {} }); assert.equal(after.structuredContent.count, count + 1)
  mark('Execution crash returns outcome unknown, keeps MCP listening and does not replay the side effect')
  const invalid = await client.callTool({ name: `${env.plugin}.agent-note`, arguments: { text: 7 } }); assert.equal(invalid.isError, true); mark('Electron retains input schema validation')
  await until(async () => !(await engines()).length, 'recovered engine cooldown', 35_000)
  const saved = JSON.parse(await readFile(join(env.profile, 'plugin-mcp-settings.json'), 'utf8')); assert.deepEqual(saved, env.settings)
  mark('Original token, port, enabled preference and plugin switches remain unchanged')
  report.processes = await processGroup(env.child.pid)
  const sorted = timings.toSorted((a, b) => a - b); report.coldP50Ms = sorted[Math.floor(sorted.length * .5)]; report.coldP95Ms = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * .95))]
  const warmSorted = warmTimings.toSorted((a,b)=>a-b); report.warmP50Ms=warmSorted[Math.floor(warmSorted.length*.5)]; report.warmP95Ms=warmSorted[Math.min(warmSorted.length-1,Math.floor(warmSorted.length*.95))]
} catch (error) { report.error = error.stack; throw error }
finally { await env.stop(); await writeFile(join(env.root, 'lifecycle-report.json'), JSON.stringify(report, null, 2)); console.log(`Evidence: ${env.root}`) }
