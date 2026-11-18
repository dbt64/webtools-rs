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
        let mut measured = false;
        let mut probed = false;
        loop {
            if !measured && root.join("measure-idle-ipc").exists() {
                let mut samples = Vec::new();
                for _ in 0..10 {
                    let started = Instant::now();
                    assert!(client
                        .request(
                            &Envelope::new("websites-update", serde_json::json!({"websites":[]})),
                            Duration::from_secs(2),
                        )
                        .unwrap()
                        .ok);
                    samples.push(started.elapsed());
                }
                samples.sort_unstable();
                eprintln!("warm IPC: median {:?}, max {:?}", samples[5], samples[9]);
                std::fs::write(root.join("idle-measured"), "").unwrap();
                measured = true;
            }
            if !probed && root.join("probe-busy-ipc").exists() {
                let result = client.request(
                    &Envelope::new("websites-update", serde_json::json!({"websites":[]})),
                    Duration::from_millis(500),
                );
                let reply = if result.is_ok_and(|ack| ack.ok) {
                    "ok"
                } else {
                    "timeout"
                };
                std::fs::write(root.join("busy-reply"), reply).unwrap();
                probed = true;
            }
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
    assert!(
        !root.join("premature").exists(),
        "preview was dispatched after hello but before Electron services were ready"
    );
    assert_eq!(output.as_deref(), Some("READY_TRANSLATION_RESULT"));
    // Let the preview's ownership exchange finish before measuring an idle link.
    std::thread::sleep(Duration::from_millis(150));
    std::fs::write(root.join("measure-idle-ipc"), "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !root.join("idle-measured").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        root.join("idle-measured").exists(),
        "warm IPC probe did not finish"
    );

    let commands = runtime.handle();
    commands.send(HostCommand::McpWake).unwrap();
    std::fs::write(root.join("probe-busy-ipc"), "").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        commands.send(HostCommand::McpWake).unwrap();
        if let Ok(reply) = std::fs::read_to_string(root.join("busy-reply")) {
            if !reply.is_empty() {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let reply = std::fs::read_to_string(root.join("busy-reply"));
    runtime.shutdown();
    ownership.end_session("visible-panel");
    assert_eq!(
        reply.as_deref().ok(),
        Some("ok"),
        "Manager IPC must respond while local commands keep arriving"
    );
    assert!(!ownership.has_owners());
}
