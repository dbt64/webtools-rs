use super::*;
use serde_json::json;
#[test]
fn cooperative_exit_preserves_only_undelivered_calls_without_resetting_the_deadline() {
    let owner = HostOwnership::default();
    let mut dispatch = McpDispatch::new(owner.clone(), Duration::ZERO);
    let now = Instant::now();
    let mut queued = dispatch.admit(request("queued"), now, Duration::from_millis(50)).unwrap();
    let mut handshake = dispatch.admit(request("handshake"), now, Duration::from_millis(50)).unwrap();
    let mut delivered = dispatch.admit(request("delivered"), now, Duration::from_secs(60)).unwrap();
    dispatch.mark_submitted("handshake");
    dispatch.mark_submitted("delivered");
    dispatch.idle_exit(&["handshake".to_string()]);
    assert_eq!(delivered.try_recv().unwrap().status, "outcome-unknown");
    assert_eq!(dispatch.pending().len(), 2);
    assert!(queued.try_recv().is_err());
    assert!(handshake.try_recv().is_err());
    dispatch.tick(now + Duration::from_millis(51));
    assert_eq!(queued.try_recv().unwrap().status, "failed");
    assert_eq!(handshake.try_recv().unwrap().status, "failed");
    assert!(!owner.has_owners());
}

#[test]
fn startup_pipe_wait_services_deadlines_without_a_manager_connection() {
    let owner = HostOwnership::default();
    let mut dispatch = McpDispatch::new(owner.clone(), Duration::ZERO);
    let mut reply = dispatch.admit(request("startup"), Instant::now(), Duration::from_millis(30)).unwrap();
    let server = webtools_ipc::PipeServer::bind(&format!("WebTools.Mcp.startup.{}",webtools_ipc::new_request_id())).unwrap();
    let started = Instant::now();
    let result = server.accept_until_with_tick(&std::sync::atomic::AtomicBool::new(false),Duration::from_secs(20), || {
        dispatch.tick(Instant::now());
        owner.has_owners()
    });
    assert_eq!(result.err().unwrap().kind(), std::io::ErrorKind::Interrupted);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(reply.try_recv().unwrap().status, "failed");
    assert_eq!(dispatch.active_calls(),0);
}
fn request(id: &str) -> McpCallRequest {
    McpCallRequest {
        call_id: id.into(),
        tool_name: "org.test.note".into(),
        arguments: json!({}),
        deadline_unix_ms: 9999999999999,
    }
}
#[test]
fn acceptance_cooldown_override_is_isolated_and_normal_runtime_remains_fifteen_seconds() {
    assert_eq!(
        acceptance_cooldown(false, true, Some("250")),
        Duration::from_secs(15)
    );
    assert_eq!(
        acceptance_cooldown(true, false, Some("250")),
        Duration::from_secs(15)
    );
    assert_eq!(
        acceptance_cooldown(true, true, Some("250")),
        Duration::from_millis(250)
    );
    assert_eq!(
        acceptance_cooldown(true, true, Some("0")),
        Duration::from_secs(15)
    );
}
#[test]
fn mcp_dispatch_bounds_admission_and_cancelled_execution_retains_ownership_until_reply() {
    let owner = HostOwnership::default();
    let mut dispatch = McpDispatch::new(owner.clone(), Duration::from_secs(15));
    let now = Instant::now();
    let mut replies = Vec::new();
    for i in 0..8 {
        replies.push(
            dispatch
                .admit(request(&format!("call-{i}")), now, Duration::from_secs(60))
                .unwrap(),
        );
    }
    assert!(dispatch
        .admit(request("ninth"), now, Duration::from_secs(60))
        .is_err());
    dispatch.mark_submitted("call-0");
    dispatch.cancel("call-0");
    assert_eq!(dispatch.active_calls(), 8);
    assert!(owner.has_owners());
    assert_eq!(dispatch.tick(now), vec!["call-0"]);
    assert!(dispatch.tick(now).is_empty());
    dispatch.complete(
        McpCallReply::failed("call-0", "cancelled", "cancelled"),
        now,
    );
    assert_eq!(dispatch.active_calls(), 7);
    assert!(replies[0].try_recv().is_ok());
}
#[test]
fn mcp_dispatch_cooldown_renewal_cannot_release_new_work_or_unrelated_owner() {
    let owner = HostOwnership::default();
    let mut dispatch = McpDispatch::new(owner.clone(), Duration::from_secs(15));
    let now = Instant::now();
    let _one = dispatch
        .admit(request("one"), now, Duration::from_secs(60))
        .unwrap();
    dispatch.mark_submitted("one");
    dispatch.complete(McpCallReply::failed("one", "completed", ""), now);
    assert!(owner.has_owners());
    let _two = dispatch
        .admit(
            request("two"),
            now + Duration::from_secs(15),
            Duration::from_secs(60),
        )
        .unwrap();
    dispatch.tick(now + Duration::from_secs(15));
    assert!(owner.has_owners());
    dispatch.mark_submitted("two");
    dispatch.complete(
        McpCallReply::failed("two", "completed", ""),
        now + Duration::from_secs(15),
    );
    owner.begin_session("ui");
    dispatch.tick(now + Duration::from_secs(31));
    assert!(owner.has_owners());
    owner.end_session("ui");
    assert!(!owner.has_owners());
}
#[test]
fn mcp_dispatch_disconnect_never_requeues_submitted_work_and_timeout_waits_for_cleanup() {
    let owner = HostOwnership::default();
    let mut dispatch = McpDispatch::new(owner.clone(), Duration::ZERO);
    let now = Instant::now();
    let mut reply = dispatch
        .admit(request("one"), now, Duration::from_millis(1))
        .unwrap();
    dispatch.mark_submitted("one");
    assert_eq!(dispatch.tick(now + Duration::from_millis(2)), vec!["one"]);
    assert_eq!(reply.try_recv().unwrap().status, "failed");
    assert_eq!(dispatch.active_calls(), 1);
    dispatch.disconnect();
    assert_eq!(dispatch.active_calls(), 0);
    assert!(dispatch.pending().is_empty());
    assert!(!owner.has_owners());
    let mut second = dispatch
        .admit(request("two"), now, Duration::from_secs(60))
        .unwrap();
    dispatch.mark_submitted("two");
    dispatch.disconnect();
    assert_eq!(second.try_recv().unwrap().status, "outcome-unknown");
    assert!(!owner.has_owners());
}
