# Rust MCP 网关与 Electron 按需执行 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** MCP 开启时维持同一地址与工具体验，让 Electron 在无窗口、无任务和无用户后台会话时退出。

**Architecture:** Rust NativeHost 使用官方 rmcp 承担监听、发现与请求路由，复用现有 HostRuntime 唤醒 Electron。Electron 保留插件授权、Ajv 校验和 Node 执行；Manager 沿用现有 AI 设置入口和 Agent 配置逻辑。

**Tech Stack:** Windows、Rust/Slint、rmcp/其必需 HTTP 异步依赖、现有 Named Pipe、Electron 44.4.5、TypeScript/Vue、Ajv、TS MCP SDK 1.32.2 验证客户端。

**Spec:** [重构设计及前端契约](../specs/2026-11-17-native-mcp-gateway-design.md)。用户已选择方案 A，并授权修订、审核无阻塞后连续开发；不实施或评估方案 B。任务状态以实际验证记录为准。

**执行状态（2026-11-17）：** Task 1–6 源码已实现，计划审核及代码审核修正已完成，定向测试、工作区测试与 r2 构建通过；Task 7 的打包、真实托管自动验收和资源对比已完成，余下人工体验项见验证记录。勾选表示相应实现/自动检查已执行；各项“提交”统一延期，原因是工作树有 182 项既有混合修改，未自动暂存或提交用户工作。完整产品交互、DPI 与真实 Agent 客户端仍按 [验证记录](../../mcp-native-gateway-validation.md) 标人工待验收，不把预览交付写成全部验收通过。

## Global Constraints

- 改造目标目录为 `D:\SystemDefault\Desktop\webtools-rs-plugin-platform-work`；基线是 `f5a9073` 加现有工作树，执行前重新记录差异及受影响文件哈希；保护已有未提交修改。
- 前端沿用“Manager → 设置 → AI”；保留分组工具开关、连接弹窗、完整 JSON、一键配置及备份；Host 控件使用现有主题令牌。
- 规范设置沿用实际 Manager userData：正式模式为 Native profile 根目录，`--phase4g-manager-test` 模式为其 `runtime/wt-manager-profile`；Rust 唯一写入，实际运行目录内已有完整配置优先，正式模式忽略旧嵌套测试配置。
- 默认端口 24459，端口范围 1024–65535，64 KiB HTTP 请求体，4 MiB Pipe 帧；保留原 URL/token/disabledPlugins。
- 最多接纳 8 个 MCP 调用（等待和执行合计）；总预算初值 60 秒，现有 Node 执行期限 30 秒，body 接收期限 15 秒；冷却初值 15 秒。
- 已投递且结果不明的调用不自动重放；取消等待者不等于运行结束；完成后先取得冷却 owner 再释放请求 owner。无会话共享端点中，不能确定归属的跨 POST 取消通知不得按全局 JSON-RPC id 查找任务。
- 插件授权、Ajv、存储与 `gesture=false` 留在 Electron；开发会话不发布为 MCP 工具；SDK/生成器当前 1.16.1，不强制插件升级。
- 正式切换后仅 Rust 监听；无新常驻 Node 进程、无新增 1–2ms 忙轮询、无后台抢焦点；不改 P5/P6 的历史验收状态。
- 所有动态验证在隔离 profile 和输出目录运行；不得改写真实 Agent 配置或重启用户的 WebTools 来跑自动检查。

## Review Focus

1. 两处旧配置冲突或损坏时，保留规范位置完整配置和原令牌，不能被 boolean marker 覆盖。归属 Task 2。
2. MCP ack 在旧同步 `request()` 等待期间到达时，仍能回到自己的请求，不能被 Quick Result 分支吞掉。归属 Task 4。
3. 客户端超时/取消后插件仍在退出时，保留并发槽位与 owner；迟到结果只结算一次。归属 Task 3/4。
4. 冷却到期恰遇新调用或 UI/定时任务时，不误退出；断连不重放有副作用调用。归属 Task 4。
5. 保存 enabled=true 但监听失败、旧页面迟到响应和插件级关闭时，开关、错误、分组与重试仍表达真实状态。归属 Task 6。

## 实施顺序与文件分工

