#[test]
fn default_search_engine_can_substitute_a_query_using_the_manager_contract() {
    let state = webtools_ipc::session::default_launcher_state();
    let template = state["searchEngines"][0]["template"].as_str().unwrap();
    assert_eq!(
        template.matches("%s").count(),
        1,
        "Manager substitutes %s; a different placeholder silently breaks web search"
    );
    assert_eq!(state["quickSearchShortcut"], "Control+Alt+Space");
}

use serde_json::json;
use webtools_ipc::state::LauncherStateStore;

#[test]
fn wpf_url_paths_and_queries_may_contain_spaces() {
    let profile = Profile::new();
    let mut store = LauncherStateStore::load(&profile.0).unwrap();
    store.replace_websites(&json!([{"id":"space","name":"带空格","url":"https://example.org/search?q=hello world","description":"","folderIds":[]}])).unwrap();
    assert_eq!(
        LauncherStateStore::load(&profile.0).unwrap().snapshot()["websites"][0]["id"],
        "space"
    );
}

#[test]
fn memory_normalization_excludes_letter_numbers_other_numbers_and_combining_marks() {
    assert_eq!(
        webtools_ipc::state::normalize_query("A〇\u{0345} Café\u{0663}"),
        "acafe٣"
    );
    assert_eq!(webtools_ipc::state::normalize_query("∮ Editor"), "editor");
}

