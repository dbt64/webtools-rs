# webtools-ipc

Native <-> Manager 命名管道协议（v1）的 Rust 实现。生产环境由 Rust 宿主创建管道，Electron Manager 连接。

## 线上格式

```text
[u32 little-endian 长度][JSON 信封]
信封: { protocolVersion: 1, requestId: 字符串<=128, type: 字符串<=48, payload: 对象 }
最大帧: 4 MiB
```

请求的对端回包是 `type = "ack"` 的信封，payload 为：

```json
{ "replyToId": "...", "replyToType": "...", "ok": true, "result": {}, "error": null }
```

与 `NativePipeFrameCodec.cs` 和 `native-manager-protocol.ts` 逐字节一致。

## 底层客户端示例

```rust
use std::time::Duration;
use webtools_ipc::{Connection, Envelope};

let mut conn = Connection::connect("WebTools.NativeHost.Manager.v1", Duration::from_secs(10))?;
let ack = conn.request(&Envelope::new("app-memory-get", serde_json::json!({})), Duration::from_secs(5))?;
println!("{:?}", ack.into_result());
```

## 为什么是单线程轮询

Windows 的同步管道句柄在同一个 file object 上串行化 I/O。用
`try_clone` 起一个后台读线程之后，写操作会被阻塞的读卡住，表现为
「连上了、写不出去、也收不到 ack」。

当前由同一个后台线程处理读写。`poll()` 用 `PeekNamedPipe` 查询可用字节，
按可用数量分批读取，增量解码器保留跨轮询的半帧。从开始读取起 5 秒内仍未收到完整帧时，
连接返回超时，由上层处理断线恢复。

Rust 启动器每轮限制处理的管道消息数量，并给本地命令处理机会。等待下一轮的
间隔并非 IPC 往返的硬上限。写入使用 Windows 同步 I/O，当对端停止读取且管道缓冲区满时，
写操作可能阻塞。

## 测试

```powershell
cargo test                                  # 编解码 / 分片 / ack 语义
node tools/fake-manager.mjs WT.Test         # 假 manager（仅测试用）
cargo run --example handshake -- WT.Test    # 握手 + 一次请求
```

`tools/fake-manager.mjs` 是**协议桩**，不是 Electron 本体；它只实现
`electron-host-ready` / `app-memory-get` / `app-memory-remember`。

## 会话行为

- Rust `HostSession` 处理双向请求，等待回执期间也会分发收到的请求。
- 连接恢复由宿主与 Manager 的上层逻辑负责，包括重新握手和同步状态。
- 握手返回双方支持的能力交集，部分扩展（如 Quick Result、MCP）单独协商。