Task 1 验证协议与基线 → Task 2 数据/内部契约 → Task 3 Electron 执行 → Task 4 非阻塞调度 → Task 5 网关实现与隔离验证 → Task 6 前端适配并统一切换 → Task 7 集成与交付。Task 1–5 保持旧生产监听与配置写入路径；新网关仅在独立隔离 profile 验证。Task 6 同批切换公共状态类型、设置写权、管理接口、监听、bootstrap 和退出判据；任一实例只有一个端口拥有者。每项是一个可审阅、可定向验证的交付，具体补丁提交仅暂存本任务实际修改的文件/区块。

Task 1 后核对 SDK 协议兼容和基线是否可比较；Task 4 后核对 ack 路由、非阻塞所有权与既有交互能否共存。未通过时修复具体问题，保留方案 A 和现有正式入口，不新增架构备选或逐阶段审批。

| 位置 | 职责 |
| --- | --- |
| `crates/webtools-ipc/src/mcp.rs`（新增） | 跨语言消息、投影、结果结构与边界校验 |
| `crates/webtools-ipc/src/mcp_store.rs`（新增） | 配置迁移、规范路径、原子存储与快照 |
| `prototype/slint-launcher/src/mcp_server.rs`（新增） | rmcp HTTP 入口与监听状态，不写插件逻辑 |
| `prototype/slint-launcher/src/mcp_dispatch.rs`（新增） | 有界请求状态、截止时间、取消与冷却决策 |
| `host_runtime.rs`、`session.rs` | 挂接现有启动/所有权/收包循环，保持单一 Pipe 读写权威 |
| `electron/plugins/mcp-tool-execution.ts`（新增） | 从旧 service 提取实时授权和执行，不持有监听器 |
| `electron/services/native-mcp-gateway.ts`（新增） | Electron 侧能力协商、投影与 MCP 命令适配 |
| `electron/ipc/plugin-mcp-handlers.ts`（新增） | 现有 Manager MCP 方法转发、来源限制和 Agent 配置 |
| `src/shared/plugin-mcp-contracts.ts`（新增） | Manager/主进程共享状态与结果类型 |
| `PluginMcpPanel.vue`、`plugin-mcp-view-model.ts`（新增） | 原页面状态映射与操作反馈，复用现有外观 |

## Task 1：锁定兼容基线，验证 rmcp 入口

**Files:** 读 `package.json`、`Cargo.toml`、两个 Cargo 包清单、现有 P5/P6 文档和 `scripts/verify-p5-{builtin-mcp,mcp-bootstrap}.mjs`；修改 `prototype/slint-launcher/Cargo.toml`、`Cargo.lock`；新增 `prototype/slint-launcher/examples/mcp_gateway_probe.rs`、`scripts/verify-mcp-gateway-protocol.mjs`、`docs/mcp-native-gateway-validation.md`。

**Interfaces:** 验证脚本使用 `--url <URL> --token <testToken>` 连接隔离 probe；probe 提供 `probe.echo`，输入/输出均 `{text:string}`。依赖版本、feature、支持的协商版本及命令写入验证记录，后续网关复用同一锁定组合。

- [x] **1. 记录基线。** 在独立输出目录保存 git HEAD/status、受影响文件 SHA-256、现有端口/配置路径规则、当前 fixture 的实际版本。查看现有源码脚本参数，不把旧报告的固定样例名当当前包名。
- [x] **2. 先定义协议验证断言。** `initialize/listTools/callTool` 成功；echo 正确；重复 JSON-RPC id 来自两个客户端不串结果；发送无法确定客户端归属的同号独立取消通知不取消任何一方；通知、Accept/版本头、错误 JSON、不支持方法按目标协议响应。探针未运行时验证脚本须非零退出，不把连接失败当协议通过。
- [x] **3. 在隔离 probe 选择并锁定 rmcp 稳定版和所需依赖。** 采用无状态 JSON 响应模式，接入同等本机 Host/Origin/Bearer/body 限制。当前 TS SDK 客户端连通后，将所选版本与对应稳定 tag 文档链接记入验证记录；不安装全套可选 SDK 能力。
- [x] **4. 执行兼容门槛。** `cargo check --locked -p slint-launcher --example mcp_gateway_probe`；运行 probe 后 `node --experimental-transform-types scripts/verify-mcp-gateway-protocol.mjs --url <probeUrl> --token <testToken>`。预期实际用例数大于 0 且全部通过。兼容失败修复 rmcp 选版/设置；未解决前不切换正式路径，不扩写手工协议替代物。
- [x] **5. 记录旧入口可比较的基线。** 仅在隔离副本采样 MCP 开/关、列表、冷/热调用、进程组内存与空闲 CPU；没有可运行旧构建时标记该项缺测，不声称比较完成。提交本项新增内容和锁文件差异。

