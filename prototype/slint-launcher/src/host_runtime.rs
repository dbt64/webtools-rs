//! Background supervisor for the real WebTools Manager.
//!
//! One thread owns the pipe, the manager process and the session. The UI thread
//! only pushes commands through a channel, so nothing in the interface can block
//! on a 45-second IPC round trip.
//!
//! If the manager dies while there are owners, the supervisor retries with a new
//! pipe name and linear backoff. When no owner remains it waits for the next use.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, RecvTimeoutError, Sender},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

/// What the supervisor reports back to the UI thread.
pub enum HostEvent {
    PanelSettings {
        operation: webtools_ipc::super_panel::PanelOperation,
        reply: mpsc::SyncSender<
            Result<webtools_ipc::super_panel::PanelStatus, webtools_ipc::settings::SettingsError>,
        >,
    },
    Settings {
        phase: webtools_ipc::settings::SettingsPhase,
        change: webtools_ipc::settings::SettingsChange,
        reply: mpsc::SyncSender<Result<(), webtools_ipc::settings::SettingsError>>,
    },
    /// The plugin projection, sent whenever the manager republishes it.
    Features(Vec<webtools_ipc::projection::FeatureRow>),
    SurfaceRequest {
        request: webtools_ipc::session::SurfaceRequest,
        reply: mpsc::SyncSender<bool>,
    },
    TemporaryWindowToggleHandled(bool),
    Websites(serde_json::Value),
    Result {
        request_id: String,
        plugin: String,
        feature: String,
        text: String,
    },
    AIConfirmation(webtools_ipc::quick_result::AIConfirmation),
    PreviewFinished {
        request_id: String,
        status: String,
        reason: String,
    },
    /// A surface session was released after its Manager request was acknowledged.
    SessionEnded(String),
    Reconnecting {
        attempt: u32,
    },
    Connected,
    Disconnected(String),
}

#[derive(Clone)]
pub enum HostCommand {
    OpenPage {
        section: &'static str,
        request_id: String,
        end_session_after: Option<String>,
    },
    Invoke {
        request_id: String,
        source: &'static str,
        plugin: String,
        feature: String,
        selection: Option<webtools_ipc::projection::Selection>,
        launcher_text: Option<String>,
        quick_result: bool,
        end_session_after: Option<String>,
    },
    Translation {
        request_id: String,
        text: String,
        source: &'static str,
        end_session_after: Option<String>,
    },
    CancelResult {
        request_id: String,
        plugin: String,
        feature: String,
        session_id: String,
    },
    AIConsentDecision {
        confirmation: webtools_ipc::quick_result::AIConfirmation,
        approved: bool,
    },
    PreviewTranslation {
        request_id: String,
        text: String,
    },
    ToggleCurrentTemporaryWindow {
        connection_generation: u64,
    },
    SyncOwnership,
    Shutdown,
}

impl HostCommand {
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::OpenPage { request_id, .. }
            | Self::Invoke { request_id, .. }
            | Self::Translation { request_id, .. } => Some(request_id),
            _ => None,
        }
    }

    pub fn open_page(section: &'static str) -> Self {
        Self::OpenPage {
            section,
            request_id: String::new(),
            end_session_after: None,
        }
    }
    pub fn invoke(
        plugin: String,
        feature: String,
        selection: Option<webtools_ipc::projection::Selection>,
    ) -> Self {
        Self::Invoke {
            request_id: String::new(),
            source: "super-panel",
            plugin,
            feature,
            selection,
            launcher_text: None,
            quick_result: false,
            end_session_after: None,
        }
    }
    pub fn invoke_with_request_id(
        plugin: String,
        feature: String,
        selection: Option<webtools_ipc::projection::Selection>,
        request_id: String,
    ) -> Self {
        Self::Invoke {
            request_id,
            source: "super-panel",
            plugin,
            feature,
            selection,
            launcher_text: None,
            quick_result: true,
            end_session_after: None,
        }
    }
    pub fn preview_translation(text: String, request_id: String) -> Self {
        Self::PreviewTranslation { text, request_id }
    }
    pub fn invoke_launcher(plugin: String, feature: String, text: Option<String>) -> Self {
        Self::Invoke {
            request_id: String::new(),
            source: "launcher",
            plugin,
            feature,
            selection: None,
            launcher_text: text,
            quick_result: false,
            end_session_after: None,
        }
    }
    pub fn translation(text: String, source: &'static str) -> Self {
        Self::Translation {
            request_id: String::new(),
            text,
            source,
            end_session_after: None,
        }
    }
    pub fn with_request_ownership(mut self, end_session_after: Option<String>) -> Self {
        match &mut self {
            Self::OpenPage {
                request_id,
                end_session_after: release,
                ..
            }
            | Self::Invoke {
                request_id,
                end_session_after: release,
                ..
            }
            | Self::Translation {
                request_id,
                end_session_after: release,
                ..
            } => {
                if request_id.is_empty() {
                    *request_id = webtools_ipc::new_request_id();
                }
                *release = end_session_after;
            }
            Self::SyncOwnership
            | Self::Shutdown
            | Self::CancelResult { .. }
            | Self::AIConsentDecision { .. }
            | Self::PreviewTranslation { .. }
            | Self::ToggleCurrentTemporaryWindow { .. } => {}
        }
        self
    }
}

