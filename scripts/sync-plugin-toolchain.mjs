// Validate the desktop runtime against the pinned SDK and generator.
import { readFile } from 'node:fs/promises'
const root = new URL('../', import.meta.url)
const read = async p => JSON.parse(await readFile(new URL(p, root), 'utf8'))
const sdk = await read('plugin-sdk/declarative-v1/package.json')
const target = await read('plugin-sdk/declarative-v1/runtime-target.json')
const product = await read('package.json')
const generator = await read('plugin-sdk/create-webtools-plugin/package.json')
const expected = { sdkVersion: sdk.version, developmentNode: sdk.engines.node, ...target }
if (target.productVersion !== product.version) throw Error('Runtime target product version drift')
const installedElectron = await read('node_modules/electron/package.json')
if (target.electronVersion !== installedElectron.version) throw Error('Runtime target Electron version drift')
const file = 'plugin-sdk/create-webtools-plugin/src/toolchain.json'
if (generator.version !== sdk.version || generator.engines.node !== sdk.engines.node
  || JSON.stringify(await read(file)) !== JSON.stringify(expected)) {
  throw Error('Plugin toolchain drift; run pnpm toolchain:sync in the webtools-plugin repository')
}
for (const guide of ['plugin-sdk/create-webtools-plugin/README.md', 'plugin-sdk/declarative-v1/README.md', 'docs/plugin-development.md']) {
  const text = await readFile(new URL(guide, root), 'utf8')
  for (const match of text.matchAll(/(?:webtools-plugin-sdk|create-webtools-plugin)-(\d+\.\d+\.\d+)\.tgz/g)) {
    if (match[1] !== sdk.version) throw Error(`${guide}: outdated tarball ${match[0]}`)
  }
}
console.log(`Plugin toolchain ${sdk.version}: SDK, generator and runtime target agree`)