## Task 2：建立设置唯一写者、工具快照与共享契约

**Files:** 新增 `crates/webtools-ipc/src/mcp.rs`、`mcp_store.rs`、`tests/mcp_contract.rs`、`tests/mcp_store.rs`、`src/shared/plugin-mcp-contracts.ts`、`src/shared/plugin-mcp-contracts.test.mjs`；修改 `crates/webtools-ipc/src/lib.rs`。旧 Electron 设置实现和 ipc.ts 公共类型暂留至 Task 6。

**Interfaces:** `mcp_profile_path(native_root:&Path)->PathBuf` 返回既有隔离 Manager profile；生产接线依据启动参数显式选择实际目录。`McpSettingsStore::load_at_profile(native_root:&Path,manager_profile:&Path)->Result<Self>`；`load_or_migrate` 是隔离目录便利入口；`set_enabled(bool)`、`set_plugin_enabled(&str,bool)`、`save()`；`McpProjectionStore::load(path:PathBuf)->Result<Self>`、`replace_from(generation:u64,McpToolsProjection)->Result<()>`、`snapshot()->Option<McpToolsProjection>`。Rust 调用结果是 `McpCallReply`（completed、cancelled、failed、outcome-unknown 四种），各携带 callId，completed 含 MCP result；请求含 callId/toolName/arguments/deadlineUnixMs。TS 先定义内部 `McpGatewayStatus`（设计第 5.3 节的新状态），不替换现有 `PluginMcpStatus`；后者到 Task 6 连同所有调用方一起迁移。

- [x] **1. 写数据驱动失败测试。** 用临时目录覆盖规范配置优先、根目录完整旧配置迁移、两份冲突、仅 marker、无文件、坏文件、旧 token 原样保留、disabledPlugins 保留、保存失败。投影覆盖合法空集、损坏、旧连接迟到 revision、超出 4 MiB 帧上限且保留上次快照。TS 测试覆盖 enabled=true/running=false 的合法错误状态与非法 running=true 缺凭据。
- [x] **2. 验证失败。** `cargo test --locked -p webtools-ipc --test mcp_contract --test mcp_store`；`node --experimental-transform-types --test src/shared/plugin-mcp-contracts.test.mjs`。预期新模块尚缺或断言失败。
- [x] **3. 实现类型、规范路径与原子存储。** 完整旧配置位置不移动；迁移保留原文件；损坏文件返回配置错误。工具 input/output schema 作为 JSON 数据保留，执行校验仍属于 Electron。大投影本期返回明确同步过大错误，不擅自抬高现有 Pipe 限制。
- [x] **4. 复跑两组测试与 `pnpm run typecheck`。** 预期相关测试真实执行且通过。此项只建立类型和存储，不改变生产端口拥有者。提交本项差异。

## Task 3：提取 Electron MCP 执行模块与投影适配

**Files:** 新增 `electron/plugins/mcp-tool-execution.ts`、`mcp-tool-execution.test.mjs`、`electron/services/native-mcp-gateway.ts`、`native-mcp-gateway.test.mjs`；修改 `electron/plugins/plugin-mcp-service.ts`、`electron/services/native-manager-client.ts`、`electron/main.ts`；必要时扩展 `electron/plugins/node-command-execution.test.mjs`。