#[test]
fn a_malformed_legacy_memory_record_does_not_discard_valid_settings() {
    let profile = Profile::new();
    std::fs::write(profile.0.join("nook-data.json"),json!({"version":2,"settings":{"theme":"light"},"appSearchMemory":{"editor":{},"good":{"appId":"0123456789abcdef","lastUsedAt":2}}}).to_string()).unwrap();
    let state = LauncherStateStore::load(&profile.0).unwrap().snapshot();
    assert_eq!(state["theme"], "light");
    assert_eq!(state["appSearchMemory"].as_array().unwrap().len(), 1);
}
struct Profile(std::path::PathBuf);
impl Profile {
    fn new() -> Self {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/r2-state-tests")
            .join(webtools_ipc::new_request_id());
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn saved_settings_websites_and_normalized_memory_survive_a_fresh_store() {
    let profile = Profile::new();
    let mut store = LauncherStateStore::load(&profile.0).unwrap();
    store
        .apply_settings(&json!({"theme":"light","launcherDisplayMode":"expanded"}))
        .unwrap();
    store.replace_websites(&json!([{"id":"one","name":"网站","url":"https://example.com","description":"简介","folderIds":["a","a"]}])).unwrap();
    store
        .remember("  Café-编辑器  ", "0123456789abcdef")
        .unwrap();
    drop(store);
    let reloaded = LauncherStateStore::load(&profile.0).unwrap();
    assert_eq!(reloaded.snapshot()["theme"], "light");
    assert_eq!(reloaded.snapshot()["launcherDisplayMode"], "expanded");
    assert_eq!(
        reloaded.snapshot()["websites"][0]["folderIds"],
        json!(["a"])
    );
    assert_eq!(
        reloaded.remembered("CAFÉ 编辑器"),
        Some("0123456789abcdef".into())
    );
    assert!(profile.0.join("launcher-state.json").exists());
}

#[test]
fn a_rejected_update_preserves_both_disk_and_live_snapshot() {
    let profile = Profile::new();
    let mut store = LauncherStateStore::load(&profile.0).unwrap();
    let before = std::fs::read(profile.0.join("launcher-state.json")).unwrap();
    assert!(store.apply_settings(&json!({"theme":"unknown"})).is_err());
    assert!(store.replace_websites(&json!([{"id":"x","name":"bad","url":"javascript:alert(1)","description":"","folderIds":[]}])).is_err());
    assert!(store.remember("test", "not-an-app-id").is_err());
    assert_eq!(
        std::fs::read(profile.0.join("launcher-state.json")).unwrap(),
        before
    );
    assert_eq!(store.snapshot()["theme"], "dark");
}

#[test]
fn failed_disk_write_does_not_acknowledge_a_change_in_memory() {
    let profile = Profile::new();
    let mut store = LauncherStateStore::load(&profile.0).unwrap();
    std::fs::remove_file(profile.0.join("launcher-state.json")).unwrap();
    std::fs::remove_dir(&profile.0).unwrap();
    std::fs::write(&profile.0, b"not a directory").unwrap();
    assert!(store.apply_settings(&json!({"theme":"light"})).is_err());
    assert_eq!(store.snapshot()["theme"], "dark");
    std::fs::remove_file(&profile.0).unwrap();
}

#[test]
fn legacy_v2_import_runs_only_before_the_first_native_state_file() {
    let profile = Profile::new();
    let legacy = json!({"version":2,"settings":{"theme":"light"},"webEntries":[],
        "appSearchMemory":{"editor":{"appId":"fedcba9876543210","lastUsedAt":12}}});
    std::fs::write(profile.0.join("nook-data.json"), legacy.to_string()).unwrap();
    let store = LauncherStateStore::load(&profile.0).unwrap();
    assert_eq!(store.snapshot()["theme"], "light");
    assert_eq!(store.remembered("EDITOR"), Some("fedcba9876543210".into()));
    drop(store);
    std::fs::write(
        profile.0.join("nook-data.json"),
        json!({"version":2,"settings":{"theme":"system"}}).to_string(),
    )
    .unwrap();
    assert_eq!(
        LauncherStateStore::load(&profile.0).unwrap().snapshot()["theme"],
        "light"
    );
}

#[test]
fn legacy_optional_website_fields_and_a_disabled_default_do_not_drop_user_data() {
    let profile = Profile::new();
    let legacy = json!({"version":2,"settings":{"theme":"light","defaultSearchEngineId":"off","searchEngines":[
        {"id":"off","name":"Disabled","template":"https://example.com/?q=%s","enabled":false},
        {"id":"on","name":"Enabled","template":"https://example.org/?q=%s"}]},
        "webEntries":[{"id":"legacy","name":"保留网址","url":"https://example.org"}]});
    let original = legacy.to_string();
    std::fs::write(profile.0.join("nook-data.json"), &original).unwrap();
    let state = LauncherStateStore::load(&profile.0).unwrap().snapshot();
    assert_eq!(state["theme"], "light");
    assert_eq!(state["defaultSearchEngineId"], "on");
    assert_eq!(state["websites"][0]["name"], "保留网址");
    assert_eq!(state["websites"][0]["description"], "");
    assert_eq!(state["websites"][0]["folderIds"], json!([]));
    assert_eq!(
        std::fs::read_to_string(profile.0.join("nook-data.json")).unwrap(),
        original
    );
}

#[test]
fn corrupt_primary_recovers_the_last_valid_backup() {
    let profile = Profile::new();
    let mut store = LauncherStateStore::load(&profile.0).unwrap();
    store.apply_settings(&json!({"theme":"light"})).unwrap();
    std::fs::write(profile.0.join("launcher-state.json"), b"{broken").unwrap();
    assert_eq!(
        LauncherStateStore::load(&profile.0).unwrap().snapshot()["theme"],
        "dark"
    );
}

#[test]
fn real_pipe_hello_uses_persisted_state_and_memory_updates_are_not_noops() {
    use std::time::Duration;
    use webtools_ipc::{Connection, Envelope, HostSession, PipeServer};
    let profile = Profile::new();
    let mut store = LauncherStateStore::load(&profile.0).unwrap();
    store.apply_settings(&json!({"theme":"light"})).unwrap();
    let name = format!("WebTools.R2.state.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let host = std::thread::spawn(move || {
        let mut session = HostSession::with_state_store(server.accept().unwrap(), store);
        for _ in 0..5 {
            assert!(session.pump(Duration::from_secs(3)).unwrap());
        }
    });
    let mut client = Connection::connect(&name, Duration::from_secs(3)).unwrap();
    let hello = client
        .request(
            &Envelope::new(
                "hello",
                json!({"processId":std::process::id(),"capabilities":[]}),
            ),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(hello.result.unwrap()["state"]["theme"], "light");
    let remember = client
        .request(
            &Envelope::new(
                "app-memory-remember",
                json!({"query":"Editor","appId":"0123456789abcdef"}),
            ),
            Duration::from_secs(3),
        )
        .unwrap();
    assert!(remember.ok);
    let read = client
        .request(
            &Envelope::new("app-memory-get", json!({"query":"EDITOR"})),
            Duration::from_secs(3),
        )
        .unwrap();
    assert_eq!(read.result.unwrap()["appId"], "0123456789abcdef");
    let bad = client
        .request(
            &Envelope::new("websites-update", json!({"websites":[{}]})),
            Duration::from_secs(3),
        )
        .unwrap();
    assert!(!bad.ok);
    assert_eq!(bad.error.unwrap().code, "INVALID_PAYLOAD");
    let bad_memory = client
        .request(
            &Envelope::new("app-memory-remember", json!({"query":"x","appId":"bad"})),
            Duration::from_secs(3),
        )
        .unwrap();
    assert!(!bad_memory.ok);
    assert_eq!(bad_memory.error.unwrap().code, "INVALID_APP_MEMORY");
    host.join().unwrap();
    assert_eq!(
        LauncherStateStore::load(&profile.0)
            .unwrap()
            .remembered("editor"),
        Some("0123456789abcdef".into())
    );
}
