# WebTools Declarative Plugin Development

首次开发插件请从 [WebTools 插件作者指南](https://github.com/dbt64/webtools-plugin/blob/main/declarative-v1/docs/author-guide.md) 开始。下面保留详细契约、专题和历史示例供查阅。

This guide describes the supported local author workflow for WebTools plugins using **Manifest v1** declarative pages or **Manifest v3** Web/Node runtime packages (Plugin API major 1). The SDK and independent starter creator are distributed as local `.tgz` packages; neither is published to npm or a plugin marketplace. Check the SDK package version and use a matching host build; the product version alone does not identify the available bridge methods.

Third-party plugins may use fixed declarative pages (v1) or package-local Web/Node runtime code (v3). WebTools' Plugin Center install-time validation, local-code trust consent, grants, and lifecycle remain authoritative even after SDK validation succeeds.

Manifest v2 Web Plugin packages are retired and must be repacked as Manifest v3. The SDK provides bounded v2 archive inspection for migration diagnostics, but does not install or activate v2 packages.

## Runtime v3 author workflow

P4 的生命周期、窗口、桌面能力及兼容说明见 SDK 自带的 [P4 API 指南](https://github.com/dbt64/webtools-plugin/blob/main/declarative-v1/docs/p4-api-guide.md)；插件私有 JSON/文件存储、备份和卸载保留规则见 [插件存储指南](https://github.com/dbt64/webtools-plugin/blob/main/declarative-v1/docs/plugin-storage-guide.md)。

### Selected-text action matching (SDK 1.8.0)

Manifest v3 may add `textMatch: 'http-url'` to an accepted context with `type: 'text'` and sources exactly `['selected-text']`. This is the only supported content rule. Omit it for ordinary text actions, including Translation; no language filter is implied. Other rule values, additional matcher fields, and applying the rule to other sources are rejected by SDK and authoritative Host validation.

NativeHost evaluates the bounded original selection locally during discovery. Electron projects only Feature metadata and the rule enum; matching does not start Electron, invoke a plugin/provider, persist text, or add it to diagnostics. After an explicit activation, Main receives the existing authorized context and rechecks eligibility and current grants. Older Native peers omit the entire conditional Feature rather than removing its condition.

`http-url` trims only outer ECMAScript whitespace for recognition and accepts HTTP(S), bounded BMP Unicode domain labels, canonical dotted IPv4, hex-group IPv6 and ports 1–65535. Credentials, internal whitespace/controls, backslashes, partial URLs, unsupported schemes, numeric alternate IPv4 spellings, scoped IPv6 and embedded IPv4-in-IPv6 are excluded. This is a deterministic URL-shape rule, not a promise of full IDNA/A-label validity or DNS reachability. Native and JS use the same corpus; invocation and the browser still apply their existing URL/security checks. Context text remains unchanged.

The optional [Selected Text Actions example](https://github.com/dbt64/webtools-plugin/blob/main/examples/plugins/selected-text-actions/README.md) implements real browser search and open-link Commands using only the public SDK. It is not installed automatically. Its build bundles the public Node adapter before the official SDK validates/packs it. A current Node Command may receive `invocation.webtools.external?.open(url)` when `external.open` is declared, granted and currently authorized. The existing Host path accepts only bounded HTTP(S) URLs without credentials and consumes one explicit activation gesture per call; there is no raw OS fallback. Search sends the selected query to Bing only after activation. This adds no new permission, changes no Product/API/Manifest version, and does not implement same-panel Translation or a dictionary.


### 插件生命周期通知（SDK 1.10.0，P4 第一部分）

Web/Node 页面通过 `connectWebTools().lifecycle` 订阅当前 Feature 页面的生命周期。Node 无界面命令仍沿用原有一次调用模型。

```js
const host = connectWebTools()
const offInitialize = host.lifecycle.onInitialize(() => { /* bridge ready，初始化页面资源 */ })
const offEnter = host.lifecycle.onEnter(async event => {
  // 每次显式打开该 Feature 都会收到，包括复用未销毁的页面。
  const context = await host.context.current()
  // 按业务更新页面；快速连续进入时应用自己已有的请求修订号避免旧异步结果覆盖。
})
const offShow = host.lifecycle.onShow(event => { /* 恢复显示 */ })
const offHide = host.lifecycle.onHide(event => { /* 暂停页面定时刷新 */ })
const offClose = host.lifecycle.onClose(() => { /* 同步清理本地资源 */ })
await host.lifecycle.ready
// 不再需要时调用相应的 offInitialize/offEnter/offShow/offHide/offClose。
```

- `onInitialize`：当前连接 ready 后每次订阅调用一次；取消订阅可阻止尚未执行的通知。不是每次唤出都重新初始化。
- `onEnter`：首次显式打开及再次从 Launcher/Panel/Manager 入口打开时通知。上下文先更新，再通知；`context.current()` 和原有 `context.onChange()` 继续可用。
- `onShow/onHide`：真实窗口显示、隐藏、最小化和恢复。仅使用快捷键召回旧窗口不会增加进入次数，也不会抹掉原上下文或页面状态；切换宿主子输入/结果区不等于隐藏窗口。
- 进入与可见事件包含 `featureId`、`generation`、`entryCount`；计数属于当前页面运行代次，关闭后重新打开从 1 开始。首次订阅会补发最近一次进入和当前可见状态，不重放全部历史。
- `onClose` 沿用现有 `host.onClose` 的连接关闭通知：也覆盖撤权或断连，是本地同步清理提示。宿主不等待插件异步回调，不能依赖它做最后一次持久化写入；强制杀进程/崩溃不保证执行。
- 事件只发给订阅它的新 SDK 页面，旧插件无需改动。新插件可 `await host.lifecycle.ready` 判断宿主支持；旧宿主会拒绝该新接口，且不会把原有搜索或上下文误当成生命周期支持。产品版本 0.1.0 相同不代表此可选接口已存在。

验收样例为 `platform-playground` 1.4.0，页面顶部显示最近 16 条生命周期记录。隐藏后召回应看到隐藏/显示记录且页面计数保留；选择新的文字/文件再次从同一功能进入，应增加进入次数并更新上下文；Esc 或“结束运行”后重新打开应建立新代次。

### P4 接口速查与使用场景

以下能力供 Manifest v3 Web 页面通过 `connectWebTools()` 使用；需要权限的能力要同时在插件 `requestedCapabilities` 和对应 Feature 的 `requiredCapabilities` 中声明，并由用户授权。`system`、`files` 等命名空间是可选的，应先判断是否存在。Node 无界面命令使用其自身的 Node SDK 接口，不应假设页面窗口 API 在命令处理器中可用。

| 场景 | 接口 | 权限 |
| --- | --- | --- |
| 用户选择文件或文件夹、用系统程序打开 | `files.select`、`files.openPath` | `system.file.select`、`system.path.open` |
| 读写系统剪贴板文字、图片、文件 | `clipboard.readText/readImage/readFiles`、`writeText/writeImage/writeFiles` | `clipboard.current.read`、`clipboard.write` |
| 把文字粘贴回原应用 | `system.pasteText` | `system.input.paste` |
| 发送 Windows 通知 | `system.notify` | `system.notification` |
| 读取显示器、截图 | `system.getDisplays`、`system.captureScreen` | `system.screen.capture` |
| 创建、显示、隐藏和管理插件子窗口 | `windows.create/show/hide/close/setBounds/setAlwaysOnTop/postMessage` | `plugin.window.manage` |

例如，在按钮的点击回调内选择文件并用系统默认程序打开：

```js
const host = connectWebTools()
button.addEventListener('click', async () => {
  if (!host.files) return
  const paths = await host.files.select({ kind: 'file' })
  if (paths.length) await host.files.openPath(paths[0])
})
```

若将插件暂时隐藏到后台，页面和子窗口等资源会保留；结束该 Feature 时，其拥有的子窗口随之关闭。插件应在隐藏时暂停不需要的定时工作，在 `onClose` 做同步清理；不要依赖关闭回调完成异步持久化。具体窗口标题栏和权限细节见下文对应章节。

插件 Feature 可由用户在 Manager 中绑定全局快捷键。默认不抓取选区；可勾选“使用当前选中内容”后再用快捷键触发：资源管理器的选中文件或文件夹会作为上下文传递，其他应用中的选中文字通过模拟复制获取。图片文件属于文件上下文；截图工具中的位图选区尚不等同于文件路径，不能保证通过此快捷键传递。复制可能影响非文字剪贴板内容。已配置快捷键在启动时注册失败会发送系统通知，用户可在插件管理中更换按键；通知显示仍取决于 Windows 通知设置。旧配置缺少选区选项时按关闭处理。这个快捷入口与搜索框和超级面板一样受 Feature 的声明及授权约束，不会凭入口增加权限。快捷键启动窗口功能时，如选区类型不被该功能接受，窗口仍会打开但不传入不匹配的选区；无界面命令不按不匹配的上下文执行。

### Feature search input and dynamic results (SDK 1.9.0)

A Web Feature can ask the Host to temporarily replace its page with a host-owned search surface. The plugin receives each query with a revision and publishes the matching rows; the Host handles Chinese IME composition, arrow navigation, Enter/click selection, loading/empty/error presentation, and Escape back to the same plugin page. Ordinary Features that do not call this API keep their existing presentation. This implementation is part of the Electron Feature window and its existing authenticated Web bridge; it adds no Rust Named Pipe message.

```js
import { connectWebTools } from '@webtools/plugin-sdk/web'

const webtools = connectWebTools()
await webtools.ready
await webtools.subInput.show({ placeholder: '搜索本地条目' })

webtools.subInput.onChange(async (query, revision) => {
  const items = query
    ? localItems.filter(item => `${item.title} ${item.description}`.includes(query))
    : []
  await webtools.results.update({ query, revision, items })
})
webtools.results.onSelect(({ item }) => {
  openLocalItem(item.id)
  void webtools.subInput.hide()
})
```

`results.update({ query, revision, items, loading?, error? })` must echo the query/revision received by `subInput.onChange`. The Host discards updates for previous queries, which allows asynchronous searches without an old response replacing newer results. Query text is limited to 1024 UTF-8 bytes. A result list has at most 30 unique IDs; each row has a bounded title, optional description, and optional PNG/JPEG/WebP data-image icon. An empty list shows the no-match state. `results.clear(query, revision)` clears only that current query. `subInput.setText` emits the same change event; `getText`, `focus`, and `releaseFocus` read, focus, or dismiss the Host search surface. The listener registration methods return unsubscribe functions.

The reusable `platform-playground` package demonstrates the API using bundled local sample records that resemble clipboard entries. It does not read the system clipboard or make network requests. Install the package and follow its acceptance steps to check Chinese input, result navigation, selection, no matches, and Escape.

Use Manifest v3 for new Web and Node plugins. The SDK's `pack` command selects v1 or v3 from `manifestVersion`; it does not execute plugin code or native files.

## Optional Web theme stylesheet (Phase 6F-4E)

Manifest v3 Web plugins may explicitly import `@webtools/plugin-sdk/theme.css` from their frontend entry. This optional stylesheet consumes the Host-published `--webtools-color-scheme`, `--webtools-color-text`, and `--webtools-color-background` tokens for the browser color scheme and base page colors. It does not standardize layout, spacing, typography, controls, icons, or component styling. Plugins that do not import it keep their own theme and remain compatible, but WebTools does not promise visual consistency for those pages. Manifest v1 declarative pages are Host-rendered and do not use this stylesheet.

The v3 starter imports the stylesheet by default; remove the import to opt out. To consume the tokens in a custom stylesheet, use them directly with fallbacks:

```css
:root { color-scheme: var(--webtools-color-scheme, light dark); }
body {
  color: var(--webtools-color-text, #1b2633);
  background: var(--webtools-color-background, #f3f5f8);
}
```

The Host injects only its frozen `:root` custom properties. The plugin stylesheet must explicitly use them; custom properties do not automatically set the real CSS `color-scheme` or style controls. See [Plugin Window Theme and UI Contract](plugin-window-theme-contract.md).

Minimal Web runtime manifest shape:

```json
{
  "manifestVersion": 3,
  "id": "org.example.weather",
  "name": "Weather",
  "description": "Weather lookup",
  "author": { "name": "Example" },
  "version": "1.0.0",
  "api": { "apiMajor": 1, "minHostVersion": "0.1.0" },
  "type": "plugin",
  "requestedCapabilities": [],
  "runtime": { "kind": "web" },
  "compatibility": { "platforms": ["win32"], "architectures": ["x64"] },
  "features": [{ "id": "home", "title": "Weather", "entry": { "kind": "web-bundle", "document": "web/index.html" } }],
  "runtimeFiles": [{ "path": "web/index.html", "role": "web", "mediaType": "text/html" }],
  "dependencies": [],
  "package": { "format": "wtplugin", "packageVersion": 1 }
}
```

For a Node plugin, set `runtime.kind` to `node`, declare a `.cjs` preload entry, include it in `runtimeFiles` with role `node-preload`, and list the platform/architecture compatibility. The SDK materializes production dependencies from the plugin project's `package.json`; only production, optional, and peer dependencies are included. Keep bundler/framework tooling such as TypeScript or Vite in `devDependencies`. The package must already contain built browser output under its declared `web` paths.

If the Node preload imports `@webtools/plugin-sdk/node` (as the Node starter does), install the supplied SDK tarball as a **production dependency** so the preload can resolve it after installation. A Web plugin can keep the SDK in `devDependencies` because Vite includes its browser entry in the built frontend. For example, from a Node project use `pnpm add --save-prod "@webtools/plugin-sdk@file:../../artifacts/webtools-plugin-sdk-1.16.2.tgz"`; from a Web project use `pnpm add --save-dev` with the same tarball.

```json
"runtime": { "kind": "node", "preload": "runtime/preload.cjs" }
```

`webtools-plugin pack . --out dist/weather.wtplugin --host-version 0.1.0` resolves npm/pnpm dependency layouts and writes a deterministic archive. Its generated inventory includes nested and scoped packages, CommonJS/JSON files, native `.node` dependencies, and package-contained helper resources. Native addons require a `compatibility.nodeAbi` matching the target WebTools Electron runtime and Windows PE architecture. Build native artifacts for the target platform before packing. The pack command validates and inventories `.exe`/`.dll`/`.node`; it never runs them.

Node-capable plugins execute as local code under the current Windows user after install/update consent. This is not an OS sandbox for files available to that user. The Node preload runs in an isolated preload world with `nodeIntegration: false`; page JavaScript does not receive Node globals. CommonJS is the preload entry format; it can use `require()` and `await import()` for ESM-only runtime dependencies. Node plugins can use standard `fs`, `path`, `os`, `crypto`, `stream`, `http`, `https`, `net`, `child_process`, and other Node APIs without WebTools wrappers. The stable plugin-local directory is provided as `process.env.WEBTOOLS_PLUGIN_DATA`; ordinary runtime close, Manager restart, and plugin update preserve it. Uninstall data deletion is a separate user choice.

Standard Web plugins use ordinary Chromium APIs. HTTP/HTTPS, fetch, XHR, WebSocket, SSE, CORS/TLS, localStorage, IndexedDB, Dedicated/Module Workers, WASM, and Blob are not WebTools capabilities. Browser CORS/TLS rules apply to browser requests; Node networking follows Node behavior. Browser storage is persistent and isolated per plugin origin. The WebTools Host bridge remains separate: `context`, PluginStore, Shared AI, and clipboard go through Main authority, and raw Shared AI tokens, SecretStore, Manager private IPC, and NativeHost pipe authority are not exposed through the plugin API.

## Feature presentation and invocation

A Feature identifies a plugin capability with the canonical pair `{ pluginId, featureId }`. Its invocation surface identifies the caller (`manager`, `launcher`, or the reserved future `super-panel`); its presentation identifies how it runs. Invocation surfaces are not Manifest presentation types.

Manifest v3 accepts an optional `presentation`. Existing v3 features without it normalize to `{ "type": "window" }`; Manifest version and Plugin API major stay unchanged.

```json
{
  "id": "editor",
  "title": "JSON Editor",
  "entry": { "kind": "web-bundle", "document": "web/index.html" },
  "presentation": {
    "type": "window",
    "defaultWidth": 960,
    "defaultHeight": 680,
    "minWidth": 360,
    "minHeight": 260,
    "resizable": true
  }
}
```

`window` opens a separate native-framed Electron BrowserWindow in the existing WebTools Electron Host. The Host clamps dimensions to the available display. Dimension values are integers from 1 through 32768; the Host applies usable minimums and safe defaults. No preload, partition, sandbox, security preferences or other Electron constructor options are author configurable.

The same installed Feature has one window. Reopening restores/focuses it and supplies a fresh invocation context through the existing Runtime 2.0 context API. Different features, including multiple features from the same plugin, can stay open simultaneously. Feature title is the window title; package asset icons fall back to the WebTools icon. Window bounds are not persisted in this foundation.

`{ "type": "command" }` declares one finite invocation without a persistent window. It accepts no window options. The supported first execution adapter is **Node + `node-command` entry**; Web commands and commands that only declare an HTML/preload entry remain unsupported and are omitted from Launcher discovery. They never execute in a hidden BrowserWindow or hidden HTML page.

Plugin Center's Feature Open action calls Main's unified activation service and leaves Manager in the management center. Plugin settings and Manifest v1 declarative pages remain in Manager. Installed v3 Features participate in Native Launcher discovery/search through Main's validated projection. Native canonical Feature requests use the same activation service and existing authenticated Named Pipe without requiring Manager UI. Development Features are not included in ordinary Launcher search. Super Panel remains separate work.

## Node command entry

Declare a CommonJS module as a command entry, and inventory it with role `node-command` and media type `text/javascript`:

```json
{
  "id": "format-text",
  "title": "Format Text",
  "commands": ["format"],
  "presentation": { "type": "command" },
  "entry": { "kind": "node-command", "module": "resources/format.cjs" },
  "acceptedContexts": [{ "type": "text", "sources": ["launcher-input"] }],
  "requiredCapabilities": ["plugin.storage.write"]
}
```

The package must have `runtime.kind: "node"` and request the capabilities required by the Feature. A command-only package does not require `runtime.preload` or an HTML entry; a Node package with Web window entries still requires the existing Node preload. The Host repeats inventory, compatibility, materialization and entry integrity checks before executing the handler.

For the SDK build path, place the module at `assets/format.cjs`. The existing asset build copies it to `dist/resources/format.cjs` and inventories declared command modules as `node-command`. A command-only project skips the Vite Web build and needs no Web page. Existing window starters and their build scripts remain unchanged. This task does not add a command starter or distribute a new SDK artifact.

```js
// assets/format.cjs
const { defineCommand } = require('@webtools/plugin-sdk/node')

module.exports = defineCommand(async ({ context, signal, webtools }) => {
  signal.throwIfAborted()
  await webtools.storage.set('last-input', context?.text ?? '')
})
```

Install the matching SDK as a production dependency when requiring its Node entry. The handler receives the existing validated context or `null`, an AbortSignal, and the existing typed Host capability families: context, PluginStore, optional Shared AI and clipboard. A resolved `void` handler means completion. Shared AI/clipboard retain Main grants and Host confirmation. No raw Electron or Manager IPC is exposed by the SDK.

### Optional Super Panel Quick Result (SDK 1.7.0)

A Manifest v3 Node Command invoked explicitly from the Selected Text Super Panel can return one plain-text result. Declare a `text/selected-text` accepted context. The Host sets `invocation.resultTarget` only after negotiating support with NativeHost. Guard this optional field: older Hosts, Manager/Launcher invocations and other contexts continue to use void completion. Host product version 0.1.0 alone does not distinguish this optional capability.

```js
const { defineCommand } = require('@webtools/plugin-sdk/node')
module.exports = defineCommand(async ({ context, signal, resultTarget }) => {
  signal.throwIfAborted()
  if (resultTarget !== 'super-panel') return
  return { kind: 'text', text: `Local result: ${context?.text ?? ''}` }
})
```

Return only `{ kind: 'text', text }` or void; no HTML, extra fields, result buttons, automatic clipboard write or automatic execution. Text is limited to 8,192 Unicode scalars and 32,768 UTF-8 bytes; CR/CRLF normalize to LF. Empty/whitespace-only text, unpaired surrogates and unsupported controls are rejected. The final private transport payload is bounded to 64 KiB. These limits do not raise the existing invocation input limits.

The Host buffers a valid result until execution and Utility Process cleanup succeed, then revalidates its source and invocation before delivering it. Cancellation, authority/source changes, timeout or cleanup failure discard the result. Authors must respect `signal`; a resolved handler does not keep background ownership. Native displays inert text in the same bounded Selected Text panel. Normal Electron idle exit or transport reconnection does not erase an already accepted result; dismissing the panel, a new capture/invocation or an explicit current-source clear does. Pending results do not survive disconnection. The result is not added to projection/cache files or diagnostics.

Approved, validated development sessions can exercise the same return path. Their entries are marked as development and bound to the selected build; reload/disconnect invalidates that source rather than falling back to an installed plugin with the same ID. Keep development and packaged acceptance evidence separate.

Shared AI still requires its declared capability, current grant and Host confirmation. Quick Result presents that confirmation in the current panel; pressing the Feature alone sends no AI request. The Host rechecks the selected provider before sending. This API does not implement Translation or expose credentials. Commands retain the current-user Node local-code trust model described below.

The Node adapter starts one temporary Electron Utility child per invocation, using the existing Runtime 2.0 Node descriptor/bootstrap and validated installed files. It has a 30-second execution/preparation deadline and a maximum of eight concurrent executions per Host. Closing the command revokes capabilities, gives cancellation a bounded cooperative cleanup turn and terminates the owned Utility process if needed; it waits for actual exit before releasing ownership. Disable, uninstall, update, Host shutdown and authority revocation cancel affected executions. Developer reload cancels development commands before releasing old build files and does not automatically repeat a command.

Commands retain Node's trusted local-code model. They have no DOM, Chromium browser storage or Web window; PluginStore and Plugin Local Data remain available. Authors must stop helper processes they spawn and respond to cancellation; this adapter is not an OS sandbox or a guarantee against deliberately detached processes. Runtime failures use the existing Main diagnostic store. The Launcher receives completion/cancellation or a short failure result, not arbitrary plugin stacks.

See [Plugin Runtime and Surface Lifecycle](plugin-runtime.md) for Host ownership, authority, multi-window Session routing and shutdown behavior.

## Super Panel context

The native Super Panel uses the existing Feature projection and activation pipeline with invocation surface `super-panel`. Extend Manifest v3 `acceptedContexts` with `text/selected-text`, `files/explorer-selection`, `directories/explorer-selection`, or `none` with empty sources. File rules may bound extensions (up to 16 dot suffixes) and maxItems (1–64). These declarations are metadata-only match rules; no plugin code runs while choosing recommendations. Mixed selections require compatible file and directory rules. A general Feature receives null rather than undeclared selected data.

`webtools.context.current()` and Node command context now return the existing text shape or a file/directory selection shape containing readonly `{ kind, path }` items. Selection is captured before the Panel focuses, never inferred from old Clipboard data, and sent only to the selected Feature. Main repeats context matching against the current validated Manifest. No new filesystem, Electron or generic IPC API is exposed. See [Native Super Panel](super-panel.md) for configuration, privacy and first-version limits. SDK tarball redistribution is deferred; this task updates source/types only.

## Development Host reload (Windows)

`pnpm dev` keeps the existing Vite preview and project watcher, and also makes a temporary development session available to WebTools. Connect the project once per Electron Host/development session:

1. Run `pnpm dev` in the external Manifest v3 project using the matching local SDK.
2. Open WebTools Manager → Plugin Center and enable **开发者模式** (Developer Mode).
3. Select **连接开发项目…**, choose the project root containing `manifest.json`, and approve the project identity, runtime kind and requested Host capabilities. A Node project executes local code as the current Windows user; authorize only code you trust.
4. Open a Feature from the development panel. Edit the frontend, CSS/assets, preload, Manifest or dependency declarations while leaving `pnpm dev` running.

To keep the development panel visible alongside the plugin, use Manager's menu → **分离为独立窗口** (Ctrl+D) before opening the Feature. A temporary Manager tab otherwise follows the normal tab-switch behavior.

The panel's **入口与上下文模拟** selector supplies Manager/no context, Launcher text, Super Panel selected text, selected files or directories. Fill text or newline-separated Windows absolute paths, then click the Feature. This supplies a fixture through the normal activation and capability checks; it does not read the current clipboard or grant file access. Feature `acceptedContexts` must accept the chosen type/source and item conditions. Inputs are checked in Main as well as the UI; the complete simulated invocation is bounded to 16 KiB, Launcher text to 300 characters, and file/directory lists to 64 items. The last successful simulation is retained for that Feature's hot reload within its current connection.

After a successful rebuild, the SDK packages the new output and sends a build update over its session-specific local named pipe. The Manager initiated this connection for the selected project; WebTools does not expose a public development control listener. The Host repeats its Manifest v3/archive validation and retains an immutable snapshot. All open Feature windows belonging to that development session keep their OS windows while their old Runtime contexts are revoked and destroyed; only then are those features recreated from the new snapshot. Frontend updates use a complete Feature reload. Node preload updates use the same coordinator, revoke the old bridge/generation and destroy the old execution context before creating a new one; `require.cache` is not patched. An unopened Feature uses the latest valid snapshot when opened. Removed or failed features close their windows and retain diagnostics; unaffected open development features continue reloading.

The Manager and Electron application are not refreshed. An unrelated active plugin is not reloaded. Invalid builds leave the last validated development snapshot available and display a diagnostic. Changing Plugin ID requires restarting `pnpm dev` and reconnecting; changing runtime kind or requested capabilities requires reconnecting and approving that contract. Feature entry and other valid Manifest/resource changes are revalidated on each build.

On reconnect, the CLI sends the last valid build before the most recent failed-build diagnostic. The Host processes those messages in order, so restoring an older valid Feature does not erase the newer rebuild error. Malformed development messages close only the offending connection rather than terminating the watcher.

Development registration does not install, replace or grant privileges to a production plugin. Development PluginStore, Node local data and browser origins are scoped to the project and isolated from the installed plugin with the same ID. Reload/restart preserves those development data; it is not uninstall. Closing Manager closes only its UI. An open development Plugin Window or active command keeps the Electron Host, development connection and diagnostic state alive; reopening Manager restores the viewer. Closing Developer Mode or disconnecting the project closes that session's windows and cancels commands. When Manager, all plugin windows and active executions are gone, the Electron Host exits and disconnects development normally; a watcher or diagnostic record alone does not keep Electron resident. Stopping `pnpm dev` removes its listener and session files. No development daemon or second Electron Host is created.

The CLI keeps a private session ticket and transient build archives under `.webtools/` in the project. Generated starters ignore that directory; add `.webtools/` to `.gitignore` in existing projects. Do not commit or distribute session tickets. If WebTools is absent, the CLI prints the connection instructions and waits while the author-owned preview/watcher runs; it does not start Electron automatically. After a watcher disconnect, restart `pnpm dev` and click **重新连接上次项目**. WebTools rereads the new session ticket and asks for project consent again. The last path exists only in the current Electron Host's memory and is cleared when Developer Mode is disabled; restarting the Host requires selecting the project again. Reconnection is explicit, with no automatic background retry. The SDK also watches `package-lock.json` and `pnpm-lock.yaml`; rebuilds do not install dependency changes automatically, so run the package manager yourself when dependencies change.

For the complete external-project acceptance workflow, see [P6 development workflow](./p6-development-workflow.md).

## Developer Runtime Diagnostics

Developer Mode adds a diagnostic panel to the existing Plugin Center. It covers SDK build/Manifest/preload failures, Node preload execution and native-addon loading errors, Feature load/bridge/runtime errors, and development connection/reload failures. Each record identifies the Plugin, Feature where available, error phase/code, message, source, timestamp and runtime generation; expandable stacks are shown when captured. At most 16 current Plugin/Feature errors are held in Electron Main memory, without a log database. Manager is a viewer and closing its renderer does not clear the records while the Host remains alive.

Use **Copy Diagnostics** to copy the selected structured record. The Host redacts local absolute paths, known Host/environment credentials and credential-shaped text before either displaying or copying it. It does not include SecretStore, raw Shared AI keys or NativeHost authentication tokens. Access and clipboard copying use Manager-only IPC; plugin pages cannot query these records. Successful new generations clear the prior active error; stale generation messages cannot replace newer state.

With Developer Mode disabled, ordinary users receive a short failure message and stable error code such as `NODE_PRELOAD_FAILED`, without internal paths or stacks. Enabling Developer Mode does not change the production package-validation, capability-consent or Runtime 2.0 trust model.

## Requirements

- A supported WebTools host. The commands below use product version `0.1.0`; pass the version you actually target.
- Node.js `>=20.19`.
- pnpm `9.15.9`.
- The locally supplied `webtools-plugin-sdk-1.16.2.tgz` and `create-webtools-plugin-1.16.2.tgz` files from the matching WebTools developer build.

The local tarball contains the author CLI and public types. Its `yauzl@3.4.0` dependency and the generated starter's TypeScript compiler are resolved by pnpm during installation. TypeScript is author tooling only; it is not part of the WebTools runtime. Do not obtain the SDK with `pnpm add @webtools/plugin-sdk` from a public registry.

## Create and validate a basic plugin

Keep the SDK tarball in a local `artifacts` directory. Create a separate tooling project that depends on that exact tarball:

```text
plugin-workspace/
  artifacts/
    webtools-plugin-sdk-1.16.2.tgz
    create-webtools-plugin-1.16.2.tgz
  tooling/
```

In `tooling/package.json`, add the local SDK and pnpm pin:

```json
{
  "name": "local-webtools-plugin-tools",
  "version": "1.0.0",
  "private": true,
  "type": "module",
  "packageManager": "pnpm@9.15.9",
  "devDependencies": {
    "@webtools/plugin-sdk": "file:../artifacts/webtools-plugin-sdk-1.16.2.tgz"
  }
}
```

From `tooling/`, install the package and generate one basic project. The generated directory must be a relative descendant of the current directory; absolute, traversal, existing-target, and symlink/junction paths are rejected.

```powershell
pnpm install --no-frozen-lockfile --ignore-scripts
pnpm install --frozen-lockfile --ignore-scripts
pnpm exec webtools-plugin --help
pnpm exec webtools-plugin create basic-plugin --host-version 0.1.0
```

The starter contains `manifest.json`, `manifest.typecheck.ts`, `package.json`, `README.md`, `.gitignore`, and an empty `dist/`. Its default capability is only `manager.page`; the starter does not include plugin storage, network, executable code, or assets. Review and edit the generated manifest fields and page content as needed.

The tooling project's `basic-plugin/` folder is also a separate pnpm project. Add the same local SDK tarball as its development dependency, then install its lockfile:

```powershell
Set-Location .\basic-plugin
pnpm add --save-dev --ignore-scripts "@webtools/plugin-sdk@file:..\..\artifacts\webtools-plugin-sdk-1.16.2.tgz"
pnpm install --frozen-lockfile --ignore-scripts
pnpm run typecheck
pnpm run validate
pnpm run plugin:pack
pnpm run inspect
```

`pnpm run plugin:pack` writes the validated package to `dist/basic-plugin.wtplugin`. The script is named `plugin:pack` to avoid colliding with pnpm's built-in `pnpm pack`, which creates a Node package tarball.

The create command accepts these optional identity values:

```text
--id <id>
--name <name>
--description <text>
--author <name>
--min-host-version <version>
```

For example:

```powershell
pnpm exec webtools-plugin create my-weather-plugin --host-version 0.1.0 --id com.example.weather --name "My Weather" --description "A compact weather page" --author "Example Author"
```

An omitted ID is derived as `org.example.<directory-slug>`. If the directory name contains no ASCII slug characters, the SDK uses `plugin.<12-character stable SHA-256 suffix>` so different non-Latin names do not collide. The display name is derived from the directory name, and `minHostVersion` defaults to `--host-version`. A supplied minimum Host version cannot be newer than the selected target version. The command writes values as JSON data and never evaluates them as shell or plugin code.

## CLI reference

The CLI accepts one command and only that command's documented options. `create`, `validate`, `pack` and `inspect` require `--host-version`; `dev` and `build` read the project Manifest.

```text
webtools-plugin --help
webtools-plugin dev [project-directory]
webtools-plugin build [project-directory] [--mode <mode>] [--json]
webtools-plugin create <directory> --host-version <version> [--id <id>] [--name <name>] [--description <text>] [--author <name>] [--min-host-version <version>] [--json]
webtools-plugin validate <directory|file.wtplugin> --host-version <version> [--json]
webtools-plugin pack <directory> --out <file.wtplugin> --host-version <version> [--json]
webtools-plugin inspect <file.wtplugin> --host-version <version> [--json]
```

`validate` accepts either plugin source or a `.wtplugin` archive. `pack` never overwrites an existing output. `inspect` accepts only a `.wtplugin`, validates its bounded archive bytes in memory, and reports its identity/display metadata, manifest/API versions, minimum Host version, requested capabilities, byte length, SHA-256, and validation result. It does not extract files, install the package, prompt for consent, or execute content.

Exit codes are stable: `0` means the command succeeded; `1` means validation or an operation failed; `2` means command syntax/options are invalid. Append `--json` to get one JSON envelope on stdout. Success has the form `{ "ok": true, "command": "…", "result": { … } }`; failure has `{ "ok": false, "command": "…", "error": { "code": "…", "path": "…", "field": "…", "message": "…", "suggestion": "…" } }`. Unknown `field` values are omitted. Diagnostics use package-relative paths and do not print absolute author paths, source contents, or credential-like values.

Diagnostic codes are `INVALID_INPUT`, `INVALID_MANIFEST`, `INCOMPATIBLE_PLUGIN`, `INVALID_PACKAGE`, `SOURCE_IO`, `SENSITIVE_CONTENT`, `OUTPUT_EXISTS`, `OUTPUT_IO`, `CLI_USAGE`, and the non-fatal `GENERIC_ICON` warning.

## Manifest v1 contract

The SDK package root exports `PluginManifestV1`, fixed unions, validation/packing functions, diagnostics, and constants. The schema is available at `@webtools/plugin-sdk/manifest.schema.json`. The public API and schema help authoring; the Host runs its own validation when installing.

The manifest includes:

- `manifestVersion: 1` and a unique reverse-DNS-style `id`;
- display `name`, `description`, `author`, and semantic `version`;
- `api.apiMajor: 1` and `api.minHostVersion`;
- `type: "declarative-manager"` and an `entry` page reference;
- declared `requestedCapabilities`, `settings`, `pages`, `actions`, and PNG `assets`.

The current closed block types are:

- `heading` and `paragraph`, each containing plain text;
- `text-input`, `select`, and `checkbox`, each bound to a declared setting key;
- `divider`;
- `button`, bound to a declared action ID.

The closed setting types are bounded `text`, `enum`, `boolean`, and `number` values with the type-specific bounds/defaults in `PluginSettingV1`. The action types are config and plugin-private storage `read`/`write`, HTTPS `external.open`, `clipboard.write`, and host-mediated `sharedAI.complete`. Actions refer only to declared keys/actions and are checked against the requested capability set.

The current capabilities are `manager.page`, `plugin.config.read`, `plugin.config.write`, `plugin.storage.read`, `plugin.storage.write`, `external.open`, `clipboard.write`, and `sharedAI.complete`. Request only what the plugin needs. Host consent and grant decisions remain controlled by WebTools; unknown fields, capabilities, API majors, and manifest versions fail closed. `manager.page` is the only capability in a newly generated basic starter.

Plugin-private storage is namespaced to the plugin and does not expose other plugins' data, WebTools settings, filesystem paths, SecretStore contents, or AI credentials. Shared AI is mediated by the Host review/action flow; the plugin receives neither a credential nor direct provider access. External open and clipboard operations use the existing Host confirmation path. Disabling or uninstalling a plugin is controlled by the host, including its separate decision about retaining plugin-private data.

## Validation and installation authority

Use the workflow commands before sharing a package:

1. **Typecheck** checks the author-authored manifest value against the public TypeScript types. It does not establish that a package can be installed.
2. **Source validation** checks strict UTF-8/JSON, duplicate keys, manifest semantics, host compatibility, IDs, references, defaults, declared assets, sensitive content, and bounded source files.
3. **Pack** includes the manifest and only declared, validated files, then creates a deterministic `.wtplugin` without overwriting an existing destination.
4. **Inspect** validates the resulting archive and returns bounded metadata and a SHA-256; it does not unpack or install it.
5. **Plugin Center installation** uses the normal WebTools file picker, Host validation, integrity checks, user consent, capability grants, and plugin lifecycle. This is the final authority.

Host/SDK limits include a 20 MiB archive, 256 entries, 50 MiB expanded data, maximum compression ratio 100, a 64 KiB manifest, and a 256 KiB PNG bound with maximum 256×256 dimensions. Additional depth, page, block, action, setting, value, key, and storage quotas are exported in `LIMITS_V1` and enforced by the Host-compatible validator.

## Private Notes example

`plugin-sdk/examples/plugins/private-notes` remains a separately maintained official example with its own identity and plugin-private storage capabilities. It is not generated from the basic starter and those storage capabilities are not copied into new basic projects. To try it, create a starter project, replace its manifest/typecheck inputs with the files from that example while keeping the generated author `package.json`, then use the same `pnpm run typecheck`, `pnpm run validate`, `pnpm run plugin:pack`, and `pnpm run inspect` flow.

In Plugin Center, install the resulting `.wtplugin` with the normal local-file picker and review the declared capabilities. A local developer package is not a publisher signature, trust decision, or bypass of Host consent. The Native file picker, real consent UI, and uninstall/data-retention interaction require manual Windows acceptance.

## Compatibility and troubleshooting

- Pass the actual target WebTools product version to `--host-version`; the current repo product is `0.1.0`.
- `INCOMPATIBLE_PLUGIN` means the API major is unsupported or the manifest requires a newer minimum Host version.
- `INVALID_MANIFEST` means a field is missing, malformed, out of bounds, or unknown. Use `field` in JSON diagnostics when present.
- `INVALID_PACKAGE` means the archive, entry names, ZIP structure, declared files, compression bounds, or PNG assets failed Host-compatible checks.
- `SENSITIVE_CONTENT` means credential-like material was found in metadata or declared assets. Remove secrets; never put AI tokens in plugin manifests/assets.
- `OUTPUT_EXISTS` means the output is preserved. Choose another filename rather than deleting or overwriting it.
- `CLI_USAGE` means an unsupported/repeated/missing option or invalid create target was supplied. Run `webtools-plugin --help`.

The original Phase 6B acceptance record used SDK 1.1.0. The current rerunnable workflow uses the SDK version declared by this checkout, creates its tooling and author projects in a unique OS temporary directory, performs frozen pnpm installs and the generated v1 workflow, and compares SDK public-API results with the Electron Host adapter. Separate tests compare the SDK's v2 archive preflight with the Electron Main validator. Neither workflow automates the real Plugin Center picker or consent UI.

## Independent Manifest v3 starter creator

The v3 creator is a separate local tarball, `create-webtools-plugin-1.16.2.tgz`; it is not a published registry package. Keep it beside the matching SDK tarball in `artifacts/`. In a separate tooling project, install and run the creator package:

```powershell
pnpm add --save-dev --ignore-scripts file:..\artifacts\create-webtools-plugin-1.16.2.tgz
pnpm install --frozen-lockfile --ignore-scripts
pnpm exec create-webtools-plugin sample-plugin --framework vue --runtime web --yes
```

Then, from the generated project, install the matching SDK tarball locally before installing the generated dependencies:

```powershell
Set-Location .\sample-plugin
pnpm add --save-dev --ignore-scripts "@webtools/plugin-sdk@file:..\..\artifacts\webtools-plugin-sdk-1.16.2.tgz"
pnpm install --frozen-lockfile --ignore-scripts
pnpm run typecheck
pnpm run build
pnpm run validate
pnpm run pack
pnpm run inspect
```

For `--runtime node`, use the creator's `--save-prod` SDK install instruction because the preload imports the Node SDK at runtime. The creator supports Vanilla, Vue, React, and Svelte with both Web and Node runtime modes. Generated Web entries opt in to the SDK's theme CSS helper; authors can remove that import. Neither the generated CLI validation nor `inspect` installs the plugin. Plugin Center repeats validation at installation and remains authoritative.

## Manifest v3 configuration (Phase 6F-4D)

SDK 1.8.0 includes the Manifest v3 configuration API introduced in 1.5.0 plus an optional CSS theme-token consumer. Configuration uses an optional Host-validated `configuration` descriptor and `webtools.config.get()/set(key, value)` for Web and Node. Each setting is a closed text/enum/boolean/number descriptor with a validated default and bounds; request and grant the corresponding `plugin.config.read/write` capability and list it in the Feature's required capabilities. The Host validates package identity and runtime generation on every call. It never exposes global settings or SecretStore through config.

Plugin Detail renders these descriptors using Host controls. Users can configure installed-disabled plugins without starting them; runtime authorization remains independent. v3 data uses a versioned config namespace separate from v1 config and private storage. Incompatible saved values remain intact until an explicit reset, which retains one latest backup. Manager saves detect stale package identities and concurrent configuration revisions instead of overwriting newer changes.

The local SDK and creator tarball identities and external-author workflow are recorded in the Phase 6F-5 report below; repository tests and package validation do not replace the user's Developer Workflow Acceptance. See [the configuration implementation report](phase6f4d-plugin-configuration-development.md) for the original 6F-4D source verification limits.

## 自定义业务窗口（Manifest v3）

插件在 `requestedCapabilities` 和相应 Feature 的 `requiredCapabilities` 中声明 `plugin.window.manage` 并获授权后，可通过 `webtools.windows` 创建和管理自己的辅助窗口。`create({ title, width, height })` 返回 `{ id }`；`show(id)`、`hide(id)`、`close(id)`、`setBounds(id, { x, y, width, height })` 和 `setAlwaysOnTop(id, enabled)` 控制窗口。`currentId` 区分主窗口 `main` 和子窗口，`postMessage(targetId, data)`、`onMessage` 与 `onClosed` 用于通信及关闭通知。子窗口属于创建它的 Feature，主窗口结束时子窗口一并关闭。

`document` 可指定插件包内声明的 HTML 入口，省略时加载当前 Feature 的页面。`chrome` 默认为 `host`（WebTools 标题栏），也可以选择 `frameless`（插件自绘）或 `native`（系统边框）；无边框窗口的拖动与关闭控件需要插件页面提供。参考 platform-playground 的“自定义窗口验收”区域。
# 选择并打开文件或文件夹

Manifest v3 的 Web 插件可在 `requestedCapabilities` 和对应 Feature 的 `requiredCapabilities` 中声明 `system.file.select`、`system.path.open`。安装或更新插件时由用户授权。

通过 `await webtools.files.select({ kind: 'file' })` 选择文件，`kind: 'directory'` 选择文件夹；设置 `multiple: true` 可多选。返回绝对路径数组，用户取消选择时返回空数组。通过 `await webtools.files.openPath(path)` 使用系统默认程序打开文件，或用文件管理器打开文件夹。操作由插件页面上的用户操作触发；未声明或未授权时对应操作不可用。已有上下文传入的路径同样可以用于打开。

## 系统剪贴板

Web Feature 在 Manifest v3 中声明 `clipboard.current.read` 可由用户操作读取当前剪贴板，声明 `clipboard.write` 可写入。旧的 `clipboard.read` 仍表示一次性传入的上下文，不自动授予读取当前剪贴板的权限。安装或更新时需要授权对应能力。

`clipboard.readText()` / `writeText(text)` 处理文字；`readImage()` 返回 PNG data URL 或无图片时的 `null`，`writeImage(dataUrl)` 写入 PNG；`readFiles()` 返回 Windows 文件或文件夹绝对路径数组，`writeFiles(paths)` 写入文件列表供资源管理器粘贴。以上操作从插件前台用户操作调用，读取文件列表没有内容时返回空数组。图像通过桥接传输，PNG data URL 上限为 15,000,000 字节。


### 系统通知

Manifest v3 声明并授予 `system.notification` 后，Web 插件可调用 `await client.system.notify("标题", "内容")` 发送系统通知。通知展示受 Windows 通知设置控制；该接口负责发送通知，不表示用户已看到或点击。

### 粘贴文字到原应用

Manifest v3 声明并授予 `system.input.paste` 后，用户在插件窗口主动操作时可调用 `await client.system.pasteText("文字")`。宿主尝试切回启动器或超级面板唤出前的应用，写入系统剪贴板并发送粘贴快捷键；若无法恢复目标窗口则返回错误，不向其他窗口输入。该操作会替换当前剪贴板内容。

### 显示器信息与屏幕截图（SDK 1.11.0）

Manifest v3 在 `requestedCapabilities` 和对应 Feature 的 `requiredCapabilities` 中声明并获授 `system.screen.capture` 后，Web 插件可使用 `client.system.getDisplays()` 读取显示器 ID、名称、屏幕边界、工作区、缩放比例、旋转角度和主显示器标记。边界与工作区使用 Electron 的 DIP 坐标。

`client.system.captureScreen({ displayId })` 返回该显示器的 PNG data URL；省略 `displayId` 时截取主显示器。截图调用必须由插件页面当前的明确用户操作触发，例如直接在按钮点击回调中调用。Host 会重新检查当前授权和运行代次，返回经过缩放的截图预览（最多约 2560×1600 像素），不暴露 Electron 对象或原始屏幕捕获接口。此能力可读取屏幕画面，安装或更新时由用户授权。

## 将 Node 命令发布为 MCP 工具

在 Manifest v3 的 `node-command` Feature 上添加 `mcpTool`。工具名称为 `插件 ID.Feature ID`。工具的标题和说明取自 Feature。工具入口专供 MCP 调用，因此请省略 `commands` 和 `acceptedContexts`。它使用调用参数作为输入。

```json
{
  "id": "text-stats",
  "title": "文本统计",
  "description": "统计去除首尾空白后的 Unicode 字符数。",
  "presentation": { "type": "command" },
  "entry": { "kind": "node-command", "module": "text-stats.cjs" },
  "mcpTool": {
    "inputSchema": {
      "type": "object",
      "properties": { "text": { "type": "string", "maxLength": 4096 } },
      "required": ["text"],
      "additionalProperties": false
    },
    "outputSchema": {
      "type": "object",
      "properties": { "characterCount": { "type": "integer", "minimum": 0 } },
      "required": ["characterCount"],
      "additionalProperties": false
    }
  }
}
```

将模块列入 `runtimeFiles`，角色为 `node-command`。模块导出 CommonJS 函数，可用 SDK `McpToolHandler<Input, Output>` 标注。调用在独立 Node 命令进程中执行，结束后宿主会清理进程。

```js
module.exports = ({ mcpInput, signal }) => {
  signal.throwIfAborted()
  return { characterCount: [...mcpInput.text.trim()].length }
}
```

输入和可选的输出 Schema 使用 JSON Schema 2020-12，顶层类型为 `object`。宿主在安装时检查 Schema，在每次调用时校验参数及声明的输出，返回 MCP `structuredContent` 和 JSON 文本。工具需随插件启用。若声明宿主能力，也需授予对应权限。来自 MCP 的调用没有桌面前台手势。

安装并启用插件后，打开 Manager → 设置 → AI 开启内置 MCP 服务，复制本机 Streamable HTTP 地址和 Bearer 请求头。地址、令牌和插件级开关持久保存。关闭 Manager 窗口仍可连接，关闭服务或退出托盘停止监听。工具发现由 Rust 托盘进程处理，实际调用按需启动 Electron；授权、输入输出校验和 SDK 调用方式保持不变。当前客户端可重新请求工具列表获取变更。配套的文本统计样例在 `plugin-sdk/examples/plugins/mcp-tool-playground`。

## 内置 MCP 服务与插件工具

服务由 WebTools 设置 → AI 提供。插件通过 Manifest v3 的 mcpTool 声明工具 Schema，并导出 SDK McpToolHandler 处理函数。完整例子、Agent 配置和资源行为见 [内置 MCP 服务与 Agent 连接](p5-builtin-mcp-agent-connection.md)。验收插件 1.17.0 的 agent-note 演示 Agent 修改后由插件页面读取同一私有数据。

## P6 配套开发工具（1.16.2）

SDK 与生成器的版本、Manifest/插件包格式和 Electron 原生 ABI 是独立概念。当前配套工具、仓库外使用方式及验证边界见 [开发工具与兼容](p6-toolchain-compatibility.md)，正式作者兼容指南随 SDK 一起提供：[版本与兼容](https://github.com/dbt64/webtools-plugin/blob/main/declarative-v1/docs/version-compatibility.md)。已有插件与用户数据不因工具升级而迁移。

已安装插件的升级、配置默认值，以及 v1 与 v3 之间的数据和授权边界，见 [已安装插件与用户数据兼容](p6-installed-plugin-compatibility.md)。