**Interfaces:** `McpToolExecution(manager,commands,isMcpPluginEnabled)` 提供 `execute(request:McpCallRequest):Promise<McpCallReply>`、`cancel(callId:string):void`、`close():void`。同一 callId 同代次最多执行一次；取消控制器由模块持有直到实际执行结束。`NativeMcpGateway(client,execution)` 提供 `publishTools(tools:McpToolProjection[]):Promise<void>`、`handleCommand(message):Promise<NativeManagerCommandReply|undefined>`、`close():void`，只在协商 `plugin-mcp-gateway-v1` 后挂接。

- [x] **1. 写行为失败测试。** 安装测试插件经真实 PluginManager 授权后调用；无效输入不产生写入、输出不符返回错误、停用/撤权后拒绝；两个不同 callId 同工具不串结果；取消信号传给 Node 执行器且终态只结算一次；迟到旧连接命令拒绝；开发会话不进入发布快照。调用继续无前台手势。
- [x] **2. 运行新测试确认失败。** `node --experimental-transform-types --test electron/plugins/mcp-tool-execution.test.mjs electron/services/native-mcp-gateway.test.mjs`。
- [x] **3. 从现有 callTool 提取执行逻辑。** 保留 `authorizeWebFeature(...,'mcp')`、Ajv、authority.revoke 和既有存储；旧 PluginMcpService 暂时调用同一模块，避免两个执行实现。Native adapter 接入现有命令回调和 feature authority changed 订阅，在初始化完成、权限/安装状态变化及重连后发布完整快照。
- [x] **4. 补齐帧大小/结果处理。** 发送前检查 UTF-8 编码帧大小；不可发送的结果转换为有界工具错误，不能靠切断 Pipe 影响其他工作。原有 Node 输出与错误映射保持，不把 SDK result 藏入只有 HTTP 层能理解的对象。
- [x] **5. 复跑新测试、既有 Node 命令测试及类型检查。** `node --experimental-transform-types --test electron/plugins/mcp-tool-execution.test.mjs electron/services/native-mcp-gateway.test.mjs electron/plugins/node-command-execution.test.mjs`；`pnpm run typecheck`。生产仍用旧监听器。提交本项差异。

## Task 4：打通不阻塞现有交互的 MCP 调度与所有权

**Files:** 新增 `prototype/slint-launcher/src/mcp_dispatch.rs`、`mcp_dispatch_tests.rs`、`host_runtime_mcp_tests.rs`、`crates/webtools-ipc/tests/mcp_session.rs`；修改 `prototype/slint-launcher/src/main.rs`、`host_runtime.rs`、`crates/webtools-ipc/src/session.rs`。

**Interfaces:** `HostCommand::McpCall{request,reply_tx}`、`McpCancel{call_id}`、`McpSync{reply_tx}`；`McpDispatch::admit(request,now)->Result<CallTicket,McpAdmissionError>`、`on_ready(generation)`、`on_reply(reply,generation,now)`、`cancel(call_id,reason)`、`on_disconnect(generation)`、`tick(now)`。该状态机返回待执行效果，由已有 supervisor 执行 Pipe/owner 操作；不拥有第二个 Pipe 读取线程。`HostSession::begin_mcp_call(&McpCallRequest)`、`begin_mcp_cancel(&str)`、`take_mcp_events()` 共用按 envelope id/type 路由的 ack 入口，所有权确认也是异步阶段。

