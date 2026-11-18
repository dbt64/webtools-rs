use serde_json::json;
use std::{sync::mpsc, time::Duration};
use webtools_ipc::{mcp::McpCallRequest, Connection, Envelope, HostSession, PipeServer};
#[test]
fn closing_ownership_ack_keeps_the_tool_demonstrably_undelivered() {
    let name = format!("WebTools.mcp.exiting.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let host = std::thread::spawn(move || {
        let mut session = HostSession::new(server.accept().unwrap());
        session.set_mcp_handler(Box::new(|_, _| Ok(json!({}))));
        session.pump(Duration::from_secs(2)).unwrap();
        session.begin_mcp_call(&McpCallRequest { call_id:"survive-exit".into(),tool_name:"org.test.note".into(),arguments:json!({}),deadline_unix_ms:9999999999999 }).unwrap();
        session.pump(Duration::from_secs(2)).unwrap();
        assert!(session.closing);
        assert!(session.take_completed_mcp_calls().is_empty());
        assert_eq!(session.take_undelivered_mcp_calls(),vec!["survive-exit"]);
    });
    let mut client = Connection::connect(&name, Duration::from_secs(2)).unwrap();
    client.request(&Envelope::new("mcp-gateway-negotiate-v1", json!({"capability":"plugin-mcp-gateway-v1"})),Duration::from_secs(2)).unwrap();
    let owner = client.poll(Duration::from_secs(2)).unwrap().unwrap();
    assert_eq!(owner.kind,"host-ownership-sync");
    client.reply(&owner,true,Some(json!({"exiting":true}))).unwrap();
    host.join().unwrap();
}
#[test]
fn mcp_ack_during_legacy_request_is_not_lost_and_dispatch_waits_for_ownership() {
    let name = format!("WebTools.mcp.session.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let (tx, rx) = mpsc::channel();
    let (finished_tx, finished_rx) = mpsc::channel();
    let host = std::thread::spawn(move || {
        let mut session = HostSession::new(server.accept().unwrap());
        session.set_mcp_handler(Box::new(|_, _| Ok(json!({}))));
        session.set_mcp_pump(Box::new(move |session| {
            for completed in session.take_completed_mcp_calls() {
                finished_tx.send(completed).unwrap();
            }
            Ok(())
        }));
        session.pump(Duration::from_secs(3)).unwrap();
        session
            .begin_mcp_call(&McpCallRequest {
                call_id: "mcp-one".into(),
                tool_name: "org.test.note".into(),
                arguments: json!({}),
                deadline_unix_ms: 9999999999999,
            })
            .unwrap();
        tx.send(()).unwrap();
        // A legacy synchronous operation must route, rather than discard, asynchronous acks.
        let ack = session
            .request(
                &Envelope::new("legacy-test", json!({})),
                Duration::from_secs(3),
            )
            .unwrap();
        assert!(ack.ok);
    });
    let mut client = Connection::connect(&name, Duration::from_secs(3)).unwrap();
    let negotiated = client
        .request(
            &Envelope::new(
                "mcp-gateway-negotiate-v1",
                json!({"capability":"plugin-mcp-gateway-v1"}),
            ),
            Duration::from_secs(3),
        )
        .unwrap();
    assert!(negotiated.ok);
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let mut legacy = None;
    let mut owner_acks = 0;
    loop {
        let message = client.poll(Duration::from_secs(3)).unwrap().unwrap();
        match message.kind.as_str() {
            "host-ownership-sync" => {
                owner_acks += 1;
                client
                    .reply(&message, true, Some(json!({"exiting":false})))
                    .unwrap();
            }
            "legacy-test" => legacy = Some(message),
            "mcp-call-v1" => {
                assert_eq!(
                    owner_acks, 2,
                    "MCP cannot run before queued and delivered ownership acknowledgements"
                );
                client.reply(&message,true,Some(json!({"callId":"mcp-one","status":"completed","result":{"content":[{"type":"text","text":"ok"}],"structuredContent":{"text":"ok"}}}))).unwrap();
                let completed = finished_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("MCP completion must be delivered before legacy operation ends");
                assert_eq!(completed.call_id, "mcp-one");
                assert_eq!(
                    completed.result.as_ref().unwrap()["structuredContent"]["text"],
                    "ok"
                );
                client
                    .reply(&legacy.take().unwrap(), true, Some(json!({})))
                    .unwrap();
                break;
            }
            _ => panic!("unexpected message {}", message.kind),
        }
    }
    host.join().unwrap();
}
#[test]
fn cancel_before_ownership_ack_never_dispatches_tool() {
    let name = format!("WebTools.mcp.cancel.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let (tx, rx) = mpsc::channel();
    let host = std::thread::spawn(move || {
        let mut session = HostSession::new(server.accept().unwrap());
        session.set_mcp_handler(Box::new(|_, _| Ok(json!({}))));
        session.pump(Duration::from_secs(3)).unwrap();
        session
            .begin_mcp_call(&McpCallRequest {
                call_id: "cancel-me".into(),
                tool_name: "org.test.note".into(),
                arguments: json!({}),
                deadline_unix_ms: 9999999999999,
            })
            .unwrap();
        session.begin_mcp_cancel("cancel-me").unwrap();
        let result = session.take_completed_mcp_calls();
        assert_eq!(result[0].status, "cancelled");
        tx.send(()).unwrap();
        session.pump(Duration::from_secs(3)).unwrap();
    });
    let mut client = Connection::connect(&name, Duration::from_secs(3)).unwrap();
    client
        .request(
            &Envelope::new(
                "mcp-gateway-negotiate-v1",
                json!({"capability":"plugin-mcp-gateway-v1"}),
            ),
            Duration::from_secs(3),
        )
        .unwrap();
    rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let sync = client.poll(Duration::from_secs(3)).unwrap().unwrap();
    assert_eq!(sync.kind, "host-ownership-sync");
    client.reply(&sync, true, Some(json!({}))).unwrap();
    host.join().unwrap();
}
