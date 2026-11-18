import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { validatePackageV3ForRuntime } from '../electron/plugins/package-validator.ts'
import { createDeterministicZip } from '../plugin-sdk/declarative-v1/src/zip-writer.mjs'
import { createDeterministicZipV3 } from '../plugin-sdk/declarative-v1/runtime-v3/zip-writer-v3.mjs'

export const legacyId = 'org.webtools.acceptance.runtime'
export const modernId = 'org.webtools.acceptance.runtime-isolation'
const label = { key: 'label', label: '标签', type: 'text', minLength: 0, maxLength: 80, default: '初始标签' }
const compact = { key: 'compact', label: '紧凑模式', type: 'boolean', default: false }
const archive = (pack, manifest, files = []) => pack([{ name: 'manifest.json', data: JSON.stringify(manifest) }, ...files])

const document = `<!doctype html><html lang="zh-CN"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>P6 v3 兼容验收</title><link rel="stylesheet" href="./sdk/ui.css">
<main class="wt-container wt-stack"><h1 class="wt-title">P6 v3 兼容验收</h1>
<label class="wt-field">验收内容<input id="note" class="wt-input" maxlength="80" value="兼容验收数据"></label>
<div><button class="wt-button wt-button-primary" id="save" disabled>写入验收数据</button>
<button class="wt-button" id="load" disabled>读取验收数据</button></div>
<p id="status" role="status">等待宿主</p><pre id="result"></pre></main>
<script type="module" src="./sample.mjs"></script></html>`

const sample = `import { connectWebTools } from './sdk/index.mjs'
const host = connectWebTools()
const status = document.querySelector('#status')
const output = document.querySelector('#result')
const key = 'p6-note'
const filePath = 'notes/compat.txt'
async function load() {
  const file = await host.storage.files.read(filePath)
  const result = {
    json: await host.storage.get(key),
    file: file === null ? null : new TextDecoder().decode(file),
    config: await host.config.get(),
    node: window.p6Compatibility.read(),
    browser: localStorage.getItem(key),
  }
  output.textContent = JSON.stringify(result, null, 2)
}
const bind = (selector, action) => {
  const button = document.querySelector(selector)
  button.disabled = false
  button.addEventListener('click', async () => {
    try { await action(); status.textContent = '操作完成' }
    catch (error) { status.textContent = '操作失败：' + (error.code ?? error.message) }
  })
}
try {
  await host.ready
  bind('#load', load)
  bind('#save', async () => {
    const text = document.querySelector('#note').value
    await host.storage.set(key, text)
    await host.storage.files.write(filePath, new TextEncoder().encode(text))
    await host.config.set('label', text)
    window.p6Compatibility.write(text)
    localStorage.setItem(key, text)
    await load()
  })
  await load()
  status.textContent = '宿主已连接'
} catch (error) { status.textContent = '连接失败：' + (error.code ?? error.message) }
`
const preload = `const { contextBridge } = require('electron')
const { readFileSync, writeFileSync } = require('node:fs')
const { join } = require('node:path')
const path = join(process.env.WEBTOOLS_PLUGIN_DATA, 'p6-note.txt')
contextBridge.exposeInMainWorld('p6Compatibility', {
  read() {
    try { return readFileSync(path, 'utf8') }
    catch (error) { if (error.code === 'ENOENT') return null; throw error }
  },
  write(text) {
    if (typeof text !== 'string' || text.length > 80) throw new TypeError('Invalid note')
    writeFileSync(path, text, 'utf8')
  },
})
`

export async function buildFixtures(directory, olderSdkPackage) {
  await mkdir(directory, { recursive: true })
  const example = JSON.parse(await readFile(new URL('../plugin-sdk/examples/plugins/private-notes/manifest.json', import.meta.url), 'utf8'))
  const legacyPage = {
    id: 'home', title: 'P6 v1 兼容验收', blocks: [
      { type: 'heading', text: '旧版声明式便笺' },
      { type: 'text-input', settingKey: 'label' },
      { type: 'button', label: '保存便笺', actionId: 'save-note' },
      { type: 'button', label: '读取便笺', actionId: 'load-note' },
    ],
  }
  const legacy = {
    ...example, id: legacyId, name: 'P6 v1 兼容验收', entry: { ...example.entry, label: 'P6 v1 兼容验收' },
    settings: [label], requestedCapabilities: [...example.requestedCapabilities, 'plugin.config.read', 'plugin.config.write'],
    pages: [legacyPage], actions: [...example.actions, { id: 'set-label', type: 'plugin.config.write', key: 'label' }],
  }
  const existing = await validatePackageV3ForRuntime(await readFile(olderSdkPackage), '0.1.0')
  const sdkFiles = existing.manifest.runtimeFiles.filter(file => file.path.startsWith('web/sdk/') || file.path.startsWith('web/runtime-v3/'))
  const sources = [
    { path: 'web/index.html', role: 'web', mediaType: 'text/html', text: document },
    { path: 'web/sample.mjs', role: 'web', mediaType: 'text/javascript', text: sample },
    { path: 'preload.cjs', role: 'node-preload', mediaType: 'text/javascript', text: preload },
  ]
  const capabilities = ['plugin.config.read', 'plugin.config.write', 'plugin.storage.read', 'plugin.storage.write']
  const modern = {
    manifestVersion: 3, id: modernId, name: 'P6 v3 兼容验收', description: '在插件升级和宿主重启之间验证本地数据。', author: { name: 'WebTools' },
    version: '1.0.0', api: { apiMajor: 1, minHostVersion: '0.1.0' }, type: 'plugin',
    requestedCapabilities: capabilities, runtime: { kind: 'node', preload: 'preload.cjs' }, compatibility: { platforms: ['win32'], architectures: ['x64'] },
    configuration: { schemaVersion: 1, settings: [label] },
    features: [{ id: 'home', title: 'P6 v3 兼容验收', entry: { kind: 'web-bundle', document: 'web/index.html' }, requiredCapabilities: capabilities }],
    runtimeFiles: [...sdkFiles, ...sources.map(({ text, ...file }) => file)], dependencies: [], package: { format: 'wtplugin', packageVersion: 1 },
  }
  const files = [...sdkFiles.map(({ path }) => ({ name: path, data: existing.snapshot.getResource(path).bytes })),
    ...sources.map(({ path, text }) => ({ name: path, data: text }))]
  const artifacts = {
    legacy: archive(createDeterministicZip, legacy),
    legacyUpdate: archive(createDeterministicZip, {
      ...legacy, version: '1.1.0', settings: [label, compact],
      pages: [{ ...legacyPage, blocks: [...legacyPage.blocks, { type: 'checkbox', settingKey: 'compact' }] }],
      actions: [...legacy.actions, { id: 'set-compact', type: 'plugin.config.write', key: 'compact' }],
    }),
    modern: archive(createDeterministicZipV3, modern, files),
    modernUpdate: archive(createDeterministicZipV3, { ...modern, version: '1.1.0', configuration: { schemaVersion: 1, settings: [label, compact] } }, files),
  }
  return Object.fromEntries(await Promise.all(Object.entries(artifacts).map(async ([key, bytes]) => {
    const path = join(directory, key + '.wtplugin')
    await writeFile(path, bytes)
    return [key, path]
  })))
}