- [x] **1. 写状态机失败测试。** 同时接纳 8 个、第 9 个拒绝；启动合并；ready 前零投递；owner 确认失败零执行；取消未发送者零执行；执行中取消未确认仍占槽/owner；超时后迟到结果只释放一次；投递后断连结果未知、不重放；旧代次 ack 无效。
- [x] **2. 写真实 Pipe 回归测试。** 伪 Manager 延迟 MCP 结果，同时能处理其他管理命令；在旧 `request()` 等待期间插入 MCP ack，随后取到正确结果；等待 MCP owner 确认时仍可处理取消/设置；分别断连于未写入、部分写入和已发送阶段。新用例接入测试模块，不能只新增未被编译的文件。
- [x] **3. 运行测试确认失败。** `cargo test --locked -p webtools-ipc --test mcp_session`；`cargo test --locked -p slint-launcher mcp_dispatch`；需要启动伪 Manager 的测试按既有 ignored 模式执行 `cargo test --locked -p slint-launcher host_runtime_mcp -- --ignored`。输出必须有真实命中用例。
- [x] **4. 实现增量调度。** 保留现有 UI/定时任务入口；MCP 不进入会自动 clone 重放 command 的通用 pending 分支；统一 pump/request 的异步 ack 路由。利用既有通道和有界等待推进事件，不给整个应用换异步框架。
- [x] **5. 写并实现冷却竞态测试。** 最后任务完成时先保留冷却；到期与新请求同刻时保留新请求；旧定时器无权释放新冷却；UI/定时任务 owner 存在时只释放 MCP owner；关闭 MCP 不关闭其他工作。测试注入时钟/冷却值，不让每个用例真实等待 15 秒。
- [x] **6. 复跑本项全部测试和已有 readiness/lifecycle 定向用例。** 测试将 MCP 结果保持未返回，先断言取消/设置/既有面板命令已被处理，再释放结果；这样证明响应不依赖工具完成。确认不新增第二个 Pipe 读者、不改写无关调度模块、无错误重放和 owner 泄漏后通过本阶段复核。提交本项差异。

## Task 5：实现 Rust 网关，在隔离环境验证

**Files:** 新增 `prototype/slint-launcher/src/mcp_server.rs`、`mcp_server_tests.rs`；修改 `main.rs`、`host_runtime.rs`、`crates/webtools-ipc/src/session.rs`、`electron/services/native-mcp-gateway.ts` 的新能力接线；生产 app.rs 启动与旧 Electron 监听不在本任务切换。

**Interfaces:** `McpServerHandle::start(profile,dispatch)->Result<Self>`、`status()->McpGatewayStatus`、`set_enabled(enabled)->Result<McpGatewayStatus>`、`set_plugin_enabled(plugin_id,enabled)->Result<()>`、`stop()`。服务 handle/设置只由 Rust 管理；admin 请求通过现有 session 分派到该 handle。HTTP handler 提交任务后异步等 reply，不持有 Slint/UI 锁或跨等待持有设置锁。

- [x] **1. 写网关失败测试。** 本机鉴权/Host/Origin/body 边界；相同端口绑定失败保留 saved enabled；再次 true 重试；设置保存失败不报成功；有效快照 tools/list 不启动；无快照合并一次同步、有效空数组不重启、同步失败返回错误；MCP 级关闭影响发现与调用，但不全局停用插件。
- [x] **2. 验证失败后接入 Task 1 锁定 SDK 与 Task 2/4 模块。** 监听异常保留 Launcher/托盘可用；协议格式错误交 SDK，业务执行错误返回 isError。仅原请求上下文的取消/断开、内部超时及服务关闭映射到自己的 callId；无归属的独立取消通知不能取消任务。关闭服务先禁止新请求、请求取消执行任务，再按真实终态释放资源。
- [x] **3. 只在隔离验收实例接入新路径。** 新能力与 admin 处理通过显式测试依赖或既有验收入口选择；该实例禁止旧 Electron 监听和设置写入，profile 与端口独立。普通运行继续旧路径，不新增产品配置开关、自动 fallback 或双写。
- [x] **4. 验证故障下宿主可用。** 端口占用、畸形请求、长期未返回工具、Electron 崩溃，以及注入到受监督 MCP 任务的可展开 panic 后，仍可处理 Launcher/托盘/热键事件；未完成等待者收到明确终态，owner 不遗留。记录任务级 panic 的处理位置，不将其描述为能隔离进程级 abort/OOM；不新增通用容错框架。
- [x] **5. 运行 Rust 网关/调度/存储定向测试、Task 1 协议脚本、Electron 模块测试和 `pnpm run typecheck`。** 同时确认普通入口的现有设置页和监听仍可用。通过后提交网关实现，正式切换留到 Task 6。

## Task 6：适配原有 AI 设置页，统一切换前后端

**Files:** 新增 `electron/ipc/plugin-mcp-handlers.ts`、`plugin-mcp-handlers.test.mjs`、`src/features/plugins/plugin-mcp-view-model.ts`、`plugin-mcp-view-model.test.mjs`；修改 `electron/main.ts`、`src/features/plugins/PluginMcpPanel.vue`、`src/shared/ipc.ts`、`prototype/slint-launcher/src/app.rs` 及对应生命周期测试；仅在类型需要时修改 `electron/preload.ts`。`mcp-agent-config.ts` 的配置合并与备份行为保留。