const MAX_ATTEMPTS: u32 = 5;

pub(crate) fn panel_handler(
    events: Sender<HostEvent>,
    stop: Arc<AtomicBool>,
) -> webtools_ipc::super_panel::PanelHandler {
    Box::new(move |operation| {
        if stop.load(Ordering::SeqCst) {
            return Err(webtools_ipc::settings::SettingsError::unavailable());
        }
        let (reply, received) = mpsc::sync_channel(1);
        events
            .send(HostEvent::PanelSettings { operation, reply })
            .map_err(|_| webtools_ipc::settings::SettingsError::unavailable())?;
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err(webtools_ipc::settings::SettingsError::unavailable());
            }
            match received.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(webtools_ipc::settings::SettingsError::unavailable())
                }
            }
        }
    })
}

pub(crate) fn settings_handler(
    settings_events: Sender<HostEvent>,
    stop: Arc<AtomicBool>,
) -> webtools_ipc::settings::SettingsHandler {
    Box::new(move |phase, change| {
        if stop.load(Ordering::SeqCst) {
            return Err(webtools_ipc::settings::SettingsError::unavailable());
        }
        let (reply, received) = mpsc::sync_channel(1);
        settings_events
            .send(HostEvent::Settings {
                phase,
                change: change.clone(),
                reply,
            })
            .map_err(|_| webtools_ipc::settings::SettingsError::unavailable())?;
        loop {
            if stop.load(Ordering::SeqCst) {
                return Err(webtools_ipc::settings::SettingsError::unavailable());
            }
            match received.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => return result,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(webtools_ipc::settings::SettingsError::unavailable())
                }
            }
        }
    })
}

fn projection_changed(
    seen: &mut Option<(String, i64)>,
    projection: &webtools_ipc::projection::Projection,
) -> bool {
    let key = (projection.host_session_id.clone(), projection.revision);
    if seen.as_ref() == Some(&key) {
        return false;
    }
    *seen = Some(key);
    true
}

