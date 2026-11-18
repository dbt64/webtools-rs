# P6 第二项：插件作者开发链路

本项完成开发入口、上下文模拟、热重载、断线重连、诊断恢复及正式包安装更新的接线。SDK/生成器为 1.16.1，沿用第一项已验证的四框架 × Web/Node 外部项目生成流程。下一项模板功能覆盖和全面兼容验收仍单独推进。

## 作者流程

1. 用配套生成器创建 Manifest v3 项目，安装依赖。在项目目录执行 `npm run dev` 或 `pnpm dev`。
2. 打开 Manager → 插件中心，启用开发者模式。通过右上菜单“分离为独立窗口”（Ctrl+D）保留管理页，再选择“连接开发项目…”并授权。
3. 在“入口与上下文模拟”选择 Manager、Launcher 或 Super Panel 的文字/文件/目录，填写数据后点击 Feature。插件通过公共 SDK `context.current()` 接收内容；`lifecycle.onEnter` 接收再次进入。无需插件使用私有 IPC。
4. 修改源文件并保存。SDK 重建并通过已有开发管道发送完整包；宿主重复清单和资源校验后重载当前 Feature，并保留同一连接下该 Feature 最近成功模拟的输入。
5. 语法错误会进入开发诊断；修复后下一次有效构建恢复。复制诊断沿用已有脱敏机制。
6. Ctrl+C 停止 watcher 后连接断开。重新执行开发命令，点击“重新连接上次项目”，重新确认权限。新会话使用新票据；关闭开发者模式会清除记住的项目路径。
7. 结束开发连接后运行 `npm run typecheck`、`npm run build`、`npm run validate`、`npm run pack` 和 `npm run inspect`，在插件中心安装生成的 `.wtplugin`。升级时提高插件 `manifest.json` 的 `version` 后重新构建打包，安装新包；插件 ID 保持不变。

## 模拟范围

| 选择 | SDK 上下文 |
| --- | --- |
| Manager | `null` |
| Launcher 输入文字 | `{type:'text', source:'launcher-input', text:…}` |
| 面板选中文字 | `{type:'text', source:'selected-text', text:…}` |
| 面板选中文件 | `{type:'files', source:'explorer-selection', items:[{kind:'file',path:…}]}` |
| 面板选中目录 | `{type:'directories', source:'explorer-selection', items:[{kind:'directory',path:…}]}` |

输入必须匹配 Feature 的 `acceptedContexts`（包括文件扩展名、数量、文字匹配条件）。不接受时返回 `DEV_CONTEXT_UNSUPPORTED`；畸形输入返回 `DEV_INVALID_CONTEXT`。完整模拟调用最多 16 KiB，Launcher 最多 300 字符，路径列表最多 64 项。路径只是模拟元数据，不证明文件存在，也不授予读取权限。

## 实现边界

开发者操作仍是 Manager 主 frame 专用 IPC；插件激活继续经过现有授权、运行时和能力校验。没有新增 Rust 通信协议、第二个调度器或全局守护进程。开发数据与正式安装数据隔离。重连不会复用上次项目的旧票据，也不会自动授权。激活异步完成时会再次确认连接和构建代次，避免旧项目输入落入后续连接。

SDK 1.16.1 额外补上 npm 的 `package-lock.json` 监听，和已有 pnpm 锁文件行为一致。依赖声明或锁文件变更触发构建，不自动执行包管理器。

## 验证证据

- `electron/plugins/plugin-development.test.mjs`：授权、上下文接受条件、断线、重连和异步代次回归。
- `src/shared/development-invocation.test.mjs`、`src/features/plugins/development-simulation.test.mjs`：五类输入、大小/来源/路径/数量边界。
- `plugin-sdk/declarative-v1/tests/dev-v3.test.mjs`：真实 watcher、预览、preload 和 npm 锁文件重建。
- `scripts/verify-p6-development.mjs`：仓库外实际安装的 SDK watcher → 真实认证开发管道 → 打包版 Electron Manager → 公共 Web SDK；覆盖上下文、热重载、失败恢复、轮换会话重连及 PluginManager 安装升级保留数据。

端到端脚本使用隔离测试 profile 和既有验收钩子选择项目/批准授权；它不代表已经自动点击过真实 Windows 文件对话框，也不替代用户对完整 Rust+Electron 预览的人工验收。当前交付的 `验收报告与步骤.md` 记录实际结果。
