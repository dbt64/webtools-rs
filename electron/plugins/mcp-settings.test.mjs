import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, mkdir, readFile, writeFile, readdir, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { parse } from 'smol-toml'
import { McpSettingsStore } from './mcp-settings.ts'
import { McpAgentConfig } from './mcp-agent-config.ts'

test('MCP settings retain credentials and plugin switches across restart', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'wt-mcp-'))
  try {
    const first = new McpSettingsStore(join(dir, 'settings.json'))
    first.value.enabled = true; first.value.disabledPlugins = ['test.plugin']
    await first.save()
    const second = new McpSettingsStore(join(dir, 'settings.json')); await second.load()
    assert.deepEqual(second.value, first.value)
  } finally { await rm(dir, { recursive: true, force: true }) }
})

test('Agent detection does not write; configuration preserves other TOML values and backs up original bytes', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'wt-agent-'))
  try {
    await mkdir(join(dir, '.codex'))
    const file = join(dir, '.codex', 'config.toml')
    const original = '# my settings\nmodel = "test-model"\n[mcp_servers.other]\nurl = "https://example.test/mcp"\n'
    await writeFile(file, original)
    const agents = new McpAgentConfig(dir)
    assert.equal((await agents.list('http://127.0.0.1:24459/mcp', 'test-token'))[0].configured, false)
    assert.equal((await readdir(join(dir, '.codex'))).length, 1)
    await agents.install('codex', 'http://127.0.0.1:24459/mcp', 'test-token')
    const parsed = parse(await readFile(file, 'utf8'))
    assert.equal(parsed.model, 'test-model')
    assert.equal(parsed.mcp_servers.other.url, 'https://example.test/mcp')
    assert.equal(parsed.mcp_servers.webtools.http_headers.Authorization, 'Bearer test-token')
    const backup = (await readdir(join(dir, '.codex'))).find(name => name.includes('backup'))
    assert.equal(await readFile(join(dir, '.codex', backup), 'utf8'), original)
    assert.equal((await agents.list('http://127.0.0.1:24459/mcp', 'test-token'))[0].configured, true)
  } finally { await rm(dir, { recursive: true, force: true }) }
})

test('JSON merge preserves other servers; malformed config is never replaced', async () => {
  const dir = await mkdtemp(join(tmpdir(), 'wt-agent-'))
  try {
    await mkdir(join(dir, '.cursor'))
    const file = join(dir, '.cursor', 'mcp.json')
    await writeFile(file, JSON.stringify({ preferences: { theme: 'dark' }, mcpServers: { other: { command: 'other' } } }))
    const agents = new McpAgentConfig(dir)
    await agents.install('cursor', 'http://127.0.0.1:24459/mcp', 'test-token')
    const parsed = JSON.parse(await readFile(file, 'utf8'))
    assert.equal(parsed.preferences.theme, 'dark'); assert.equal(parsed.mcpServers.other.command, 'other')
    await writeFile(file, '{invalid')
    await assert.rejects(agents.install('cursor', 'http://127.0.0.1:24459/mcp', 'test-token'))
    assert.equal(await readFile(file, 'utf8'), '{invalid')
    await assert.rejects(agents.install('../arbitrary', 'http://127.0.0.1/mcp', 'token'))
  } finally { await rm(dir, { recursive: true, force: true }) }
})
