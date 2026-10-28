use serde_json::json;
use std::time::Duration;
use webtools_ipc::{Connection, Envelope, HostSession, PipeServer};

#[test]
fn missing_panel_authority_is_reported_as_capability_required() {
    let name = format!("WebTools.R2.panel.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let thread = std::thread::spawn(move || {
        let mut session = HostSession::new(server.accept().unwrap());
        session.pump(Duration::from_secs(2)).unwrap();
    });
    let ack = Connection::connect(&name, Duration::from_secs(2))
        .unwrap()
        .request(
            &Envelope::new("super-panel-settings-get", json!({})),
            Duration::from_secs(2),
        )
        .unwrap();
    thread.join().unwrap();
    assert!(!ack.ok);
    assert_eq!(ack.error.unwrap().code, "CAPABILITY_REQUIRED");
}

use webtools_ipc::super_panel::*;
#[test]
fn only_the_wpf_settings_shape_and_hold_range_are_accepted() {
    let valid =
        json!({"enabled":true,"mouseButton":"xbutton1","gesture":"press","holdMilliseconds":300});
    assert!(
        !PanelSettings::parse(&valid)
            .unwrap()
            .auto_translation_preview
    );
    for (key, value) in [
        ("holdMilliseconds", json!(299)),
        ("holdMilliseconds", json!(1501)),
        ("mouseButton", json!("left")),
        ("gesture", json!("double")),
        ("enabled", json!(1)),
        ("extra", json!(true)),
    ] {
        let mut bad = valid.clone();
        bad[key] = value;
        assert!(PanelSettings::parse(&bad).is_none(), "{key}");
    }
}
struct Profile(std::path::PathBuf);
impl Profile {
    fn new() -> Self {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/r2-panel-state-tests")
            .join(webtools_ipc::new_request_id());
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn pin() -> FeatureRef {
    FeatureRef {
        plugin_id: "webtools.translation".into(),
        feature_id: "translate-text".into(),
    }
}

#[test]
fn settings_and_pins_share_one_restartable_file() {
    let p = Profile::new();
    let mut store = PanelStateStore::new(&p.0);
    store.load().unwrap();
    assert!(!p.0.join("super-panel-state.json").exists());
    let mut state = store.snapshot();
    state.pinned.push(pin());
    state.settings.enabled = false;
    state.settings.auto_translation_preview = true;
    store.save(state.clone()).unwrap();
    let mut reopened = PanelStateStore::new(&p.0);
    reopened.load().unwrap();
    assert_eq!(reopened.snapshot(), state);
    state.settings.mouse_button = MouseButton::Xbutton2;
    reopened.save(state).unwrap();
    assert_eq!(reopened.snapshot().pinned, vec![pin()]);
}
#[test]
fn legacy_wpf_trigger_schema_migrates_without_registering_removed_keyboard_or_rewriting() {
    let p = Profile::new();
    let source=json!({"schemaVersion":1,"enabled":true,"keyboardShortcut":"Alt+Q","middleHoldEnabled":false,"holdMilliseconds":700,"pinned":[pin()]}).to_string();
    std::fs::write(p.0.join("super-panel-state.json"), &source).unwrap();
    let mut store = PanelStateStore::new(&p.0);
    store.load().unwrap();
    assert!(!store.snapshot().settings.enabled);
    assert_eq!(store.snapshot().settings.mouse_button, MouseButton::Middle);
    assert_eq!(store.snapshot().pinned, vec![pin()]);
    assert_eq!(
        std::fs::read_to_string(p.0.join("super-panel-state.json")).unwrap(),
        source
    );
}
#[test]
fn experimental_tuple_pins_are_imported_once_and_titles_resolve_from_projection() {
    let p = Profile::new();
    let legacy = json!([
        ["webtools.translation", "translate-text", "旧标题"],
        ["webtools.translation", "translate-text", "重复"]
    ])
    .to_string();
    std::fs::write(p.0.join("panel-pins.json"), &legacy).unwrap();
    let mut store = PanelStateStore::new(&p.0);
    store.load().unwrap();
    assert_eq!(store.snapshot().pinned, vec![pin()]);
    store.save(store.snapshot()).unwrap();
    std::fs::write(p.0.join("panel-pins.json"), "[]").unwrap();
    let mut next = PanelStateStore::new(&p.0);
    next.load().unwrap();
    assert_eq!(next.snapshot().pinned, vec![pin()]);
}
#[test]
fn invalid_files_and_failed_replace_preserve_original_state() {
    use std::os::windows::fs::OpenOptionsExt;
    let p = Profile::new();
    let path = p.0.join("super-panel-state.json");
    std::fs::write(&path, "{broken").unwrap();
    let mut store = PanelStateStore::new(&p.0);
    assert!(store.load().is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    store.save(store.snapshot()).unwrap();
    let old = store.snapshot();
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&path)
        .unwrap();
    let mut next = old.clone();
    next.settings.enabled = false;
    assert!(store.save(next).is_err());
    assert_eq!(store.snapshot(), old);
    drop(lock);
}
#[test]
fn requests_negotiate_and_preserve_changed_notification_while_waiting_for_next_reply() {
    let name = format!("WebTools.R2.panel.sync.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let thread = std::thread::spawn(move || {
        let mut session = HostSession::new(server.accept().unwrap());
        let mut settings = PanelSettings::default();
        session.set_panel_handler(Box::new(move |op| {
            if let PanelOperation::Update(next) = op {
                settings = next;
            }
            Ok(PanelStatus::new(settings.clone(), true, None))
        }));
        for _ in 0..3 {
            assert!(session.pump(Duration::from_secs(3)).unwrap());
        }
    });
    let mut client = Connection::connect(&name, Duration::from_secs(3)).unwrap();
    let hello = client
        .request(
            &Envelope::new("hello", json!({"capabilities":[CAPABILITY]})),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(
        hello.result.unwrap()["supportedFeatures"],
        json!([CAPABILITY])
    );
    let update = PanelSettings {
        enabled: false,
        ..Default::default()
    };
    let changed = client
        .request(
            &Envelope::new(
                "super-panel-settings-update",
                serde_json::to_value(update).unwrap(),
            ),
            Duration::from_secs(3),
        )
        .unwrap();
    assert!(changed.ok);
    assert_eq!(changed.result.unwrap()["triggerStatus"], "disabled");
    let read = client
        .request(
            &Envelope::new("super-panel-settings-get", json!({})),
            Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(
        read.result.unwrap()["activeTrigger"],
        serde_json::Value::Null
    );
    let notification = client.next_event(Duration::from_secs(1)).unwrap().unwrap();
    assert_eq!(notification.kind, "super-panel-settings-changed");
    thread.join().unwrap();
}