**Interfaces:** `registerPluginMcpIpc(ipc,deps):void`，deps 为 Manager frame 校验、native request 和现有 McpAgentConfig；保留 desktop.pluginMcp 方法名/参数。`toMcpPanelState(status,loadError):McpPanelState` 产生开关值、状态文案、错误和可用动作；`McpPanelState` 定义于同一 view-model。文案与动作严格对应设计第 4.2 节。重试调用 `setEnabled(true)`，开关调用 `setEnabled(!status.enabled)`。

- [x] **1. 写管理和 UI 失败测试。** 非 Manager/子 frame 拒绝；错误 IpcResult 不更新成功态；断连给可恢复错误；enabled=true/running=false 开关仍为开；重试同 true；按实际端口报错；初始加载失败不是 disabled；MCP 关闭插件分组仍保留；旧页面 load 响应不能覆盖新操作结果；失败按钮恢复可用。
- [x] **2. 执行失败验证。** `node --experimental-transform-types --test electron/ipc/plugin-mcp-handlers.test.mjs src/features/plugins/plugin-mcp-view-model.test.mjs`。
- [x] **3. 实现原区域适配。** UI 不展示进程细节；保留分组数量、展开、刷新、手动 JSON 与 Agent 按钮。`get/preferences` 来自 Rust，`listTools` 使用正式工具投影的未按 MCP 开关过滤视图；连接参数以当前 Rust 状态为准。读取/显式刷新/操作后/页面重新激活时刷新，不增加高频轮询。
- [x] **4. 完善现有弹窗键盘和错误恢复。** 焦点进入弹窗、Tab 留在模态区域、Esc 关闭后回到连接按钮；一键配置只有用户点击才写入且先备份；“已配置”不冒充在线。颜色/边距/控件沿用 Host 页面，不引入 UI 框架或强制修改插件 CSS。
- [x] **5. 同批切换完整链路。** ipc.ts 将 `PluginMcpStatus` 重导出为内部新状态并更新全部初始化/调用方；Rust 从规范设置启动网关成为唯一配置写者；移除 `mcp-bootstrap`、Electron 自动监听和 MCP listener 保活判据；Manager 管理方法转到 Rust。去掉 Task 5 临时测试路径选择及旧生产监听接线，保留版本回滚办法。此补丁必须同时具备可用设置页、正确监听与空闲退出，不交付半切换状态。
- [ ] **6. 联动验收后提交。** 新管理/UI 测试、仍适用的 Agent 合并/备份用例、类型检查及生命周期用例均通过：已有快照托盘启动不拉 Electron、关闭 Manager 仍监听、调用冷却后退出、UI/定时任务不被误关、退出托盘关闭端口。老设置写入测试迁移至 Rust，保留格式错误与其他 Agent 配置不被覆盖断言。

## Task 7：真实托管集成、体验与资源验收，文档收尾

**Files:** 修改 `scripts/verify-p5-builtin-mcp.mjs`、`verify-p5-mcp-bootstrap.mjs`；新增 `scripts/verify-mcp-native-lifecycle.mjs`、`scripts/measure-mcp-native-gateway.ps1`；按需扩展现有进程采样脚本；更新 `docs/p5-builtin-mcp-agent-connection.md`、`docs/plugin-development.md`、`docs/mcp-native-gateway-validation.md`。只有相关旧描述受影响才更新主题/P6 文档；插件平台主计划只加本重构链接，不改勾选状态。

**Interfaces:** 所有改造后的 MCP 验收脚本统一接受 `--delivery <absoluteDir> --plugin-package <absoluteWtplugin>`，自行在隔离目录创建 profile 与测试 token，运行 `stage/WebTools.NativeHost.Rust.exe`；不再用独立 Electron 入口冒充 Rust 托管验收。资源脚本参数为 `-Delivery <absoluteDir> -BaselineDelivery <oldAbsoluteDir> -PluginPackage <absoluteWtplugin> [-WindowSeconds 5]`，内部复用现有采样约束并使用独立临时根，证据保存到 delivery/verification。

