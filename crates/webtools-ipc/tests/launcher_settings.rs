use serde_json::{json, Value};
use std::time::Duration;
use webtools_ipc::{Connection, Envelope, HostSession, PipeServer};

fn request_update(payload: Value) -> webtools_ipc::Ack {
    let name = format!("WebTools.R2.settings.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let host = std::thread::spawn(move || {
        let mut session = HostSession::new(server.accept().unwrap());
        assert!(session.pump(Duration::from_secs(3)).unwrap());
    });
    let mut client = Connection::connect(&name, Duration::from_secs(3)).unwrap();
    let ack = client
        .request(
            &Envelope::new("launcher-settings-update", payload),
            Duration::from_secs(3),
        )
        .unwrap();
    host.join().unwrap();
    ack
}

#[test]
fn invalid_shortcut_reports_the_wpf_error_instead_of_unknown_message() {
    let ack = request_update(json!({"quickSearchShortcut":"not-a-shortcut"}));
    assert!(!ack.ok);
    assert_eq!(ack.error.unwrap().code, "INVALID_HOTKEY");
}

use webtools_ipc::{
    settings::{SettingsError, SettingsHandler, SettingsPhase},
    state::LauncherStateStore,
};
struct Profile(std::path::PathBuf);
impl Profile {
    fn new() -> Self {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/r2-settings-tests")
            .join(webtools_ipc::new_request_id());
        std::fs::create_dir_all(&p).unwrap();
        LauncherStateStore::load(&p).unwrap();
        Self(p)
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn update(profile: &Profile, payload: Value, handler: SettingsHandler) -> webtools_ipc::Ack {
    let store = LauncherStateStore::load(&profile.0).unwrap();
    let name = format!("WebTools.R2.apply.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let host = std::thread::spawn(move || {
        let mut session = HostSession::with_state_store(server.accept().unwrap(), store);
        session.set_settings_handler(handler);
        assert!(session.pump(Duration::from_secs(3)).unwrap());
    });
    let ack = Connection::connect(&name, Duration::from_secs(3))
        .unwrap()
        .request(
            &Envelope::new("launcher-settings-update", payload),
            Duration::from_secs(3),
        )
        .unwrap();
    host.join().unwrap();
    ack
}

#[test]
fn successful_update_acks_the_saved_state_and_a_new_store_restores_it() {
    let profile = Profile::new();
    let ack = update(
        &profile,
        json!({"theme":"light","launcherDisplayMode":"expanded","quickSearchShortcut":"Ctrl+Alt+F9","everythingEnabled":true}),
        Box::new(|_, _| Ok(())),
    );
    assert!(ack.ok);
    let saved = ack.result.unwrap();
    assert_eq!(saved["theme"], "light");
    let reloaded = LauncherStateStore::load(&profile.0).unwrap().snapshot();
    assert_eq!(reloaded, saved);
    assert_eq!(reloaded["quickSearchShortcut"], "Ctrl+Alt+F9");
    assert_eq!(reloaded["launcherDisplayMode"], "expanded");
    assert_eq!(reloaded["everythingEnabled"], true);
}

#[test]
fn occupied_shortcut_does_not_change_theme_or_file() {
    let profile = Profile::new();
    let before = std::fs::read(profile.0.join("launcher-state.json")).unwrap();
    let ack = update(
        &profile,
        json!({"theme":"light","quickSearchShortcut":"Ctrl+Alt+F9"}),
        Box::new(|_, _| Err(SettingsError::hotkey_unavailable())),
    );
    assert!(!ack.ok);
    assert_eq!(ack.error.unwrap().code, "HOTKEY_UNAVAILABLE");
    assert_eq!(
        std::fs::read(profile.0.join("launcher-state.json")).unwrap(),
        before
    );
}

#[test]
fn disk_replace_failure_rolls_back_prepared_native_resources_and_never_commits() {
    use std::os::windows::fs::OpenOptionsExt;
    let profile = Profile::new();
    let before = std::fs::read(profile.0.join("launcher-state.json")).unwrap();
    let _lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(profile.0.join("launcher-state.json"))
        .unwrap();
    let phases = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = phases.clone();
    let ack = update(
        &profile,
        json!({"theme":"light"}),
        Box::new(move |phase, _| {
            captured.lock().unwrap().push(phase);
            Ok(())
        }),
    );
    assert!(!ack.ok);
    assert_eq!(ack.error.unwrap().code, "INVALID_PAYLOAD");
    assert_eq!(
        *phases.lock().unwrap(),
        vec![SettingsPhase::Prepare, SettingsPhase::Rollback]
    );
    assert_eq!(
        std::fs::read(profile.0.join("launcher-state.json")).unwrap(),
        before
    );
}

#[test]
fn native_commit_failure_restores_persisted_settings_before_returning_error() {
    let profile = Profile::new();
    let ack = update(
        &profile,
        json!({"theme":"light"}),
        Box::new(|phase, _| {
            if phase == SettingsPhase::Commit {
                Err(SettingsError::unavailable())
            } else {
                Ok(())
            }
        }),
    );
    assert!(!ack.ok);
    assert_eq!(
        LauncherStateStore::load(&profile.0).unwrap().snapshot()["theme"],
        "dark"
    );
}

#[test]
fn invalid_payload_does_not_reach_native_application() {
    let profile = Profile::new();
    let ack = update(
        &profile,
        json!({"theme":true}),
        Box::new(|_, _| panic!("invalid payload must not reserve a hotkey")),
    );
    assert!(!ack.ok);
    assert_eq!(ack.error.unwrap().code, "INVALID_PAYLOAD");
}
