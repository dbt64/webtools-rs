use serde_json::json;
use std::{fs, path::PathBuf};
use webtools_ipc::mcp::McpToolsProjection;
use webtools_ipc::mcp_store::{mcp_profile_path, McpProjectionStore, McpSettingsStore};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "wt-mcp-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn settings(enabled: bool, token: &str) -> serde_json::Value {
    json!({"enabled":enabled,"port":24459,"token":token,"disabledPlugins":["org.test.disabled"]})
}
const TOKEN: &str = "preserved-token-12345678901234567890";

#[test]
fn production_uses_actual_manager_profile_instead_of_a_stale_nested_test_profile() {
    let root = Root::new();
    let nested = mcp_profile_path(&root.0);
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        root.0.join("plugin-mcp-settings.json"),
        settings(true, TOKEN).to_string(),
    )
    .unwrap();
    fs::write(
        nested.join("plugin-mcp-settings.json"),
        settings(false, "stale-test-token-12345678901234567890").to_string(),
    )
    .unwrap();
    let mut store = McpSettingsStore::load_at_profile(&root.0, &root.0).unwrap();
    assert!(store.value.enabled);
    assert_eq!(store.value.token, TOKEN);
    store.set_enabled(false).unwrap();
    let actual: serde_json::Value =
        serde_json::from_slice(&fs::read(root.0.join("plugin-mcp-settings.json")).unwrap())
            .unwrap();
    assert_eq!(actual["enabled"], false);
    assert_eq!(actual["token"], TOKEN);
}

#[test]
fn canonical_full_config_wins_over_root_marker_and_preserves_credentials() {
    let root = Root::new();
    let profile = mcp_profile_path(&root.0);
    fs::create_dir_all(&profile).unwrap();
    fs::write(
        profile.join("plugin-mcp-settings.json"),
        settings(true, TOKEN).to_string(),
    )
    .unwrap();
    fs::write(
        root.0.join("plugin-mcp-settings.json"),
        json!({"enabled":false}).to_string(),
    )
    .unwrap();
    let mut store = McpSettingsStore::load_or_migrate(&root.0).unwrap();
    assert!(store.value.enabled);
    assert_eq!(store.value.token, TOKEN);
    assert_eq!(store.value.disabled_plugins, vec!["org.test.disabled"]);
    store.set_enabled(false).unwrap();
    store.set_plugin_enabled("org.test.disabled", true).unwrap();
    let restored = McpSettingsStore::load_or_migrate(&root.0).unwrap();
    assert!(!restored.value.enabled);
    assert!(restored.value.disabled_plugins.is_empty());
    assert_eq!(restored.value.token, TOKEN);
}
#[test]
fn migrates_complete_root_config_without_replacing_original() {
    let root = Root::new();
    let original = settings(true, TOKEN).to_string();
    fs::write(root.0.join("plugin-mcp-settings.json"), &original).unwrap();
    let store = McpSettingsStore::load_or_migrate(&root.0).unwrap();
    assert_eq!(store.value.token, TOKEN);
    assert!(store.value.enabled);
    assert_eq!(
        fs::read_to_string(root.0.join("plugin-mcp-settings.json")).unwrap(),
        original
    );
    assert!(mcp_profile_path(&root.0)
        .join("plugin-mcp-settings.json")
        .exists());
}
#[test]
fn absent_settings_or_boolean_marker_do_not_enable_service() {
    for marker in [false, true] {
        let root = Root::new();
        if marker {
            fs::write(
                root.0.join("plugin-mcp-settings.json"),
                "{\"enabled\":true}",
            )
            .unwrap();
        }
        let first = McpSettingsStore::load_or_migrate(&root.0).unwrap();
        assert!(!first.value.enabled);
        assert_eq!(first.value.token.len(), 64);
        let second = McpSettingsStore::load_or_migrate(&root.0).unwrap();
        assert_eq!(first.value.token, second.value.token);
    }
}
#[test]
fn corrupted_canonical_file_is_not_overwritten_or_replaced_with_root_config() {
    let root = Root::new();
    let profile = mcp_profile_path(&root.0);
    fs::create_dir_all(&profile).unwrap();
    fs::write(profile.join("plugin-mcp-settings.json"), "broken").unwrap();
    fs::write(
        root.0.join("plugin-mcp-settings.json"),
        settings(true, TOKEN).to_string(),
    )
    .unwrap();
    assert!(McpSettingsStore::load_or_migrate(&root.0).is_err());
    assert_eq!(
        fs::read_to_string(profile.join("plugin-mcp-settings.json")).unwrap(),
        "broken"
    );
}
#[test]
fn failed_save_does_not_commit_changed_preference() {
    let root = Root::new();
    let mut store = McpSettingsStore::load_or_migrate(&root.0).unwrap();
    let path = mcp_profile_path(&root.0).join("plugin-mcp-settings.json");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(store.set_enabled(true).is_err());
    assert!(!store.value.enabled);
}
#[test]
fn projection_accepts_valid_empty_catalog_and_rejects_stale_or_oversized_updates() {
    let root = Root::new();
    let path = root.0.join("projection.json");
    let mut store = McpProjectionStore::load(path.clone()).unwrap();
    assert!(store.snapshot().is_none());
    store.begin_generation(1);
    let empty = McpToolsProjection {
        schema_version: 1,
        revision: 2,
        tools: vec![],
    };
    store.replace_from(1, empty.clone()).unwrap();
    assert!(store.snapshot().unwrap().tools.is_empty());
    store.begin_generation(2);
    assert!(store.replace_from(1, empty.clone()).is_err());
    let mut new = empty.clone();
    new.revision = 1;
    store.replace_from(2, new.clone()).unwrap();
    assert!(store.replace_from(2, new.clone()).is_err());
    let mut huge: serde_json::Value = json!({"schemaVersion":1,"revision":3,"tools":[{"pluginId":"org.test.plugin","featureId":"note","title":"Note","description":"","inputSchema":{"type":"object"}}]});
    huge["tools"][0]["description"] = serde_json::Value::String("x".repeat(4 * 1024 * 1024));
    assert!(store
        .replace_from(2, serde_json::from_value(huge).unwrap())
        .is_err());
    assert_eq!(store.snapshot().unwrap().revision, 1);
    assert_eq!(
        McpProjectionStore::load(path)
            .unwrap()
            .snapshot()
            .unwrap()
            .revision,
        1
    );
}
#[test]
fn damaged_projection_requires_sync_instead_of_becoming_a_valid_empty_catalog() {
    let root = Root::new();
    let path = root.0.join("projection.json");
    fs::write(&path, "bad").unwrap();
    let store = McpProjectionStore::load(path).unwrap();
    assert!(store.snapshot().is_none());
}
