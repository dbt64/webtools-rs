// Real tarball consumers outside the repository; no SDK source copying/linking.
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { resolve, join, isAbsolute } from 'node:path'
import { fileURLToPath } from 'node:url'
import { PluginManager } from '../electron/plugins/plugin-manager.ts'
const repo = fileURLToPath(new URL('..', import.meta.url))
assert.ok(isAbsolute(process.argv[2] ?? ''), 'Supply absolute delivery directory')
const delivery = resolve(process.argv[2])
const version = JSON.parse(await readFile(join(repo, 'plugin-sdk/declarative-v1/package.json'))).version
const artifacts = join(delivery, 'artifacts'), projects = join(delivery, 'projects')
await mkdir(artifacts, { recursive: true }); await mkdir(projects, { recursive: true })
const quote = s => "'" + s.replaceAll("'", "''") + "'"
function run(args, cwd, label) {
  const result = spawnSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command', `& npm.cmd ${args.map(quote).join(' ')}; exit $LASTEXITCODE`],
    { cwd, encoding: 'utf8', windowsHide: true, timeout: 180000, maxBuffer: 10 * 1024 * 1024 })
  if (result.status !== 0) throw Error(`${label}: ${result.error ?? ''}\n${result.stdout}\n${result.stderr}`)
  return result.stdout
}
for (const dir of ['declarative-v1', 'create-webtools-plugin']) run(['pack', '--pack-destination', artifacts, '--silent'], join(repo, 'plugin-sdk', dir), 'pack ' + dir)
const tooling = join(delivery, 'tooling')
await mkdir(tooling, { recursive: true }); await writeFile(join(tooling, 'package.json'), JSON.stringify({ name: 'p6-tooling-consumer', private: true }))
run(['install', '--ignore-scripts', '--no-audit', '--no-fund', join(artifacts, `create-webtools-plugin-${version}.tgz`)], tooling, 'install generator tarball')
const cli = join(tooling, 'node_modules/create-webtools-plugin/bin/create-webtools-plugin.mjs')
const manager = new PluginManager({ userData: join(delivery, 'verification-profile'), hostVersion: '0.1.0', confirm: async () => true,
  externalOpen: async () => {}, clipboardWrite() {}, ai: { complete: async () => ({ text: '' }), getDefaultProviderInfo: async () => ({ providerName: '', model: '' }) } })
await manager.initialize()
const report = { version, developmentNode: process.version, cases: [], passed: false }
try {
  for (const framework of ['vanilla', 'vue', 'react', 'svelte']) for (const runtime of ['web', 'node']) {
    const name = `${framework}-${runtime}`, cwd = join(projects, name)
    console.log('Creating and verifying ' + name)
    const created = spawnSync(process.execPath, [cli, name, '--framework', framework, '--runtime', runtime, '--package-manager', 'npm', '--yes'], { cwd: projects, encoding: 'utf8' })
    assert.equal(created.status, 0, created.stdout + created.stderr)
    run(['install', runtime === 'node' ? '--save-prod' : '--save-dev', '--ignore-scripts', '--no-audit', '--no-fund', join(artifacts, `webtools-plugin-sdk-${version}.tgz`)], cwd, name + ' dependencies')
    run(['run', 'typecheck'], cwd, name + ' typecheck')
    run(['run', 'build'], cwd, name + ' build')
    run(['run', 'validate'], cwd, name + ' validate')
    run(['run', 'pack'], cwd, name + ' pack')
    run(['run', 'inspect'], cwd, name + ' inspect')
    const installed = await manager.install(await readFile(join(cwd, name + '.wtplugin')), manager.session)
    assert.equal(installed.outcome, 'installed', JSON.stringify(installed))
    report.cases.push({ framework, runtime, sdkVersion: JSON.parse(await readFile(join(cwd, 'node_modules/@webtools/plugin-sdk/package.json'))).version,
      checks: ['tarball generator', 'local SDK dependency', 'typecheck', 'build', 'validate', 'pack', 'inspect', 'Host installation validation'], pluginId: installed.plugin.id })
    console.log('PASS ' + name)
  }
  report.passed = true
} catch (error) { report.error = String(error.stack ?? error); throw error }
finally { manager.close(); await writeFile(join(delivery, 'toolchain-verification.json'), JSON.stringify(report, null, 2)) }
