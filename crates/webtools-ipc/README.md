# webtools-ipc

Native <-> Manager 命名管道协议（v1）的 Rust 实现。只依赖 `serde` / `serde_json` /
`windows-sys`（仅 PeekNamedPipe）。

## 线上格式

```text
[u32 little-endian 长度][JSON 信封]
信封: { protocolVersion: 1, requestId: 字符串<=128, type: 字符串<=48, payload: 对象 }
最大帧: 4 MiB
```

对端（manager）回包是 `type = "ack"` 的信封，payload 为：

```json
{ "replyToId": "...", "replyToType": "...", "ok": true, "result": {}, "error": null }
```

与 `NativePipeFrameCodec.cs` 和 `native-manager-protocol.ts` 逐字节一致。

## 用法

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

**实测踩过这个坑**：hello 发出去，对端一条日志都没有，进程永久挂住。

所以这里不设读线程：`poll()` 用 `PeekNamedPipe` 先看有没有数据，
有才 `read`。读写永远在同一个线程、不会并发，也就不存在死锁。

代价是：服务端主动推送（`plugins-update` 等）不会实时唤醒调用方，
需要调用方自己用定时器 `next_event(timeout)` 拉。对启动器来说够用。

## 测试

```powershell
cargo test                                  # 编解码 / 分片 / ack 语义
node tools/fake-manager.mjs WT.Test         # 假 manager（仅测试用）
cargo run --example handshake -- WT.Test    # 握手 + 一次请求
```

`tools/fake-manager.mjs` 是**协议桩**，不是 Electron 本体；它只实现
`electron-host-ready` / `app-memory-get` / `app-memory-remember`。

## 还没做

- 双向：manager 发起的请求（`launcher-settings-update` 等）目前只会进 `backlog`
- 重连：断线后需要上层重新 `connect`
- capability 协商：现在只是透传，没有按能力裁剪请求
