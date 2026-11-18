import type { PluginJson, PluginSettingValue } from '../../src/shared/plugin-contracts.ts'
import { LIMITS, opaqueKey, parseStrictJson, record, validPluginId, validateSettingValue, type ValidatedManifest } from './manifest.ts'
import { ManagedFs, SerialQueue, isMissing } from './managed-fs.ts'
import { fail } from './errors.ts'
import { PluginConfigurationStore, runtimeConfigPaths } from './plugin-configuration.ts'

export class PluginStore {
  private static readonly maxFileBytes = 4 * 1024 * 1024
  private static readonly maxFileCount = 256
  private static readonly maxFilesBytes = 32 * 1024 * 1024
  private static readonly maxBackupBytes = 8 * 1024 * 1024
  private readonly writes = new SerialQueue()
  readonly fs: ManagedFs
  readonly configuration: PluginConfigurationStore
  constructor(fs: ManagedFs) { this.fs = fs; this.configuration = new PluginConfigurationStore(fs, this.writes) }
  private validateDocument(values: Record<string, PluginJson>): void {
    const json = JSON.stringify(values)
    if (Object.keys(values).length > LIMITS.keys || Buffer.byteLength(json) > LIMITS.data) fail('STORAGE_LIMIT')
    // The on-disk envelope counts towards the reader's depth budget.
    try { parseStrictJson(json) } catch { fail('STORAGE_LIMIT') }
  }
  private logical(scope: 'data' | 'config', id: string): string { if (!validPluginId(id)) fail('INVALID_INPUT'); return `${scope}/${id}.json` }
  private fileRoot(id: string): string { if (!validPluginId(id)) fail('INVALID_INPUT'); return `data/${id}/files` }
  private fileLogicalPath(id: string, value: unknown): string {
    if (typeof value !== 'string' || !value || Buffer.byteLength(value, 'utf8') > 512 || value.startsWith('/') || value.endsWith('/')
      || /[\\:\x00-\x1f\x7f<>"|?*]/u.test(value)) fail('INVALID_INPUT')
    const parts = value.split('/')
    if (parts.length > 12 || parts.some(part => !part || part === '.' || part === '..' || part.length > 180 || /[. ]$/u.test(part)
      || /^(?:con|prn|aux|nul|com[1-9¹²³]|lpt[1-9¹²³])(?:\.|$)/iu.test(part))) fail('INVALID_INPUT')
    return `${this.fileRoot(id)}/${value}`
  }
  private filePrefix(value: unknown): string {
    if (typeof value !== 'string' || Buffer.byteLength(value, 'utf8') > 512 || value.startsWith('/') || value.includes('\\')
      || /[\x00-\x1f\x7f:\\<>"|?*]/u.test(value)) fail('INVALID_INPUT')
    const parts = value.replace(/\/$/u, '').split('/')
    if (value && parts.some(part => !part || part === '.' || part === '..' || /[. ]$/u.test(part))) fail('INVALID_INPUT')
    return value
  }
  private async snapshotStoredData(id: string): Promise<{ data: Buffer | null; files: Array<{ path: string; bytes: Buffer }> }> {
    let data: Buffer | null
    try { data = await this.fs.read(this.logical('data', id), PluginStore.maxBackupBytes) }
    catch (error) { if (isMissing(error)) data = null; else throw error }

    const listed = await this.fs.listFiles(this.fileRoot(id))
    if (listed.length > PluginStore.maxFileCount) fail('STORAGE_LIMIT')
    let totalBytes = 0
    const files: Array<{ path: string; bytes: Buffer }> = []
    for (const file of listed) {
      this.fileLogicalPath(id, file.path)
      totalBytes += file.size
      if (file.size > PluginStore.maxFileBytes || totalBytes > PluginStore.maxFilesBytes) fail('STORAGE_LIMIT')
      const bytes = await this.fs.read(`${this.fileRoot(id)}/${file.path}`, PluginStore.maxFileBytes)
      if (bytes.length !== file.size) fail('STORAGE_INVALID')
      files.push({ path: file.path, bytes })
    }
    return { data, files }
  }
  private async restoreStoredData(id: string, snapshot: { data: Buffer | null; files: Array<{ path: string; bytes: Buffer }> }): Promise<void> {
    await this.fs.remove(await this.fs.path(this.fileRoot(id)))
    for (const file of snapshot.files) {
      const logical = this.fileLogicalPath(id, file.path)
      await this.fs.mkdir(logical.slice(0, logical.lastIndexOf('/')))
      await this.fs.atomicBytes(logical, file.bytes)
    }
    const dataPath = this.logical('data', id)
    if (snapshot.data === null) await this.fs.remove(await this.fs.path(dataPath))
    else await this.fs.atomicBytes(dataPath, snapshot.data)
  }
  private decodeBase64(value: unknown): Buffer {
    if (typeof value !== 'string' || value.length > Math.ceil(PluginStore.maxFileBytes / 3) * 4 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/u.test(value)) fail('INVALID_INPUT')
    const bytes = Buffer.from(value, 'base64')
    if (bytes.length > PluginStore.maxFileBytes || bytes.toString('base64') !== value) fail('STORAGE_LIMIT')
    return bytes
  }
  private async readValues(scope: 'data' | 'config', id: string): Promise<Record<string, PluginJson>> {
    try {
      const bytes = await this.fs.read(this.logical(scope, id), LIMITS.data)
      const data = parseStrictJson(new TextDecoder('utf-8', { fatal: true }).decode(bytes))
      if (!data || typeof data !== 'object' || Array.isArray(data)) fail('STORAGE_INVALID')
      const values = data as Record<string, PluginJson>
      if (Object.keys(values).length > LIMITS.keys) fail('STORAGE_INVALID')
      for (const [key, value] of Object.entries(values)) { opaqueKey(key); this.validateJson(value) }
      return values
    } catch (error) { if (isMissing(error)) return {}; fail('STORAGE_INVALID') }
  }
  validateJson(value: unknown): PluginJson {
    let json: string
    try { json = JSON.stringify(value); if (typeof json !== 'string' || Buffer.byteLength(json) > LIMITS.value) fail('STORAGE_LIMIT') } catch { fail('STORAGE_LIMIT') }
    // Round trip excludes non-JSON values, cycles/prototypes and excessive nesting.
    const result = parseStrictJson(json)
    const check = (input: unknown): void => {
      if (input === null || typeof input === 'string' || typeof input === 'boolean') return
      if (typeof input === 'number' && Number.isFinite(input)) return
      if (Array.isArray(input)) { for (const item of input) check(item); return }
      if (input && typeof input === 'object' && [Object.prototype, null].includes(Object.getPrototypeOf(input))) { for (const item of Object.values(input)) check(item); return }
      fail('INVALID_INPUT')
    }
    check(value); return result
  }
  async readConfig(manifest: ValidatedManifest): Promise<Record<string, PluginSettingValue>> {
    const saved = await this.readValues('config', manifest.id); const settings = manifest.settings
    try {
      record(saved, [], settings.map(setting => setting.key))
      const result: Record<string, PluginSettingValue> = {}
      for (const setting of settings) result[setting.key] = validateSettingValue(setting, Object.hasOwn(saved, setting.key) ? saved[setting.key] : setting.default)
      return result
    } catch { fail('CONFIG_INCOMPATIBLE') }
  }
  async writeConfig(manifest: ValidatedManifest, key: string, value: unknown, mayCommit: () => boolean): Promise<void> {
    return this.writes.run(async () => {
      const setting = manifest.settings.find(item => item.key === key); if (!setting) fail('INVALID_INPUT')
      const parsed = validateSettingValue(setting, value); await this.readConfig(manifest)
      const saved = await this.readValues('config', manifest.id); saved[key] = parsed
      this.validateDocument(saved)
      await this.fs.atomicJson(this.logical('config', manifest.id), saved, mayCommit)
    })
  }
  async readData(id: string, key: string): Promise<PluginJson> { opaqueKey(key); const data = await this.readValues('data', id); return data[key] ?? null }
  async listDataKeys(id: string, prefix: string): Promise<string[]> {
    const data = await this.readValues('data', id)
    return Object.keys(data).filter(key => key.startsWith(prefix)).sort()
  }
  async writeData(id: string, key: string, value: unknown, mayCommit: () => boolean): Promise<void> {
    opaqueKey(key); const parsed = this.validateJson(value)
    return this.writes.run(async () => {
      const data = await this.readValues('data', id); data[key] = parsed
      this.validateDocument(data)
      await this.fs.atomicJson(this.logical('data', id), data, mayCommit)
    })
  }
  async deletePrivateData(id: string): Promise<void> {
    await this.writes.run(async () => {
      const paths = [...(['config', 'data'] as const).map(scope => this.logical(scope, id)), this.fileRoot(id), ...runtimeConfigPaths(id)]
      for (const path of paths) await this.fs.remove(await this.fs.path(path))
    })
  }
  async deleteData(id: string, key: string, mayCommit: () => boolean): Promise<void> {
    opaqueKey(key)
    return this.writes.run(async () => {
      const data = await this.readValues('data', id)
      if (!Object.hasOwn(data, key)) { if (!mayCommit()) fail('SESSION_EXPIRED'); return }
      delete data[key]; this.validateDocument(data)
      await this.fs.atomicJson(this.logical('data', id), data, mayCommit)
    })
  }
  async listFiles(id: string, prefix: string): Promise<Array<{ path: string; size: number }>> {
    const normalizedPrefix = this.filePrefix(prefix)
    return (await this.fs.listFiles(this.fileRoot(id)))
      .filter(file => file.path.startsWith(normalizedPrefix))
      .map(file => ({ path: file.path, size: file.size }))
  }
  async readFile(id: string, path: string): Promise<string | null> {
    const logical = this.fileLogicalPath(id, path)
    try { return (await this.fs.read(logical, PluginStore.maxFileBytes)).toString('base64') }
    catch (error) { if (isMissing(error)) return null; throw error }
  }
  async writeFile(id: string, path: string, encoded: string, mayCommit: () => boolean): Promise<void> {
    const logical = this.fileLogicalPath(id, path); const bytes = this.decodeBase64(encoded)
    return this.writes.run(async () => {
      const current = await this.fs.listFiles(this.fileRoot(id))
      const old = current.find(file => file.path === path)
      const total = current.reduce((sum, file) => sum + file.size, 0) - (old?.size ?? 0) + bytes.length
      if (!old && current.length >= PluginStore.maxFileCount || total > PluginStore.maxFilesBytes) fail('STORAGE_LIMIT')
      const parent = logical.slice(0, logical.lastIndexOf('/'))
      await this.fs.mkdir(parent)
      await this.fs.atomicBytes(logical, bytes, mayCommit)
    })
  }
  async deleteFile(id: string, path: string, mayCommit: () => boolean): Promise<void> {
    const logical = this.fileLogicalPath(id, path)
    return this.writes.run(async () => {
      if (!mayCommit()) fail('SESSION_EXPIRED')
      await this.fs.remove(await this.fs.path(logical))
    })
  }
  async exportBackup(id: string): Promise<unknown> {
    const data = await this.readValues('data', id)
    const files = await this.fs.listFiles(this.fileRoot(id))
    const dataBytes = Buffer.byteLength(JSON.stringify(data), 'utf8')
    let totalBytes = dataBytes
    const encodedFiles: Array<{ path: string; data: string }> = []
    for (const file of files) {
      if (file.size > PluginStore.maxFileBytes) fail('STORAGE_LIMIT')
      totalBytes += file.size
      if (totalBytes > PluginStore.maxBackupBytes) fail('STORAGE_LIMIT')
      const content = await this.fs.read(`${this.fileRoot(id)}/${file.path}`, PluginStore.maxFileBytes)
      encodedFiles.push({ path: file.path, data: content.toString('base64') })
    }
    if (totalBytes > PluginStore.maxBackupBytes) fail('STORAGE_LIMIT')
    return { format: 'webtools-plugin-storage', version: 1, pluginId: id, exportedAt: new Date().toISOString(), data, files: encodedFiles }
  }
  async importBackup(id: string, input: unknown, mayCommit: () => boolean): Promise<void> {
    if (!input || typeof input !== 'object' || Array.isArray(input)) fail('INVALID_INPUT')
    const backup = input as Record<string, unknown>
    if (Object.keys(backup).length !== 6 || !['format', 'version', 'pluginId', 'exportedAt', 'data', 'files'].every(key => Object.hasOwn(backup, key))
      || backup.format !== 'webtools-plugin-storage' || backup.version !== 1 || backup.pluginId !== id
      || typeof backup.exportedAt !== 'string' || !Number.isFinite(Date.parse(backup.exportedAt))
      || !backup.data || typeof backup.data !== 'object' || Array.isArray(backup.data) || !Array.isArray(backup.files)) fail('INVALID_INPUT')
    const data: Record<string, PluginJson> = {}
    for (const [key, value] of Object.entries(backup.data as Record<string, unknown>)) {
      opaqueKey(key); data[key] = this.validateJson(value)
    }
    this.validateDocument(data)
    if (backup.files.length > PluginStore.maxFileCount) fail('STORAGE_LIMIT')
    const files: Array<{ path: string; bytes: Buffer }> = []
    const seen = new Set<string>()
    let totalBytes = Buffer.byteLength(JSON.stringify(data), 'utf8')
    for (const item of backup.files) {
      if (!item || typeof item !== 'object' || Array.isArray(item)) fail('INVALID_INPUT')
      const entry = item as Record<string, unknown>
      if (Object.keys(entry).length !== 2 || !Object.hasOwn(entry, 'path') || !Object.hasOwn(entry, 'data')) fail('INVALID_INPUT')
      const path = entry.path as string
      this.fileLogicalPath(id, path)
      if (seen.has(path)) fail('INVALID_INPUT')
      seen.add(path)
      const bytes = this.decodeBase64(entry.data)
      totalBytes += bytes.length
      if (totalBytes > PluginStore.maxBackupBytes || files.length >= PluginStore.maxFileCount) fail('STORAGE_LIMIT')
      files.push({ path, bytes })
    }
    return this.writes.run(async () => {
      if (!mayCommit()) fail('SESSION_EXPIRED')
      // JSON and plugin files are separate filesystem paths, so keep a bounded
      // snapshot and roll it back if any step of this multi-path import fails.
      const previous = await this.snapshotStoredData(id)
      if (!mayCommit()) fail('SESSION_EXPIRED')
      try {
        await this.fs.atomicJson(this.logical('data', id), data, mayCommit)
        await this.fs.remove(await this.fs.path(this.fileRoot(id)))
        for (const file of files) {
          if (!mayCommit()) fail('SESSION_EXPIRED')
          const logical = this.fileLogicalPath(id, file.path)
          await this.fs.mkdir(logical.slice(0, logical.lastIndexOf('/')))
          await this.fs.atomicBytes(logical, file.bytes, mayCommit)
        }
      } catch (error) {
        try { await this.restoreStoredData(id, previous) }
        catch { fail('STORAGE_RECOVERY_FAILED') }
        throw error
      }
    })
  }
  async flush(): Promise<void> { await this.writes.run(async () => undefined) }
}
