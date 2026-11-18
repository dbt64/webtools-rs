# P5 浏览器与媒体工作流交付

2026-11-15。总体计划的浏览器自动化、图像/音视频接口与示例条目。

实现：沿用 Electron Web capability bridge、Manifest v3、运行实例 authority 和撤销信号。Rust 继续负责 Launcher/托盘/入口，无新增 Rust IPC 消息。SDK 1.15.0、platform-playground 1.18.0。具体 API、使用场景、退出行为和限制见 `plugin-sdk/declarative-v1/docs/p5-browser-media.md`。

浏览器提供可视独立窗口与固定步骤，处理 HTTP(S)、输入、点击、文本和截图；图像提供 PNG/JPEG 输出和缩放；媒体提供用户选择目标后的 WAV/MP3/MP4/WebM 转换，宿主内置 FFmpeg。插件可继续使用 Node 依赖扩展算法。

## 验证证据

- TypeScript 类型检查；新接口和原宿主 bridge 针对性测试。
- 真实 FFmpeg WAV→MP3→WAV；失败和预取消保留已有目标，临时文件清理。
- Electron 集成验证：连续三个浏览器实例、填写/点击/读取、跨运行实例拒绝、撤销信号关闭窗口、极端长宽比拒绝、删除网页登录数据。
- 完整打包插件公共 SDK 路径：真实浏览器操作与截图、640×400 PNG→320×200 JPEG、Node preload 生成 WAV、显式关闭浏览器。
- 预览包内 FFmpeg 实际生成 MP4 并转 WebM。

脚本：`scripts/verify-p5-browser-media.mjs`、`scripts/verify-p5-workflow-effects.cjs`；具体结果 JSON 与日志随交付放在 webtoolstest/P5-Browser-Media-20261115-r2。自动环境采用隔离用户数据，不改真实插件和 Agent 配置。系统保存对话框、真实 Launcher 召回和播放体验留给用户人工验收，不由 CDP 代替结论。

## 范围与后续

这是 P5 对应条目的基础可运行实现；P5 示例覆盖还需结合各项用户验收收尾。没有把完整 uBrowser 方法集合、Sharp 全部 API 或任意 FFmpeg 参数描述成已实现。未开始 P6/P7。

2026-11-15 用户已确认本轮任务按验收流程测试通过。此确认仅覆盖本轮浏览器与媒体任务，不自动标记 P5 全部历史条目完成。
