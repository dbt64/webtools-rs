import { readFile, writeFile, copyFile, rename, mkdir, stat } from 'node:fs/promises'
import { join, dirname } from 'node:path'
import { homedir } from 'node:os'
import { parse, stringify } from 'smol-toml'

export interface McpAgent { id: string; name: string; path: string; configured: boolean }
export class McpAgentConfig {
  constructor(private readonly home = homedir()) {}
  private candidates() {
    return [
      { id: 'codex', name: 'Codex / ChatGPT', path: join(this.home, '.codex', 'config.toml'), format: 'toml' },
      { id: 'cursor', name: 'Cursor', path: join(this.home, '.cursor', 'mcp.json'), format: 'json' },
      { id: 'claude-code', name: 'Claude Code', path: join(this.home, '.claude.json'), format: 'json' },
    ]
  }
  async list(url: string, token: string): Promise<McpAgent[]> {
    const found: McpAgent[] = []
    for (const agent of this.candidates()) {
      try {
        // Only offer clients with an existing config or client directory. Detection never creates files.
        await stat(agent.id === 'claude-code' ? agent.path : dirname(agent.path))
        let configured = false
        try {
          const text = await readFile(agent.path, 'utf8')
          const data: any = agent.format === 'toml' ? parse(text) : JSON.parse(text)
          const entry = (agent.format === 'toml' ? data.mcp_servers : data.mcpServers)?.webtools
          configured = entry?.url === url && (entry.http_headers ?? entry.headers)?.Authorization === `Bearer ${token}`
        } catch { /* An invalid config remains visible; installation reports the parse error without overwriting it. */ }
        found.push({ id: agent.id, name: agent.name, path: agent.path, configured })
      } catch { /* Client is not installed. */ }
    }
    return found
  }
  async install(id: string, url: string, token: string) {
    const agent = this.candidates().find(item => item.id === id)
    if (!agent || !(await this.list(url, token)).some(item => item.id === id)) throw new Error('未检测到该 Agent。')
    let original: string | undefined
    try { original = await readFile(agent.path, 'utf8') }
    catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error }
    const data: any = original === undefined ? {} : agent.format === 'toml' ? parse(original) : JSON.parse(original)
    const key = agent.format === 'toml' ? 'mcp_servers' : 'mcpServers'
    if (data[key] !== undefined && (typeof data[key] !== 'object' || data[key] === null || Array.isArray(data[key]))) throw new Error('Agent MCP 配置格式无效。')
    data[key] ??= {}
    data[key].webtools = agent.format === 'toml'
      ? { url, http_headers: { Authorization: `Bearer ${token}` }, enabled: true }
      : { type: 'http', url, headers: { Authorization: `Bearer ${token}` } }
    const text = agent.format === 'toml' ? stringify(data) : JSON.stringify(data, null, 2) + '\n'
    // Preserve all other config values, and retain the original bytes as a backup before replacing.
    if (original !== undefined) await copyFile(agent.path, agent.path + `.webtools-backup-${Date.now()}`)
    await mkdir(dirname(agent.path), { recursive: true })
    await writeFile(agent.path + '.webtools.tmp', text, { mode: 0o600 })
    await rename(agent.path + '.webtools.tmp', agent.path)
  }
}
