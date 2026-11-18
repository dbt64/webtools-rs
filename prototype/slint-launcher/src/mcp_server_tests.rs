use super::*;
use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
};
use webtools_ipc::mcp_store::mcp_profile_path;

#[test]
fn mcp_server_worker_panic_reports_failure_and_management_can_retry() {
    let (_profile, server, _rx) = setup();
    server.state.panic_on_start.store(true, Ordering::SeqCst);
    let _ = server.set_enabled(true);
    let end = Instant::now() + Duration::from_secs(2);
    while server.status()["phase"] != "error" && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        server.operation("mcp-admin-get-v1", &json!({})).unwrap()["phase"],
        "error"
    );
    assert!(server.set_enabled(true).is_ok());
    assert_eq!(server.status()["running"], true);
    server.stop();
}
struct Profile(std::path::PathBuf);
impl Profile {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("wt-mcp-server-{}", webtools_ipc::new_request_id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn setup() -> (
    Profile,
    Arc<McpServerHandle>,
    std::sync::mpsc::Receiver<HostCommand>,
) {
    let profile = Profile::new();
    let mut store = McpSettingsStore::load_or_migrate(&profile.0).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    store.value.port = listener.local_addr().unwrap().port();
    drop(listener);
    store.save().unwrap();
    let owner = webtools_ipc::HostOwnership::default();
    let dispatch = Arc::new(Mutex::new(McpDispatch::new(owner, Duration::from_secs(15))));
    let (tx, rx) = std::sync::mpsc::channel();
    let server =
        McpServerHandle::new(&profile.0, &mcp_profile_path(&profile.0), dispatch, tx).unwrap();
    (profile, server, rx)
}
fn post(server: &McpServerHandle, body: Value) -> String {
    let config = server.state.settings.lock().unwrap().value.clone();
    let mut socket = TcpStream::connect(("127.0.0.1", config.port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let text = body.to_string();
    write!(socket,"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: 2025-11-25\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",config.port,config.token,text.len(),text).unwrap();
    let mut response = String::new();
    socket.read_to_string(&mut response).unwrap();
    response
}
#[test]
fn mcp_server_empty_snapshot_lists_without_waking_electron_and_restart_preserves_settings() {
    let (_profile, server, rx) = setup();
    server
        .operation("mcp-gateway-negotiate-v1", &json!({}))
        .unwrap();
    server
        .operation(
            "mcp-tools-update-v1",
            &json!({"schemaVersion":1,"revision":1,"tools":[]}),
        )
        .unwrap();
    server.set_enabled(true).unwrap();
    let state = server.status();
    assert_eq!(state["running"], true);
    let response = post(
        &server,
        json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
    );
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"tools\":[]"));
    assert!(rx.try_recv().is_err());
    server.stop();
    assert_eq!(server.status()["running"], false);
    assert!(server.state.settings.lock().unwrap().value.enabled);
}
#[test]
fn mcp_server_bind_failure_preserves_saved_enabled_and_same_true_retries() {
    let (_profile, server, _rx) = setup();
    server
        .state
        .settings
        .lock()
        .unwrap()
        .set_enabled(true)
        .unwrap();
    let port = server.state.settings.lock().unwrap().value.port;
    let occupied = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    assert!(server.start_saved().is_err());
    assert_eq!(server.status()["enabled"], true);
    assert_eq!(server.status()["phase"], "error");
    drop(occupied);
    server.set_enabled(true).unwrap();
    assert_eq!(server.status()["running"], true);
    server.set_enabled(false).unwrap();
    assert_eq!(server.status()["enabled"], false);
}
#[test]
fn mcp_server_missing_catalog_coalesces_sync_and_returns_only_after_projection_is_saved() {
    let (_profile, server, rx) = setup();
    server.set_enabled(true).unwrap();
    let request_server = server.clone();
    let pending = std::thread::spawn(move || {
        post(
            &request_server,
            json!({"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}),
        )
    });
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(3)).unwrap(),
        HostCommand::McpWake
    ));
    server
        .operation("mcp-gateway-negotiate-v1", &json!({}))
        .unwrap();
    server
        .operation(
            "mcp-tools-update-v1",
            &json!({"schemaVersion":1,"revision":1,"tools":[]}),
        )
        .unwrap();
    assert!(pending.join().unwrap().contains("\"tools\":[]"));
    server.stop();
}
