import { randomBytes } from 'node:crypto'
import { mkdir, readFile, writeFile, rename } from 'node:fs/promises'
import { dirname } from 'node:path'

export interface McpSettings { enabled: boolean; port: number; token: string; disabledPlugins: string[] }
export class McpSettingsStore {
  value: McpSettings = { enabled: false, port: 24459, token: randomBytes(32).toString('base64url'), disabledPlugins: [] }
  constructor(private readonly path: string) {}
  async load() {
    try {
      const data = JSON.parse(await readFile(this.path, 'utf8'))
      if (typeof data.enabled !== 'boolean' || !Number.isInteger(data.port) || data.port < 1024 || data.port > 65535
        || typeof data.token !== 'string' || !/^[A-Za-z0-9_-]{32,128}$/.test(data.token)
        || !Array.isArray(data.disabledPlugins) || !data.disabledPlugins.every((id: unknown) => typeof id === 'string')) throw new Error('Invalid MCP settings')
      this.value = data
    } catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error }
  }
  async save() {
    await mkdir(dirname(this.path), { recursive: true })
    await writeFile(this.path + '.tmp', JSON.stringify(this.value, null, 2), { mode: 0o600 })
    await rename(this.path + '.tmp', this.path)
  }
}
