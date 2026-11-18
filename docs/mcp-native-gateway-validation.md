# 原生 MCP 网关验证记录

本轮将 MCP HTTP 监听、设置和工具发现迁到 Rust 托盘进程，Electron 仅按需执行插件工具。设计与实施步骤见 [设计](superpowers/specs/2026-11-17-native-mcp-gateway-design.md) 和 [计划](superpowers/plans/2026-11-17-native-mcp-gateway-plan.md)。用户入口保持 Manager → 设置 → AI，插件 Manifest v3 与 SDK 不变。

2026-11-17 r2 预览的自动验证已完成：Rust 180 项、显式 ready 集成 1 项、Node 定向 30 项、协议 9 项、Manager UI 6 项通过；50 轮回收、正式 15 秒冷却、无快照首次同步、8+1 并发与崩溃不重放通过。同机空闲 Private Bytes 从旧版约 170.55 MiB 降至 8.41 MiB，净少 162.14 MiB；50 次冷调用 P50/P95 为 2255/2487 ms。交付位于 `D:/SystemDefault/Desktop/webtoolstest/MCP-Native-Gateway-20261117-r2`，完整数据和人工待验收项见其中《验收报告与步骤.md》。当前状态是可人工体验验收的预览，未将 DPI、跨功能交互和真实 Agent 客户端标记为通过。

## 协议与运行边界

- Rust 官方 SDK `rmcp = 3.10.0`，禁用默认 features，仅启用 server 与 Streamable HTTP server；[对应版本文档](https://docs.rs/rmcp/3.10.0/rmcp/)。现有 TS 客户端 SDK `1.32.2` 保留。
- 无状态 JSON 响应，协商稳定的 2025 协议版本；不启用会话或服务端 SSE。GET 返回 405。
- 仅 `127.0.0.1`，严格 Host、无 Origin、Bearer 认证；HTTP body 64 KiB / 15 秒，Pipe 帧 4 MiB。
- 全局最多 8 个已接纳调用；总预算 60 秒包含启动、排队与现有 Node 命令期限。客户端不再等待不等于执行已清理，槽位和 owner 保留到确认结束。
- 每次调用生成独立内部 ID。共享地址/令牌的独立取消通知没有稳定客户端身份，无法确定归属时不按全局同号 JSON-RPC ID 取消。原请求断开、上下文取消、关服务与超时仍请求清理。
- 最后执行结束后默认 15 秒冷却，仅释放 MCP owner；不强关其他窗口、定时任务或开发会话。

## 可复现命令

在 PowerShell 7 从仓库运行；`$delivery` 为全新预览交付目录，`$fixture` 为当前 playground `.wtplugin` 的绝对路径。所有真实运行均使用短临时目录和本轮创建的进程树，不读取或修改用户真实 Agent 配置。

```powershell
pnpm run typecheck
pnpm run toolchain:check
pnpm run build
cargo test --locked --workspace
cargo test --locked -p slint-launcher host_runtime::readiness_tests::queued_preview_waits_for_electron_services_ready -- --ignored
node --experimental-transform-types scripts/verify-mcp-gateway-protocol.mjs --probe target/debug/examples/mcp_gateway_probe.exe
node --experimental-transform-types scripts/verify-p5-builtin-mcp.mjs --delivery $delivery --plugin-package $fixture
node --experimental-transform-types scripts/verify-p5-mcp-bootstrap.mjs --delivery $delivery --plugin-package $fixture --cycles 1
node --experimental-transform-types scripts/verify-mcp-native-lifecycle.mjs --delivery $delivery --plugin-package $fixture --cycles 50 --cooldown-ms 250
node --experimental-transform-types scripts/verify-mcp-native-lifecycle.mjs --delivery $delivery --plugin-package $fixture --cycles 1 --cooldown-ms 250 --no-cache
node --experimental-transform-types scripts/measure-mcp-native-gateway.mjs --delivery $delivery --baseline-delivery $oldDelivery --plugin-package $fixture
```

同一 Native 可执行文件有单实例保护，上述打包验收必须串行运行。50 轮加速冷却只在同时设置隔离 profile 与验收标记时生效，正式冷却仍为 15 秒，单独实测。采样覆盖整个本轮进程组，每种场景 3 个等长窗口；比较各窗口 Private Bytes 中位数，Working Set、句柄与每逻辑核 CPU 同时保留。具体构建指纹、次数、耗时和最终数字见交付目录的《验收报告与步骤.md》及 `verification` 原始 JSON。

## 证据与覆盖范围

| 设计条目 | 验证来源 | 状态说明 |
| --- | --- | --- |
| A1 原有快照启动、发现无需 Electron | lifecycle 与协议脚本、空快照单元测试 | 自动验证 |
| A2 首次快照同步和释放 | lifecycle `--no-cache`、server 测试 | 自动验证 |
| A3 单宿主冷/热调用、冷却回收 | 50 轮生命周期与正式 15 秒路径 | 自动验证 |
| A4 既有焦点和其他任务 owner | dispatch owner、legacy Pipe 与 ready 回归；产品全路径见人工步骤 | 部分自动，跨功能交互待人工 |
| A5 Manager 关闭、关服务和托管结束 | 打包 Manager UI、生命周期清理 | 自动验证 |
| A6 配置保留和插件级开关 | store、Manager UI 与 lifecycle | 自动验证 |
| A7 并发/取消/超时/崩溃/权限/校验 | dispatch、真实 Pipe、执行器、adapter 和 lifecycle | 自动验证；不承诺独立 POST 精确取消 |
| A8 AI 设置与 Agent 备份 | 打包 UI 深浅色、键盘、开关/重试；配置合并测试 | DPI、系统主题、真实客户端待人工 |
| A9 正式工具与 P6 | 同一 PluginManager 正式工具投影、工具链一致性检查 | 开发热重载/上下文模拟待人工 |
| A10 资源收益与延迟 | 进程组 3 窗口采样、50 冷调用计时 | 以最终交付 JSON 为准 |

## 审核修正

计划修订后独立审核允许实施。实施最终审核发现并修正两处边界：启动等待及重试期间也必须推进原 60 秒预算；正常空闲退出期间，能证明工具帧尚未发送的请求保留到下一个宿主，沿用原截止时间。已尝试发送的调用仍不自动重放。两处均补了回归测试。

实际代码确认正式 Manager 使用原用户目录，`HostOptions.profile` 仅在 `--phase4g-manager-test` 生效。Rust 设置存储显式接收实际目录，新增根目录完整配置优先于陈旧隔离配置的回归；不把验收目录误当正式路径。

## 升级、故障和回滚

保留原 `plugin-mcp-settings.json` 和插件数据；规范配置有效时始终优先，损坏文件保留并报告。缓存损坏触发一次重新同步，合法空缓存不触发启动。绑定失败不会更换端口或清除开启偏好，页面可关闭或重试。

此次是单一路径切换，没有运行时方案 B。需要回滚时使用先前完整交付版本，保留用户 profile；旧版本需要根目录 enabled 标记，按完整配置当前 enabled 值重建该布尔标记，勿把它覆盖到完整配置文件。若正式目录中标记与完整配置路径相同，应保留完整配置，旧读取器可读取其 enabled 字段。源码回滚只逆转本任务差异，不能重置整棵混有其他开发工作的工作树。本次未将混合用户修改自动提交。
