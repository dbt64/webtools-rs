# WebTools Rust

WebTools Windows 原生宿主的 Rust / Slint 重构项目，目前处于开发和集成预览阶段。项目包含启动器、搜索、托盘、Super Panel 和与现有 Electron Manager 通信的实现；插件窗口与完整管理界面仍依赖单独提供的 Manager。本仓库不包含 Manager、安装器或完整发布包，也尚未完成正式替换及真实用户环境验收。

## 目录

- `prototype/slint-launcher/`：Slint 界面、Windows 宿主和相关资源。
- `crates/webtools-ipc/`：与 Manager 通信的命名管道协议和状态实现。
- `fixtures/`：开发和测试使用的应用目录样例。
- `resources/everything/`：文件搜索所用的 Everything 运行文件及其许可和来源说明。

## Windows 构建

安装 Rust 工具链和 Visual Studio 的 C++ 桌面开发工具，然后在 PowerShell 中执行：

```powershell
cargo check --locked --workspace --all-targets
cargo build --locked -p slint-launcher
cargo run --locked -p slint-launcher
```

默认开发模式使用可执行文件旁的 `data` 目录。可通过 `WT_PROFILE_ROOT` 指定隔离数据目录；`WT_CONFIG` 可指定启动器 JSON 配置路径。未提供配置时使用默认值；若要连接 Manager，可在配置中设置 `managerExe` 为兼容 Manager 可执行文件的路径。开发模式使用隔离的 Manager 启动参数。

集成构建可使用 `cargo build --locked --release -p slint-launcher --features production`。该模式默认读取 `%APPDATA%\Nook`，并在可执行文件旁寻找 `Manager\WebTools.exe`；Manager 需要另行构建和提供。设置 `WT_PROFILE_ROOT` 可使用隔离数据。完整安装和打包仍由现有 WebTools 主项目负责。

## 许可

项目使用 [MIT 许可证](LICENSE)。第三方资源的许可见各资源目录。
