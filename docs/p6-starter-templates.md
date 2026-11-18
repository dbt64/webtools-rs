# P6 第三项 · 四框架插件模板

SDK 和生成器为 1.16.2。开发者可从 Vanilla、Vue、React、Svelte 任选一个框架，再选 Web 或 Node 运行时。四种框架沿用各自的入口和构建方式，功能示例共享 `web/src/demo.ts` 和同一份示例数据。

| 示例 | 运行时 | 使用方法 |
| --- | --- | --- |
| 普通插件窗口 | Web / Node | 在插件中心打开 **Home**。Node 模板额外显示 preload 读取的包内示例文字和启动计数。 |
| 宿主子输入和动态结果 | Web / Node | 点 **打开宿主子输入**，搜索「文件」或 `window`，选中结果后页面显示所选项。 |
| 选中文件 | Web / Node | 在资源管理器选中 1–64 个文件，通过超级面板打开 **查看选中文件**。插件仅显示本次调用传入的路径。 |
| 自定义窗口 | Web / Node | 授予窗口管理权限后打开子窗口，向主窗口发消息，再由主窗口关闭。样式复用 SDK 的 `ui.css` 和宿主主题。 |
| 无界面命令 | Node | 在记事本选中文字，通过超级面板运行 **整理选中文字**。命令合并多余空白，直接把结果回传给面板。 |

`web/src/demo.ts` 通过公共 Web SDK 订阅上下文和查询修订，关闭时取消订阅。Vite 一次生成主页面和 `web/child.html`。Node 命令源文件位于 `assets/normalize-selection.cjs`，SDK 构建时复制到 `resources/`，包内登记为 `node-command`。Node 和 Web 示例沿用 Manifest v3 与既有宿主权限检查。

## 验证

- `scripts/verify-p6-toolchain.mjs` 从独立 tarball 生成并检查八个组合。类型检查、构建、验证、打包、检查归档和 PluginManager 安装校验全部通过。
- `scripts/verify-p6-templates.mjs` 在隔离配置中运行打包版 Manager。四框架各启动一个模板。自动检查宿主动态搜索及结果选择、子窗口打开及消息和关闭、开发模式的选中文件路径、开发模式的 Node 命令执行。文件与文字输入通过现有开发入口模拟。
- 完整 Windows 预览和验收插件位于桌面 `webtoolstest/P6-Templates-20261118-r2`。资源管理器真实选区、原生超级面板内的命令结果和真实键盘操作留在人工验收。
