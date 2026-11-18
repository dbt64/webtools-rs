# P5 内置 MCP 服务与 Agent 连接

本地 MCP 服务属于 WebTools 平台，入口为 Manager → 设置 → AI。无需安装“服务插件”。插件仅贡献工具声明和执行函数，所有插件共用一个 HTTP 端点。

## 使用与生命周期

- 总开关负责服务监听；按插件的开关控制工具发现和调用，不停用插件本身。
- 启用状态、端口、令牌和插件级开关保存在用户配置目录的 plugin-mcp-settings.json。默认地址 http://127.0.0.1:24459/mcp。端口占用会显示错误，不偷偷更换地址。
- Rust 托盘进程负责监听、认证与工具发现。已有有效工具快照时，开启服务、初始化和查询工具不会启动 Electron。首次无快照会临时启动执行宿主同步一次；合法空列表也算有效快照。
- 调用工具时按需启动同一个 Electron 宿主，继续使用现有插件授权、Schema 校验和 Node 执行器。最后一次执行实际结束后冷却 15 秒；没有其他窗口或任务时自动回收 Electron。关闭 Manager 窗口不停止服务；关闭总开关停止监听，退出托盘结束整个托管链路。
- 正式用户仍使用原 Manager 用户目录中的完整配置；隔离验收的嵌套 profile 不覆盖正式设置。升级保留地址、令牌和插件级开关，旧根目录 enabled 标记不再是配置来源。端口绑定失败保留开启偏好，页面显示错误并可重试同一端口。
- 配置失效不会导致 Manager 无法启动；保留原文件并在 AI 设置显示错误，不把读取失败显示成用户关闭。

实现与本轮证据见 [原生 MCP 网关验证记录](mcp-native-gateway-validation.md)。这次迁移不改变插件 API 和原 AI 设置入口。

## 连接 Agent

“连接 AI Agent”检测 Codex/ChatGPT（~/.codex/config.toml）、Cursor（~/.cursor/mcp.json）与 Claude Code（~/.claude.json）。只检测已存在的客户端目录/配置，用户点击“一键配置”才写入。配置合并保留其他服务器和设置值，先保存带时间戳的完整原文件备份。TOML 会重新序列化，原注释/排版可从备份查回。格式错误时拒绝覆盖。

Codex 使用 mcp_servers.webtools.url 和 http_headers.Authorization；Cursor/Claude Code 使用 mcpServers.webtools。JSON 手动配置始终提供完整 mcpServers 包装。服务器参数少于客户端的所有可选配置项是正常的，本地 HTTP 服务无需 STDIO command/args。

## 插件开发 API

沿用 Manifest v3 的 mcpTool 和 SDK Node McpToolHandler：

```json
{
  "id": "agent-note",
  "title": "AI 便笺读写",
  "entry": {"kind":"node-command","module":"agent-note.cjs"},
  "presentation": {"type":"command"},
  "requiredCapabilities":["plugin.storage.read","plugin.storage.write"],
  "mcpTool": {
    "inputSchema":{"type":"object","properties":{"text":{"type":"string","maxLength":4096}},"additionalProperties":false},
    "outputSchema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false}
  }
}
```

```js
module.exports = async ({ mcpInput, webtools }) => {
  if (typeof mcpInput.text === 'string') await webtools.storage.set('agent-note', mcpInput.text)
  return { text: await webtools.storage.get('agent-note') ?? '' }
}
```

Manifest 顶层声明 runtime.kind=node、对应 runtimeFiles 和存储能力。带 Web 界面的 Node 插件需要 preload；本验收插件 preload 不注入业务 API，页面继续使用公开 SDK bridge。此 Node 模型运行当前用户代码，不是操作系统沙箱。验收插件由 Web 改为 Node+Web 混合是为了同一插件共享 UI 和命令私有存储，平台未给原 Web 沙箱新增 Node 权限。

对外工具名为 pluginId.featureId。工具会进行输入/输出 Schema 校验，并通过现有能力授权执行。Agent 调用没有前台点击凭证，不能自动获得粘贴等要求用户操作的权限。

## 自动验证记录

- TypeScript 检查、Electron 构建、插件包校验通过。
- MCP 配置和 Node 命令针对性测试 11/11 通过；配置写入使用临时目录，没有写入真实 Agent 配置。
- 打包版隔离验证：界面开启服务、401/403、官方 MCP 客户端发现和调用、无效输入、插件级关闭/恢复、实际 Node 命令读写存储、插件 UI 读取同一结果、关闭监听、重启后同一地址/令牌与数据恢复通过。
- 全量 pnpm test 尝试发现模板生成、SDK tarball、发布版本契约等非 MCP 领域失败，并因长时间未结束而中止；不标记全量通过。日志：webtoolstest/mcp-builtin-tests-20261113.log。
- Rust 测试构建通过（指定过滤器未匹配测试），生产二进制另行构建。完整包另外通过四项 Rust 托盘启动验证：关闭状态不监听、开启状态无 Manager 启动监听、实际命令读写、Rust 结束后端口关闭。

参考：用户提供的 uTools 8.0.2 MCP 只读报告及 newUtools/analysis/extracted/app/dist/plugins/ffffffff；Codex HTTP 配置：https://developers.openai.com/codex/mcp 。
