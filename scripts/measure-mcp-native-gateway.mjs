import assert from 'node:assert/strict'
import { parseArgs } from 'node:util'
import { writeFile, mkdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { fixture, processGroup } from './mcp-native-test-support.mjs'
const { values } = parseArgs({ options: { delivery: { type: 'string' }, 'baseline-delivery': { type: 'string' }, 'plugin-package': { type: 'string' }, 'window-seconds': { type: 'string', default: '5' } } })
assert.ok(values.delivery && values['baseline-delivery'] && values['plugin-package'])
const seconds = Number(values['window-seconds']); assert.ok(Number.isInteger(seconds) && seconds >= 3 && seconds <= 60)
const median = values => values.toSorted((a, b) => a - b)[Math.floor(values.length / 2)]
const report = { windowSeconds: seconds, windowsPerScenario: 3, units: { memory: 'bytes', cpu: 'percentage of one logical core' }, scenarios: [] }
try {
  for (const scenario of [
    { name: 'native-mcp-off', delivery: values.delivery, enabled: false },
    { name: 'old-electron-mcp-idle', delivery: values['baseline-delivery'], old: true },
    { name: 'new-rust-mcp-idle', delivery: values.delivery },
  ]) {
    const env = await fixture(scenario.delivery, values['plugin-package'], scenario)
    const result = { name: scenario.name, root: env.root, windows: [], samples: [] }; report.scenarios.push(result)
    try {
      await sleep(5000)
      for (let window = 0; window < 3; window++) {
        const start = await processGroup(env.child.pid), started = Date.now(), samples = []
        while (Date.now() - started < seconds * 1000) {
          const group = await processGroup(env.child.pid)
          const sample = { atMs: Date.now(), processes: group.length, electron: group.filter(p => p.name.toLowerCase() === 'webtools.exe').length, privateBytes: group.reduce((n, p) => n + p.privateBytes, 0), workingSet: group.reduce((n, p) => n + p.workingSet, 0), handles: group.reduce((n, p) => n + p.handles, 0) }
          samples.push(sample); result.samples.push(sample); await sleep(500)
        }
        const end = await processGroup(env.child.pid), elapsed = (Date.now() - started) / 1000
        result.windows.push({ privateMedianBytes: median(samples.map(s => s.privateBytes)), workingSetMedianBytes: median(samples.map(s => s.workingSet)), cpuOneCorePercent: Math.max(0, end.reduce((n, p) => n + p.cpu, 0) - start.reduce((n, p) => n + p.cpu, 0)) / elapsed * 100 })
      }
      if (scenario.name !== 'old-electron-mcp-idle') assert.ok(result.samples.every(s => s.electron === 0), `${scenario.name}: unexpected Electron`)
      console.log(`${scenario.name}: ${JSON.stringify(result.windows)}`)
    } finally { await env.stop() }
  }
  const old = report.scenarios[1].windows.map(w => w.privateMedianBytes), current = report.scenarios[2].windows.map(w => w.privateMedianBytes)
  report.netMemoryImprovementConfirmed = Math.max(...current) < Math.min(...old)
  report.privateSavingsMedianBytes = median(old) - median(current)
  assert.ok(report.netMemoryImprovementConfirmed, 'Net idle memory benefit did not exceed observed window variation')
} catch (error) { report.error = error.stack; throw error }
finally {
  const output = join(resolve(values.delivery), 'verification'); await mkdir(output, { recursive: true }); await writeFile(join(output, 'memory-comparison.json'), JSON.stringify(report, null, 2)); console.log(`Resource evidence: ${output}`)
}