#[cfg(test)]
#[path = "host_runtime_readiness_tests.rs"]
mod readiness_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "queries MyMemory with generic text through an isolated real Electron Manager"]
    fn machine_preview_uses_real_manager_and_returns_result() {
        let exe = std::env::var("WT_LIFECYCLE_MANAGER").expect("WT_LIFECYCLE_MANAGER");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/machine-preview")
            .join(webtools_ipc::new_request_id());
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("WT_PROFILE_ROOT", &root);
        let ownership = webtools_ipc::HostOwnership::default();
        ownership.begin_session("visible-panel");
        let (events, receiver) = mpsc::channel();
        let runtime = HostRuntime::start(
            exe,
            vec!["--phase4g-manager-test".into(), "--manager-only".into()],
            Duration::from_secs(30),
            events,
            ownership.clone(),
            Arc::new(Mutex::new(
                webtools_ipc::state::LauncherStateStore::load(&root).unwrap(),
            )),
            webtools_ipc::projection_cache::ProjectionCache::memory(),
        );
        let request_id = webtools_ipc::new_request_id();
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut sent = false;
        let mut output = None;
        while Instant::now() < deadline {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(HostEvent::PanelSettings { reply, .. }) => {
                    let settings = webtools_ipc::super_panel::PanelSettings {
                        auto_translation_preview: true,
                        ..Default::default()
                    };
                    let _ = reply.send(Ok(webtools_ipc::super_panel::PanelStatus::new(
                        settings, true, None,
                    )));
                }
                Ok(HostEvent::Features(rows))
                    if !sent
                        && rows
                            .iter()
                            .any(|row| row.plugin_id == "webtools.translation") =>
                {
                    runtime
                        .handle()
                        .send(HostCommand::preview_translation(
                            "Hello world".into(),
                            request_id.clone(),
                        ))
                        .unwrap();
                    sent = true;
                }
                Ok(HostEvent::Result {
                    request_id: id,
                    text,
                    ..
                }) if id == request_id => {
                    output = Some(text);
                    break;
                }
                Ok(HostEvent::PreviewFinished { status, reason, .. }) if status != "completed" => {
                    panic!("real preview rejected: {status}/{reason}");
                }
                Ok(_) | Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        ownership.end_session("visible-panel");
        runtime.shutdown();
        let output = output.expect("real Manager must publish the machine translation result");
        std::fs::write(root.join("result.txt"), &output).unwrap();
        assert!(
            output.contains("MyMemory") && output.contains("你好"),
            "unexpected machine result: {output}"
        );
        assert!(
            !ownership.has_owners(),
            "preview must not leave request ownership after transport closes"
        );
        eprintln!(
            "PASS real Electron -> MyMemory -> Rust panel result: {output}; evidence={}",
            root.display()
        );
    }

    #[test]
    #[ignore = "spawns an owned fake Manager to verify cancellation during a pending preview"]
    fn slow_preview_does_not_block_cancel_commands() {
        if std::env::var_os("WT_ASYNC_PREVIEW_CHILD").is_some() {
            let root = std::path::PathBuf::from(std::env::var_os("WT_ASYNC_PREVIEW_ROOT").unwrap());
            let pipe = std::env::var("WEBTOOLS_NATIVE_PIPE").unwrap();
            let mut client =
                webtools_ipc::Connection::connect(&pipe, Duration::from_secs(5)).unwrap();
            client.request(&webtools_ipc::Envelope::new("hello", serde_json::json!({
                "processId":std::process::id(), "capabilities":["electron-host-lifecycle-v1"]
            })), Duration::from_secs(3)).unwrap();
            let ownership = client
                .request(
                    &webtools_ipc::Envelope::new(
                        "electron-host-ownership-get",
                        serde_json::json!({}),
                    ),
                    Duration::from_secs(3),
                )
                .unwrap();
            client
                .request(
                    &webtools_ipc::Envelope::new(
                        "electron-host-ready",
                        serde_json::json!({
                            "ownershipRevision":ownership.result.unwrap()["revision"]
                        }),
                    ),
                    Duration::from_secs(3),
                )
                .unwrap();
            let mut preview = None;
            loop {
                let message = match client.poll(Duration::from_millis(50)) {
                    Ok(Some(message)) => message,
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
                    preview = Some(message);
                    std::fs::write(root.join("preview-received"), "pending").unwrap();
                } else if message.kind == "feature-result-cancel-v1" {
                    client.reply(&message, true, None).unwrap();
                    if let Some(original) = preview.take() {
                        assert_eq!(message.payload["requestId"], original.payload["requestId"]);
                        client.reply(&original, true, Some(serde_json::json!({"requestId":original.payload["requestId"],"status":"cancelled","reasonCode":"cancelled"}))).unwrap();
                    }
                    std::fs::write(
                        root.join("cancel-received"),
                        "cancelled before provider completed",
                    )
                    .unwrap();
                }
            }
            return;
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/preview-cancel")
            .join(webtools_ipc::new_request_id());
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("WT_ASYNC_PREVIEW_CHILD", "1");
        std::env::set_var("WT_ASYNC_PREVIEW_ROOT", &root);
        let ownership = webtools_ipc::HostOwnership::default();
        ownership.begin_session("visible-panel");
        let (events, _receiver) = mpsc::channel();
        let runtime = HostRuntime::start(
            std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            vec![
                "--ignored".into(),
                "--exact".into(),
                "host_runtime::tests::slow_preview_does_not_block_cancel_commands".into(),
                "--nocapture".into(),
            ],
            Duration::from_secs(30),
            events,
            ownership.clone(),
            Arc::new(Mutex::new(webtools_ipc::state::LauncherStateStore::memory())),
            webtools_ipc::projection_cache::ProjectionCache::memory(),
        );
        let request_id = webtools_ipc::new_request_id();
        runtime
            .handle()
            .send(HostCommand::preview_translation(
                "Hello world".into(),
                request_id.clone(),
            ))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("preview-received").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::env::remove_var("WT_ASYNC_PREVIEW_CHILD");
        std::env::remove_var("WT_ASYNC_PREVIEW_ROOT");
        let received = root.join("preview-received").exists();
        ownership.begin_session("cancel-control");
        runtime
            .handle()
            .send(HostCommand::CancelResult {
                request_id,
                plugin: "webtools.translation".into(),
                feature: "translate-text".into(),
                session_id: "cancel-control".into(),
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !root.join("cancel-received").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let cancelled = root.join("cancel-received").exists();
        runtime.shutdown();
        ownership.end_session("visible-panel");
        assert!(
            received,
            "fake Manager must receive the actual preview request"
        );
        assert!(
            cancelled,
            "cancel command was queued behind the pending provider response"
        );
        assert!(
            !ownership.has_owners(),
            "cancelled preview/control ownership must be released"
        );
        eprintln!(
            "PASS pending preview cancelled before provider completion; evidence={}",
            root.display()
        );
    }
    #[test]
    fn same_row_count_with_new_revision_or_session_is_forwarded() {
        let mut seen = None;
        let mut projection = webtools_ipc::projection::Projection {
            host_session_id: "a".into(),
            revision: 1,
            features: vec![],
        };
        assert!(projection_changed(&mut seen, &projection));
        assert!(!projection_changed(&mut seen, &projection));
        projection.revision = 2;
        assert!(projection_changed(&mut seen, &projection));
        projection.host_session_id = "b".into();
        assert!(projection_changed(&mut seen, &projection));
    }

    #[test]
    fn invoke_ack_releases_only_the_transferred_session_on_success_or_failure() {
        for accepted in [true, false] {
            let ownership = webtools_ipc::HostOwnership::default();
            let previous = webtools_ipc::new_request_id();
            let reopened = webtools_ipc::new_request_id();
            ownership.begin_session(&previous);
            ownership.begin_session(&reopened);
            let command = HostCommand::invoke_launcher("test.plugin".into(), "open".into(), None)
                .with_request_ownership(Some(previous.clone()));
            let pipe = format!("WebTools.lifecycle.{}", webtools_ipc::new_request_id());
            let server = webtools_ipc::PipeServer::bind(&pipe).unwrap();
            let expected = reopened.clone();
            let manager = std::thread::spawn(move || {
                let mut client =
                    webtools_ipc::Connection::connect(&pipe, Duration::from_secs(2)).unwrap();
                let mut snapshots = vec![];
                for _ in 0..5 {
                    let message = client.next_event(Duration::from_secs(2)).unwrap().unwrap();
                    if message.kind == "host-ownership-sync" {
                        snapshots.push(message.payload.clone());
                        client
                            .reply(&message, true, Some(serde_json::json!({"exiting":false})))
                            .unwrap();
                    } else {
                        assert_eq!(message.kind, "feature-invoke");
                        client.reply(&message, accepted, Some(serde_json::json!({
                            "requestId": message.payload["requestId"],
                            "status": if accepted { "accepted" } else { "failed" },
                            "reasonCode": if accepted { "accepted" } else { "dispatch-failed" },
                        }))).unwrap();
                    }
                }
                let final_snapshot = snapshots.last().unwrap();
                assert_eq!(final_snapshot["sessionIds"], serde_json::json!([expected]));
                assert_eq!(final_snapshot["requests"], serde_json::json!([]));
            });
            let connection = server.accept().unwrap();
            let mut session = webtools_ipc::HostSession::with_ownership(
                connection,
                webtools_ipc::state::LauncherStateStore::memory(),
                ownership.clone(),
            );
            let (events, receiver) = mpsc::channel();
            run(command, &mut session, &ownership, &events).unwrap();
            assert!(
                matches!(receiver.recv().unwrap(), HostEvent::SessionEnded(id) if id == previous)
            );
            assert!(
                ownership.has_owners(),
                "reopened native surface must still own Manager"
            );
            ownership.end_session(&reopened);
            assert!(
                !ownership.has_owners(),
                "transferred session/request must be released after any ack"
            );
            manager.join().unwrap();
        }
    }
}

pub struct HostRuntime {
    tx: Sender<HostCommand>,
    thread: Option<std::thread::JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    connection_generation: Arc<AtomicU64>,
    process: Arc<Mutex<Option<OwnedHandle>>>,
}

impl HostRuntime {
    /// Binds the pipe and spawns the manager; the manager connects on its own.
    pub fn start(
        exe: String,
        args: Vec<String>,
        idle_trim: Duration,
        events: Sender<HostEvent>,
        ownership: webtools_ipc::HostOwnership,
        launcher_state: Arc<Mutex<webtools_ipc::state::LauncherStateStore>>,
        projection_cache: webtools_ipc::projection_cache::ProjectionCache,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let connection_generation = Arc::new(AtomicU64::new(0));
        let process = Arc::new(Mutex::new(None));
        let stop = cancelled.clone();
        let connected_thread = connected.clone();
        let connection_generation_thread = connection_generation.clone();
        let process_thread = process.clone();
        let thread = std::thread::spawn(move || {
            supervise(
                exe,
                args,
                idle_trim,
                rx,
                events,
                stop,
                connected_thread.clone(),
                connection_generation_thread,
                process_thread,
                ownership,
                launcher_state,
                projection_cache,
            );
            connected_thread.store(false, Ordering::SeqCst);
        });
        Self {
            tx,
            thread: Some(thread),
            cancelled,
            connected,
            connection_generation,
            process,
        }
    }

    pub fn handle(&self) -> Sender<HostCommand> {
        self.tx.clone()
    }

    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst) && !self.is_finished()
    }

    /// Queue a one-shot toggle only for the currently live Manager link.
    /// Like focus requests, this must never be replayed after a reconnect.
    pub fn toggle_current_temporary_window(&self) -> bool {
        if !self.is_connected() {
            return false;
        }
        let generation = self.connection_generation.load(Ordering::SeqCst);
        if !self.is_connected() {
            return false;
        }
        self.tx
            .send(HostCommand::ToggleCurrentTemporaryWindow {
                connection_generation: generation,
            })
            .is_ok()
    }

    /// Graceful stop: tells the thread to kill the manager and waits for it.
    pub fn shutdown(mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
    fn stop(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        let _ = self.tx.send(HostCommand::Shutdown);
        if let Ok(process) = self.process.lock() {
            if let Some(handle) = process.as_ref() {
                unsafe {
                    windows_sys::Win32::System::Threading::TerminateProcess(
                        handle.as_raw_handle() as _,
                        0,
                    );
                }
            }
        }
    }
}

impl Drop for HostRuntime {
    fn drop(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// One live link: manager process + pipe session.
struct Link {
    host: webtools_ipc::HostProcess,
    session: webtools_ipc::HostSession,
}

fn supervise(
    exe: String,
    args: Vec<String>,
    idle_trim: Duration,
    rx: Receiver<HostCommand>,
    events: Sender<HostEvent>,
    cancelled: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    connection_generation: Arc<AtomicU64>,
    process: Arc<Mutex<Option<OwnedHandle>>>,
    ownership: webtools_ipc::HostOwnership,
    launcher_state: Arc<Mutex<webtools_ipc::state::LauncherStateStore>>,
    projection_cache: webtools_ipc::projection_cache::ProjectionCache,
) {
    let mut attempt: u32 = 0;
    let mut pending: Option<HostCommand> = None;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return;
        }
        if !ownership.has_owners() {
            match pending
                .take()
                .or_else(|| wait_for_command(&rx, &ownership, &cancelled, &events))
            {
                Some(HostCommand::Shutdown) | None => return,
                Some(HostCommand::SyncOwnership) if !ownership.has_owners() => continue,
                Some(HostCommand::ToggleCurrentTemporaryWindow { .. }) => {
                    let _ = events.send(HostEvent::TemporaryWindowToggleHandled(false));
                    continue;
                }
                Some(command) => pending = Some(command),
            }
            if !ownership.has_owners() {
                continue;
            }
        }
        if attempt > 0 {
            let retry = attempt - 1;
            let _ = events.send(HostEvent::Reconnecting { attempt: retry });
            eprintln!("host: reconnecting attempt {retry}");
            std::thread::sleep(Duration::from_millis(400 * attempt as u64));
        }
        let Some(mut link) = open_link(
            &exe,
            &args,
            attempt,
            &cancelled,
            &process,
            &events,
            ownership.clone(),
            launcher_state.clone(),
        ) else {
            connected.store(false, Ordering::SeqCst);
            if !ownership.has_owners() {
                attempt = 0;
                continue;
            }
            attempt += 1;
            if attempt >= MAX_ATTEMPTS {
                eprintln!("host: manager never started after {MAX_ATTEMPTS} attempts");
                let _ = events.send(HostEvent::Disconnected("manager did not start".into()));
                return;
            }
            continue;
        };
        let link_generation = connection_generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        connected.store(true, Ordering::SeqCst);
        let _ = events.send(HostEvent::Connected);
        attempt = 1;
        let mut last_activity = Instant::now();
        let mut trimmed = false;
        let mut projection_seen = None;
        let mut websites_seen = None;
        let mut lost = String::new();
        if let Some(command) = pending.take() {
            match command {
                HostCommand::Shutdown => {
                    link.host.kill();
                    return;
                }
                HostCommand::ToggleCurrentTemporaryWindow { .. } => {
                    let _ = events.send(HostEvent::TemporaryWindowToggleHandled(false));
                }
                command => {
                    if let Err(error) = run(command.clone(), &mut link.session, &ownership, &events)
                    {
                        lost = error.to_string();
                        if !matches!(
                            &command,
                            HostCommand::ToggleCurrentTemporaryWindow { .. }
                        ) {
                            pending = Some(command);
                        }
                    }
                }
            }
        }
        if !lost.is_empty() {
            connected.store(false, Ordering::SeqCst);
            link.host.kill();
            process.lock().unwrap().take();
            if !ownership.has_owners() {
                attempt = 0;
                continue;
            }
            let _ = events.send(HostEvent::Disconnected(lost));
            attempt += 1;
            continue;
        }
        loop {
            if cancelled.load(Ordering::SeqCst) {
                link.host.kill();
                return;
            }
            if let Err(error) = forward_completed_previews(&mut link.session, &events) {
                lost = error.to_string();
                break;
            }
            forward_confirmations(&mut link.session, &events);
            if link.session.closing {
                break;
            }
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(HostCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => {
                    link.host.kill();
                    eprintln!("host: stopped");
                    return;
                }
                Ok(command) => {
                    if let HostCommand::ToggleCurrentTemporaryWindow {
                        connection_generation: requested_generation,
                    } = &command
                    {
                        if *requested_generation != link_generation {
                            let _ = events.send(HostEvent::TemporaryWindowToggleHandled(false));
                            continue;
                        }
                    }
                    if let Err(error) = run(command.clone(), &mut link.session, &ownership, &events)
                    {
                        lost = error.to_string();
                        if !matches!(
                            &command,
                            HostCommand::ToggleCurrentTemporaryWindow { .. }
                        ) {
                            pending = Some(command);
                        }
                        break;
                    }
                    last_activity = Instant::now();
                    trimmed = false;
                    if link.session.closing {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Keep answering the manager while the UI is idle: it asks
                    // for renderer state and projections on its own schedule.
                    if let Err(error) = link.session.pump(Duration::from_millis(1)) {
                        lost = error.to_string();
                        break;
                    }
                    forward_features(
                        &mut link.session,
                        &events,
                        &mut projection_seen,
                        &projection_cache,
                    );
                    forward_websites(&link.session, &events, &mut websites_seen);
                    forward_results(&mut link.session, &events);
                    forward_confirmations(&mut link.session, &events);
                    if !trimmed && last_activity.elapsed() > idle_trim {
                        link.host.trim_working_set();
                        trimmed = true;
                        eprintln!("host: idle {:?}, working set trimmed", idle_trim);
                    }
                }
            }
        }
        connected.store(false, Ordering::SeqCst);
        let manager_requested_exit = link.session.closing;
        if manager_requested_exit {
            // An ownership-sync acknowledgement may announce exiting before
            // Electron's before-quit handler asks us to acknowledge closing.
            // Keep servicing that handshake while it completes its normal exit.
            while link.host.is_running() {
                if link.session.pump(Duration::from_millis(20)).is_err() {
                    break;
                }
            }
            if let Err(error) = link.host.wait_for_exit() {
                eprintln!("host: Manager exit wait failed: {error}");
            }
        }
        link.host.kill();
        process.lock().unwrap().take();
        if manager_requested_exit || !ownership.has_owners() {
            attempt = 0;
            if matches!(pending.as_ref(), Some(HostCommand::SyncOwnership)) {
                pending = None;
            }
            continue;
        }
        eprintln!("host: pipe lost ({lost})");
        let _ = events.send(HostEvent::Disconnected(lost));
        attempt += 1;
        if attempt >= MAX_ATTEMPTS {
            eprintln!("host: reconnect attempts exhausted");
            let _ = events.send(HostEvent::Disconnected(
                "reconnect attempts exhausted".into(),
            ));
            return;
        }
    }
}

fn wait_for_command(
    rx: &Receiver<HostCommand>,
    ownership: &webtools_ipc::HostOwnership,
    cancelled: &AtomicBool,
    events: &Sender<HostEvent>,
) -> Option<HostCommand> {
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return None;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(HostCommand::Shutdown) => return Some(HostCommand::Shutdown),
            Ok(HostCommand::ToggleCurrentTemporaryWindow { .. }) => {
                let _ = events.send(HostEvent::TemporaryWindowToggleHandled(false));
            }
            Ok(command) if ownership.has_owners() => return Some(command),
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

/// Binds a fresh pipe, spawns the manager and completes the hello handshake.
fn open_link(
    exe: &str,
    args: &[String],
    attempt: u32,
    cancelled: &Arc<AtomicBool>,
    process: &Mutex<Option<OwnedHandle>>,
    events: &Sender<HostEvent>,
    ownership: webtools_ipc::HostOwnership,
    launcher_state: Arc<Mutex<webtools_ipc::state::LauncherStateStore>>,
) -> Option<Link> {
    let started = Instant::now();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let digits = format!("{nanos:x}");
    let run_id = format!(
        "{}{attempt}",
        digits[digits.len().saturating_sub(10)..].to_string()
    );
    let temp_root = crate::config::profile_root().join("runtime");
    std::fs::create_dir_all(&temp_root).ok()?;
    let pipe = format!("WebTools.NativeHost.Manager.v1.Phase4E.{run_id}");
    let profile = temp_root.join("wt-manager-profile");
    std::fs::create_dir_all(&profile).ok()?;

    let server = webtools_ipc::PipeServer::bind(&pipe).ok()?;
    let options = webtools_ipc::HostOptions {
        exe: exe.into(),
        args: args.to_vec(),
        pipe,
        token: None,
        profile,
        trace_dir: None,
        temp_dir: Some(temp_root),
    };
    let mut host = webtools_ipc::HostProcess::launch(&options).ok()?;
    let handle = unsafe {
        windows_sys::Win32::System::Threading::OpenProcess(
            windows_sys::Win32::System::Threading::PROCESS_TERMINATE,
            0,
            host.pid(),
        )
    };
    if !handle.is_null() {
        *process.lock().unwrap() = Some(unsafe { OwnedHandle::from_raw_handle(handle as _) });
    }
    let spawned = started.elapsed();
    let Ok(connection) = server.accept_until(cancelled, Duration::from_secs(20)) else {
        host.kill();
        return None;
    };
    if connection.client_process_id().ok() != Some(host.pid()) {
        eprintln!("host: rejecting unexpected Manager pipe client");
        host.kill();
        return None;
    }
    let mut session =
        webtools_ipc::HostSession::with_shared_state(connection, launcher_state, ownership);
    session.set_settings_handler(settings_handler(events.clone(), cancelled.clone()));
    session.set_panel_handler(panel_handler(events.clone(), cancelled.clone()));
    session.set_surface_request_handler(Box::new({
        let events = events.clone();
        move |request| {
            let (reply, received) = mpsc::sync_channel(1);
            if events
                .send(HostEvent::SurfaceRequest { request, reply })
                .is_err()
            {
                return false;
            }
            received.recv().unwrap_or(false)
        }
    }));
    let deadline = Instant::now() + Duration::from_secs(20);
    while session.hello.is_none() && !cancelled.load(Ordering::SeqCst) && Instant::now() < deadline
    {
        if session.pump(Duration::from_millis(20)).is_err() {
            break;
        }
    }
    match session.hello.as_ref() {
        Some(hello) if !cancelled.load(Ordering::SeqCst) && hello.process_id == host.pid() => eprintln!(
            "host: spawned={spawned:?} hello={:?} caps={:?}",
            started.elapsed(),
            hello.capabilities
        ),
        _ => {
            eprintln!("host: hello failed or host stopping");
            host.kill();
            return None;
        }
    }
    let uses_host_lifecycle = session.hello.as_ref().is_some_and(|hello| {
        hello
            .capabilities
            .iter()
            .any(|capability| capability == "electron-host-lifecycle-v1")
    });
    while uses_host_lifecycle
        && !session.host_ready
        && !cancelled.load(Ordering::SeqCst)
        && Instant::now() < deadline
    {
        if session.pump(Duration::from_millis(20)).is_err() {
            break;
        }
    }
    if cancelled.load(Ordering::SeqCst) || uses_host_lifecycle && !session.host_ready {
        eprintln!("host: Electron services did not become ready before dispatch");
        host.kill();
        return None;
    }
    eprintln!("host: services-ready={:?}", started.elapsed());
    Some(Link { host, session })
}

/// Reports the projected feature list once per change.
fn forward_features(
    session: &mut webtools_ipc::HostSession,
    events: &Sender<HostEvent>,
    seen: &mut Option<(String, i64)>,
    projection_cache: &webtools_ipc::projection_cache::ProjectionCache,
) {
    let Some(projection) = session.projection() else {
        return;
    };
    if !projection_changed(seen, projection) {
        return;
    }
    if std::env::var("WT_PANEL_DEBUG").is_ok() {
        for row in &projection.features {
            let contexts: Vec<String> = row
                .accepted_contexts
                .iter()
                .map(|rule| format!("{:?}:{}", rule.kind, rule.sources.join("|")))
                .collect();
            eprintln!(
                "feature {}/{} [{}] contexts=[{}]",
                row.plugin_id,
                row.feature_id,
                row.title,
                contexts.join(", ")
            );
        }
    }
    let _ = events.send(HostEvent::Features(projection.features.clone()));
    if let Err(error) = projection_cache.save(projection) {
        eprintln!("host: feature projection cache save failed: {error}");
    }
}

/// Pushes any quick results the manager published onto the UI channel.
fn forward_results(session: &mut webtools_ipc::HostSession, events: &Sender<HostEvent>) {
    for result in session.take_results() {
        let _ = events.send(HostEvent::Result {
            request_id: result.request_id,
            plugin: result.plugin_id,
            feature: result.feature_id,
            text: result.text,
        });
    }
}

fn forward_confirmations(session: &mut webtools_ipc::HostSession, events: &Sender<HostEvent>) {
    for confirmation in session.take_confirmations() {
        let _ = events.send(HostEvent::AIConfirmation(confirmation));
    }
}

fn forward_completed_previews(
    session: &mut webtools_ipc::HostSession,
    events: &Sender<HostEvent>,
) -> webtools_ipc::Result<()> {
    let completed = session.take_completed_quick_invokes();
    if completed.is_empty() {
        return Ok(());
    }
    for (request_id, ack) in completed {
        let outcome = ack
            .result
            .as_ref()
            .and_then(|value| webtools_ipc::contract::parse_invoke_result(value, &request_id));
        let (status, reason) = outcome
            .map(|outcome| (outcome.status.to_string(), outcome.reason_code.to_string()))
            .unwrap_or_else(|| ("failed".into(), "dispatch-failed".into()));
        let _ = events.send(HostEvent::PreviewFinished {
            request_id,
            status,
            reason,
        });
    }
    session.sync_ownership()?;
    Ok(())
}

fn forward_websites(
    session: &webtools_ipc::HostSession,
    events: &Sender<HostEvent>,
    seen: &mut Option<serde_json::Value>,
) {
    let state = session.launcher_state_snapshot();
    let websites = state
        .get("websites")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    if seen.as_ref() == Some(&websites) {
        return;
    }
    *seen = Some(websites.clone());
    let _ = events.send(HostEvent::Websites(websites));
}

fn run(
    command: HostCommand,
    session: &mut webtools_ipc::HostSession,
    ownership: &webtools_ipc::HostOwnership,
    events: &Sender<HostEvent>,
) -> webtools_ipc::Result<()> {
    match command {
        HostCommand::Shutdown => {}
        HostCommand::ToggleCurrentTemporaryWindow { .. } => {
            if let Some(process_id) = session.hello.as_ref().map(|hello| hello.process_id) {
                crate::visual::allow_foreground_process(process_id);
            }
            match session.toggle_current_temporary_window() {
                Ok(handled) => {
                    let _ = events.send(HostEvent::TemporaryWindowToggleHandled(handled));
                }
                Err(error) => {
                    let _ = events.send(HostEvent::TemporaryWindowToggleHandled(false));
                    return Err(error);
                }
            }
        }
        HostCommand::SyncOwnership => {
            let ack = session.sync_ownership()?;
            if !ack.ok {
                return Err(webtools_ipc::ProtocolError::Malformed(
                    "host ownership sync rejected",
                ));
            }
        }
        HostCommand::OpenPage {
            section,
            request_id,
            end_session_after,
        } => {
            let started = Instant::now();
            let ack = session.open_page(&request_id, section)?;
            eprintln!(
                "host: open-page({section}) ok={} in {:?}",
                ack.ok,
                started.elapsed()
            );
            if let Some(id) = end_session_after {
                ownership.end_session(&id);
                let _ = session.sync_ownership()?;
                let _ = events.send(HostEvent::SessionEnded(id));
            }
        }
        HostCommand::Invoke {
            request_id,
            source,
            plugin,
            feature,
            selection,
            launcher_text,
            quick_result,
            end_session_after,
        } => {
            let started = Instant::now();
            let result = match selection.as_ref() {
                Some(selection)
                    if selection.kind == webtools_ipc::projection::ContextKind::Text =>
                {
                    session.feature_invoke_with_context(
                        &request_id,
                        &plugin,
                        &feature,
                        selection.text.as_deref().unwrap_or(""),
                        quick_result,
                    )
                }
                Some(selection) => {
                    session.feature_invoke_with_selection(&request_id, &plugin, &feature, selection)
                }
                None if launcher_text.is_some() => session.feature_invoke_with_launcher_text(
                    &request_id,
                    &plugin,
                    &feature,
                    launcher_text.as_deref().unwrap_or_default(),
                ),
                None => session.feature_invoke(&request_id, &plugin, &feature, source),
            };
            let ack = result?;
            {
                let outcome = ack.result.as_ref().and_then(|value| {
                    webtools_ipc::contract::parse_invoke_result(value, &request_id)
                });
                match outcome {
                    Some(outcome) => eprintln!(
                        "host: invoke({plugin}:{feature}) status={} reason={} dismiss={} in {:?}",
                        outcome.status,
                        outcome.reason_code,
                        outcome.dismiss_panel,
                        started.elapsed()
                    ),
                    None => eprintln!(
                        "host: invoke({plugin}:{feature}) ok={} in {:?} result={:?}",
                        ack.ok,
                        started.elapsed(),
                        ack.result
                    ),
                }
            }
            if let Some(id) = end_session_after {
                ownership.end_session(&id);
                let _ = session.sync_ownership()?;
                let _ = events.send(HostEvent::SessionEnded(id));
            }
        }
        HostCommand::Translation {
            request_id,
            text,
            source,
            end_session_after,
        } => {
            let started = Instant::now();
            let ack = session.translation_prefill(&request_id, &text, source)?;
            eprintln!(
                "host: translation-prefill ok={} in {:?}",
                ack.ok,
                started.elapsed()
            );
            if let Some(id) = end_session_after {
                ownership.end_session(&id);
                let _ = session.sync_ownership()?;
                let _ = events.send(HostEvent::SessionEnded(id));
            }
        }
        HostCommand::CancelResult {
            request_id,
            plugin,
            feature,
            session_id,
        } => {
            let ack = session.cancel_quick_result(&request_id, &plugin, &feature)?;
            eprintln!("host: feature-result-cancel ok={}", ack.ok);
            ownership.end_session(&session_id);
            let _ = session.sync_ownership()?;
            let _ = events.send(HostEvent::SessionEnded(session_id));
        }
        HostCommand::AIConsentDecision {
            confirmation,
            approved,
        } => {
            let ack = session.decide_ai_confirmation(&confirmation, approved)?;
            eprintln!(
                "host: feature-ai-decision approved={approved} ok={}",
                ack.ok
            );
        }
        HostCommand::PreviewTranslation { request_id, text } => {
            if !session.begin_feature_quick_result(
                &request_id,
                "webtools.translation",
                "translate-text",
                &text,
            )? {
                let _ = events.send(HostEvent::PreviewFinished {
                    request_id,
                    status: "rejected".into(),
                    reason: "dispatch-failed".into(),
                });
            }
        }
    }
    Ok(())
}