- [x] **1. 写集成断言后更新脚本。** 设计 A1–A9 逐项关联到自动证据或人工步骤；无快照首次同步与已有快照启动分开。覆盖 list/call、8+1 并发、半途取消、执行后故意断连、插件/权限切换、旧数据、关闭 Manager、关闭总开关、退出托盘以及 50 轮回收。断连例使用可观察计数器，证明未自动重复副作用。
- [x] **2. 执行源码验证。** `pnpm run typecheck`、`pnpm run toolchain:check`、`pnpm run build`；`cargo test --locked --workspace`，并显式执行本任务 ignored 集成测试。运行 Task 2/3/6 列出的 Node 定向测试。零用例不算通过；发现既有失败记录与本任务关系，不扩大成全仓重构。
- [x] **3. 打包独立预览。** 在 PowerShell 7 执行 `pnpm run package:rust-preview -- -OutputDirectory 'D:\SystemDefault\Desktop\webtoolstest\MCP-Native-Gateway-20261117-r1'`（执行当天如日期变化改目录，存在则用新 r 序号，禁止覆盖）。使用当前有效的 platform-playground 包复制进交付目录，保留实际版本文件名。无需无关 SDK 重发。
- [x] **4. 运行真实托管脚本。** `node --experimental-transform-types scripts/verify-p5-builtin-mcp.mjs --delivery <delivery> --plugin-package <fixture>`，bootstrap 与 native-lifecycle 脚本使用同样参数。预期每个断言记录结果、清理只限本轮子进程和隔离路径；不操作用户运行实例。
- [ ] **5. 检查实际产品交互。** 打包版 AI 设置初始/关闭/运行/占用端口/断连/重试、开关和插件分组；深/浅/系统主题、100%/150% DPI、窄窗口、长名称、键盘和连接弹窗。并发 MCP 时操作普通插件失焦/恢复/Esc、分离窗口、Translation、Super Panel 翻译按钮、子输入/IME；穿插到期定时任务。P6 开发连接做一次热重载/重连/上下文模拟定向回归，不重复整套模板矩阵。截图及操作记录附报告，未操作部分标人工待验收。
- [ ] **6. 资源和客户端验收。** 同机同口径比较 MCP 关、旧入口空闲、新入口空闲、列表、冷/热调用、最终冷却退出；至少 20 次冷调用记录 P50/P95。每种空闲场景做 3 个等长采样窗口，报告整个进程组 Private Bytes 中位数及跨窗口范围：新实现每个窗口中位数均低于旧实现最小窗口中位数，才确认收益超过本次观察波动；未达到则定位并修复，不以进程数减少替代收益。Working Set、句柄数与空闲 CPU 同时报告；CPU 超出原生纯空闲观察范围须解释或修复。无其他窗口/任务/后台会话且真实 15 秒冷却结束后 Electron/UtilityProcess 必须为 0；50 轮无持续增长或残留。已有配置的 Codex/Cursor/Claude 客户端逐个验证，未运行者标未测，不主动改真实配置。
- [x] **7. 同步说明与交付记录。** P5 MCP 文档将旧“Electron 监听并常驻”改为新运行模型，说明普通用户使用方式不变、首次快照、错误恢复和升级保留。中文《验收报告与步骤.md》列出构建指纹、fixture、自动结果、人工步骤、资源数据和已知限制。按设计第 9 节记录代码/marker/配置回滚办法；提交文档与本项脚本。

## 交付判定与后续执行

只有设计 A1–A10 有明确证据且相关问题解决后，才标记本重构完成；源码编译通过、旧历史测试或文档已写完均不能替代真实链路与前端验收。前端人工未测则交付可测预览并明确该状态，不写“全部验收通过”。

用户已授权本轮修订、审核后连续实施。任务紧密依赖同一 Pipe/owner 契约，由当前会话顺序实施并记录进度，不同时让多个实现者改 `main.ts`、`host_runtime.rs` 和 `session.rs`。阶段测试通过即可继续，不增加逐轮审批。实施前读两份文档并检查工作树是否已被 super panel 聊天继续更新。
