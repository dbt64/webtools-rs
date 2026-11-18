# P6 第一项：开发工具版本与兼容

本轮交付 SDK 与 create-webtools-plugin 1.16.0。两者使用同一配套版本号；已有插件本身的 version、Manifest、Plugin API major 和宿主产品版本保持独立。

## 版本来源与维护

SDK package.json 是开发工具发布版本来源。runtime-target.json 是经过验证的宿主目标（产品 0.1.0、Electron 44.4.5、win32/x64、Node ABI 149）。生成器携带独立的 src/toolchain.json 快照，不读取 WebTools 源码或依赖仓库目录。

在仓库运行 `node scripts/sync-plugin-toolchain.mjs` 同步生成器版本、开发 Node 要求与目标快照；`node scripts/sync-plugin-toolchain.mjs --check` 拒绝 SDK/生成器/已安装 Electron 目标不一致的发布。生成的新项目记录 webtools-toolchain.json，并精确引用 SDK 发布版本。SDK 内的旧 Manifest v1 创建命令也从自己安装的 package.json 读取当前版本。

## 兼容矩阵

| 项目 | 含义 |
| --- | --- |
| 开发 Node >=22.12 | 在作者电脑运行生成器、TypeScript 和 Vite，与插件运行时 Node 不同 |
| SDK/生成器 1.16.0 | 本次成套开发工具版本；以本地 tgz 提供，尚未发布 npm |
| Manifest v1 / v3 | v1 为宿主渲染的声明式插件，v3 为 Web/Node 插件；旧已安装插件不强制迁移 |
| Manifest v2 | 只用于预检诊断，不作为生产插件安装 |
| Plugin API major 1 | API 主版本，不随 SDK minor 或模板变更自动增加 |
| wtplugin packageVersion 1 | 插件包格式，与 npm tgz 和插件自身版本不同 |
| minHostVersion 0.1.0 | 产品最低版本；不能单凭相同产品版本推断可选 API 都存在，插件仍需能力声明及可用性检查 |
| Electron 44.4.5 / ABI 149 | 仅带原生 .node 扩展时需要对目标 Electron 重编译并声明 compatibility.nodeAbi |

纯 JavaScript 的 Node 插件模板不再写 nodeAbi；它并不依赖原生 ABI。Web 插件将 SDK 与前端依赖用于构建；Node 模板 preload 直接依赖 SDK，因此 SDK 保留为生产依赖，打包时随插件携带。插件安装时仍由宿主校验清单、依赖、平台和权限。

## 本地使用

在一个仓库外的工具目录先安装配套生成器：

```powershell
npm install --save-dev "D:\SystemDefault\Desktop\webtoolstest\P6-Toolchain-20261116-r1\artifacts\create-webtools-plugin-1.16.0.tgz"
npx --no-install create-webtools-plugin my-plugin --framework vanilla --runtime web --package-manager npm --yes
cd my-plugin
npm install --save-dev "D:\SystemDefault\Desktop\webtoolstest\P6-Toolchain-20261116-r1\artifacts\webtools-plugin-sdk-1.16.0.tgz"
npm run typecheck
npm run build
npm run validate
npm run pack
npm run inspect
```

Node 模板选择 `--runtime node`，安装 SDK 时改为 `--save-prod`。pnpm 用户同样先安装本地生成器再用 `pnpm exec create-webtools-plugin`；`pnpm run pack` 执行项目脚本，而 `pnpm pack` 产生 npm 包 tgz，两者不同。首次安装前端构建依赖需要访问 npm；本次未声称提供完整离线依赖镜像。

## 验证与阶段边界

新增失败回归先复现旧 SDK 引用，修复后验证四框架 × 两运行时的版本/依赖/ABI。真实验收脚本 `scripts/verify-p6-toolchain.mjs` 从两个 tgz 安装生成器和 SDK，在仓库外逐一创建、安装依赖、检查类型、构建、验证、打包、检查归档，并通过 PluginManager 实际安装校验。报告记录每个组合的结果。

本轮只完成 P6 第一项和与它直接相关的模板修复。没有将八种模板的构建通过描述成热重载、Launcher/面板上下文模拟或真实窗口交互已经全部完成；这些仍按 P6 后续条目继续。宿主行为未修改，继续使用 P5 已验收的完整 Rust+Electron 包。
