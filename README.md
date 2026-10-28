# WebTools

WebTools 是面向 Windows 的启动器、选区面板和插件效率工具。Rust / Slint 宿主提供搜索框、快捷键、托盘与 Super Panel；Electron Host 按需提供 Manager、插件运行时及独立的翻译窗口。本仓库包含两部分源码，可以一起构建，无需另行下载 Manager。

项目目前处于 0.1.0 开发阶段。Rust 宿主和 Electron 的集成已有预览实现，仍需要实际 Windows 环境下的完整验收；便携包尚未提供正式安装、升级或自动更新支持。

## 功能

- **启动器**：搜索本机应用、收藏网站、插件功能及文件；使用全局快捷键呼出。
- **Super Panel**：根据选中文字提供搜索、翻译和插件操作，也支持快捷入口和 Quick Result。
- **翻译**：Translation 2.0 使用独立窗口，提供 WebTools 翻译和网站翻译页签；翻译服务和 AI 凭据通过设置配置。
- **插件平台**：管理本地插件及其权限、配置和生命周期，支持独立窗口与命令；仓库提供 SDK、脚手架及示例。
- **Manager**：管理网址、插件、设置与共享 AI。插件和翻译使用独立窗口。

第三方网页和翻译服务的可用性取决于网络和服务配置。插件开发请参阅 [开发指南](docs/plugin-development.md) 和 [脚手架说明](plugin-sdk/create-webtools-plugin/README.md)。

## 架构和目录

Rust 宿主与 Electron Host 通过认证的 Windows 命名管道协调。宿主负责原生交互，Electron 负责 Manager、插件运行时与窗口。

```text
crates/webtools-ipc/           Rust 管道协议和状态
prototype/slint-launcher/      Rust / Slint 宿主
electron/                      Electron Host、插件运行时和服务
src/                           Manager、翻译和共享前端契约
plugin-sdk/、examples/         插件 SDK、脚手架与示例
resources/                     品牌资源及 Everything 运行文件
native/WebTools.UpdateHelper/  更新助手源码
scripts/、docs/、tests/        构建工具、文档和测试
```

`docs/` 中部分阶段记录描述的是历史架构和当时的验收状态；当前入口与命令以本 README 为准。

## Windows 开发

需要 Windows x64、Rust 工具链、Visual Studio C++ 桌面开发工具、Node.js 22.7 以上和 pnpm 9.15.9。更新助手单独开发需要 .NET 10 SDK。

```powershell
pnpm install --frozen-lockfile
pnpm run typecheck
pnpm run build
cargo check --locked --workspace --all-targets
cargo run --locked -p slint-launcher
```

`pnpm run electron:dev` 可单独调试 Electron。Rust 开发模式默认使用可执行文件旁的 `data` 目录；可设置 `WT_PROFILE_ROOT` 指向隔离数据目录，或使用 `WT_CONFIG` 指定启动器 JSON 配置。配置项 `managerExe` 可指定 Manager 路径；不配置时需要 Manager 的功能不可用。

## 打包 Windows 便携预览

```powershell
pnpm run package:win
```

脚本在本仓库构建 Rust 宿主和 Electron Manager，将两者与 Everything 运行文件一起放入 `release/native-rust-preview-*`，并生成 ZIP。解压后运行 `WebTools.NativeHost.Rust.exe`。预览包使用当前 Windows 用户的 `%APPDATA%\Nook` 数据，运行前应从托盘退出已运行的 WebTools。

此命令生成便携预览，尚未提供经验证的 Rust 安装器、升级或自动更新发布流程。仓库中保留的旧安装器及发布脚本依赖此前的宿主和检查项目，不应作为本仓库的发布入口。部分历史测试也依赖此前宿主，需要迁移后才能作为完整测试套件运行。

## 许可

WebTools 项目采用 [MIT 许可证](LICENSE)。Everything 等第三方资源的来源与许可证见相应资源目录。
