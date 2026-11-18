# P5 定时任务收尾（2026-11-15）

既有 WebTools 定时任务就是平台登记、Rust 持久调度、到期唤醒插件的方案。“定时提醒”是使用样例，未新增第二套调度器。

## 对标与补齐

对照 uTools 官方 `requestSchedule/onScheduleTrigger/getSchedules/removeSchedule` 文档及本机分析产物 `newUtools/analysis/decompiled/renderer/plugins_ffffffff_index.js` 的 dT（统一列表）、lT（日志）、cT（删除确认）。用户逐项授权、一次性/Cron/起止时间、隐藏 Web 回调和 Node 执行、插件内查询删除、成功失败统计、停用和权限撤销后的暂停均已实现。

本轮新增设置 → 定时任务，展示所有插件任务的归属、规则、下次执行、状态、统计及确认删除。新增最近 30 次执行日志（时间、结果、耗时、错误），旧存储无日志仍可读取。修复暂停任务重启后错误重算下次时间、Cron 最后一次执行后仍标记启用、极大时间戳截断。另修复 SDK 声明缺少分号导致导入该类型时编译失败。

## 明确保留的差异

uTools 官方文档说明一次性任务完成后自动删除。本项目保留已经验收过的结束统计；统一页面分组显示“已结束与暂停记录”，用户可删除。Cron 到期也保留历史；历史占每插件 64 条限额，已结束记录不会继续执行。

重启沿用已有约定：周期任务计算下一次未来时间，不补跑停机期间周期；错过的一次性任务重启后尝试一次。未确认的 uTools 重启细节不宣称完全一致。

## 权限与风险

插件 API 仍为 tasks.request/list/remove/onTrigger。Manager 的 desktop.pluginTasks.list/remove 经主窗口主 frame 验证转发到既有 Rust 命名管道的 plugin-tasks-admin-list/remove。插件窗口与子 frame 无权调用；SDK 仍按插件身份隔离。删除同时匹配 pluginId 和 code，避免删掉另一插件的同名任务。删除不回滚已开始执行的副作用，同代码重建继续受执行锁限制。

日志是本地插件错误文本，每条最多 1024 字符、每任务最多 30 条；以文本渲染。新增管理页不授予插件前台手势或任意代码执行权限。Node 插件仍能以当前用户执行本地代码，Host capability 检查不等于操作系统沙盒。

## 验证

Rust 测试覆盖调度边界、旧存储兼容、日志限额、管理删除隔离；Electron 测试覆盖来源限制、非法参数拒绝与断连反馈；UI 测试覆盖状态与日志排序。真实打包脚本 verify-p5-scheduled-tasks.mjs 验证 Rust→Electron→Web/Node 执行；verify-p5-schedule-management.mjs 验证真实设置页与持久删除。

交付：`D:\SystemDefault\Desktop\webtoolstest\P5-Schedule-Management-20261115-r1`，包含完整 Rust+Electron 预览、platform-playground 1.18.0、SDK 1.15.1。自动验证结果与人工步骤见交付目录“验收报告与步骤.md”。用户已在本会话确认本轮全部验收通过，包括停用暂停与后台不抢占其他插件的行为说明。P5 总体结项依据见 [P5 结项记录](p5-completion.md)。
