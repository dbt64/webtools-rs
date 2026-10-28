use super::*;
use webtools_ipc::{Connection, Envelope};

#[test]
#[ignore = "launches an isolated fake Manager to test the real startup dispatcher"]
fn queued_preview_waits_for_electron_services_ready() {
    const TEST: &str =
        "host_runtime::readiness_tests::queued_preview_waits_for_electron_services_ready";
    if let Some(root) = std::env::var_os("WT_HOST_READY_TEST_CHILD") {
        let root = std::path::PathBuf::from(root);
        let mut client = Connection::connect(
            &std::env::var("WEBTOOLS_NATIVE_PIPE").unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();
        client.request(&Envelope::new("hello", serde_json::json!({
            "processId":std::process::id(),
            "capabilities":["electron-host-lifecycle-v1","super-panel-context-v1","launcher-feature-surfaces-v1"]
        })), Duration::from_secs(3)).unwrap();
        client
            .request(
                &Envelope::new(
                    "feature-quick-result-negotiate-v1",
                    serde_json::json!({
                        "capability":"super-panel-quick-result-v1"
                    }),
                ),
                Duration::from_secs(3),
            )
            .unwrap();
        std::fs::write(root.join("hello"), "connected, services not initialized").unwrap();
        while !root.join("allow-ready").exists() {
            if let Some(message) = client.poll(Duration::from_millis(10)).unwrap() {
                if message.kind == "feature-invoke" {
                    std::fs::write(
                        root.join("premature"),
                        "feature-invoke before electron-host-ready",
                    )
                    .unwrap();
                    client.reply(&message, true, Some(serde_json::json!({
                        "requestId":message.payload["requestId"],"status":"failed","reasonCode":"dispatch-failed"
                    }))).unwrap();
                } else {
                    assert_eq!(message.kind, "host-ownership-sync");
                    client
                        .reply(&message, true, Some(serde_json::json!({"exiting":false})))
                        .unwrap();
                }
            }
        }
        let ownership = client
            .request(
                &Envelope::new("electron-host-ownership-get", serde_json::json!({})),
                Duration::from_secs(3),
            )
            .unwrap();
        client
            .request(
                &Envelope::new(
                    "electron-host-ready",
                    serde_json::json!({
                        "ownershipRevision":ownership.result.unwrap()["revision"]
                    }),
                ),
                Duration::from_secs(3),
            )
            .unwrap();
        loop {
            let message = match client.poll(Duration::from_millis(20)) {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(_) => break,
            };
            if message.kind == "host-ownership-sync" {
                if client
                    .reply(&message, true, Some(serde_json::json!({"exiting":false})))
                    .is_err()
                {
                    break;
                }
            } else if message.kind == "feature-invoke" {
                let id = message.payload["requestId"].clone();
                client.request(&Envelope::new("feature-result-v1", serde_json::json!({
                    "resultVersion":1,"requestId":id,"ref":message.payload["ref"],"text":"READY_TRANSLATION_RESULT"
                })), Duration::from_secs(3)).unwrap();
                client.reply(&message, true, Some(serde_json::json!({"requestId":id,"status":"completed","reasonCode":"completed"}))).unwrap();
            }
        }
        return;
    }

    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/host-ready-regression")
        .join(webtools_ipc::new_request_id());
    std::fs::create_dir_all(&root).unwrap();
    std::env::set_var("WT_HOST_READY_TEST_CHILD", &root);
    std::env::set_var("WT_PROFILE_ROOT", &root);
    let ownership = webtools_ipc::HostOwnership::default();
    ownership.begin_session("visible-panel");
    let (events, receiver) = mpsc::channel();
    let runtime = HostRuntime::start(
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        vec![
            "--ignored".into(),
            "--exact".into(),
            TEST.into(),
            "--nocapture".into(),
        ],
        Duration::from_secs(30),
        events,
        ownership.clone(),
        Arc::new(Mutex::new(webtools_ipc::state::LauncherStateStore::memory())),
        webtools_ipc::projection_cache::ProjectionCache::memory(),
    );
    runtime
        .handle()
        .send(HostCommand::preview_translation(
            "Electron".into(),
            webtools_ipc::new_request_id(),
        ))
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !root.join("hello").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::env::remove_var("WT_HOST_READY_TEST_CHILD");
    assert!(root.join("hello").exists(), "fake Manager must connect");
    let deadline = Instant::now() + Duration::from_millis(250);
    while !root.join("premature").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    std::fs::write(root.join("allow-ready"), "services initialized").unwrap();
    let mut output = None;
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(HostEvent::Result { text, .. }) => {
                output = Some(text);
                break;
            }
            Ok(HostEvent::PreviewFinished { status, .. }) if status == "failed" => break,
            _ => {}
        }
    }
    runtime.shutdown();
    ownership.end_session("visible-panel");
    assert!(
        !root.join("premature").exists(),
        "preview was dispatched after hello but before Electron services were ready"
    );
    assert_eq!(output.as_deref(), Some("READY_TRANSLATION_RESULT"));
    assert!(!ownership.has_owners());
}
