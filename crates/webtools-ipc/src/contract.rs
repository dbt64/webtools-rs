//! Validation for the feature-invocation contract.
//!
//! Both checks mirror the native host's own validators so the Rust side can
//! reject a payload before it reaches the manager instead of learning about it
//! through a generic ack failure:
//!
//! * requests  -> NativeFeatureInvocationProtocol.IsValidRequest
//! * responses -> NativeFeatureInvocationProtocol.TryParseResult

use serde_json::Value;

pub const MAX_INVOKE_BYTES_WITH_CONTEXT: usize = 16 * 1024;
pub const MAX_INVOKE_BYTES_WITHOUT_CONTEXT: usize = 1024;
pub const MAX_RESULT_BYTES: usize = 256;
pub const MAX_TEXT_LAUNCHER: usize = 300;
pub const MAX_TEXT_SUPER_PANEL: usize = 3000;
pub const MAX_ITEMS: usize = 64;

const REJECTED_REASONS: [&str; 9] = [
    "invalid-request",
    "feature-not-found",
    "plugin-disabled",
    "permission-denied",
    "feature-unavailable",
    "stale-request",
    "unsupported-feature-kind",
    "queue-full",
    "manager-not-ready",
];

const FAILED_REASONS: [&str; 5] = [
    "manager-timeout",
    "manager-disconnected",
    "dispatch-failed",
    "command-failed",
    "command-timeout",
];

const RESERVED_PLUGIN_IDS: [&str; 3] = ["__proto__", "prototype", "constructor"];

/// A validated feature-invoke result payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvokeOutcome {
    pub status: String,
    pub reason_code: String,
    pub dismiss_panel: bool,
}

impl InvokeOutcome {
    pub fn accepted(&self) -> bool {
        self.status == "accepted"
    }

    pub fn completed(&self) -> bool {
        self.status == "completed"
    }
}

/// True when the payload satisfies every rule the native host enforces.
pub fn is_valid_feature_invoke(payload: &Value) -> bool {
    let Some(object) = payload.as_object() else {
        return false;
    };

    let Some(request_id) = object.get("requestId").and_then(Value::as_str) else {
        return false;
    };
    if !is_request_id(request_id) {
        return false;
    }

    let Some(reference) = object.get("ref") else {
        return false;
    };
    if !is_valid_ref(reference) {
        return false;
    }

    let Some(source) = object.get("source").and_then(Value::as_str) else {
        return false;
    };
    if source != "launcher" && source != "super-panel" {
        return false;
    }

    let context = object.get("context").filter(|value| !value.is_null());
    let quick = object.get("quickResult").filter(|value| !value.is_null());

    if let Some(quick) = quick {
        let version = quick.get("version").and_then(Value::as_u64);
        if version != Some(1) || source != "super-panel" {
            return false;
        }
    }

    if let Some(context) = context {
        if !is_valid_context(context, source) {
            return false;
        }
        if quick.is_some() && !is_selected_text(context) {
            return false;
        }
    }

    let limit = if context.is_some() {
        MAX_INVOKE_BYTES_WITH_CONTEXT
    } else {
        MAX_INVOKE_BYTES_WITHOUT_CONTEXT
    };
    serde_json::to_vec(payload)
        .map(|bytes| bytes.len() <= limit)
        .unwrap_or(false)
}

/// Parses the result member of a successful ack, mirroring the host's strict reader.
pub fn parse_invoke_result(value: &Value, expected_request_id: &str) -> Option<InvokeOutcome> {
    if !value.is_object() {
        return None;
    }
    if serde_json::to_vec(value)
        .map(|bytes| bytes.len() > MAX_RESULT_BYTES)
        .unwrap_or(true)
    {
        return None;
    }
    let object = value.as_object()?;

    let mut names = std::collections::HashSet::new();
    for name in object.keys() {
        if !matches!(
            name.as_str(),
            "requestId" | "status" | "reasonCode" | "dismissPanel"
        ) {
            return None;
        }
        if !names.insert(name.as_str()) {
            return None;
        }
    }
    let expected = if names.contains("dismissPanel") { 4 } else { 3 };
    if names.len() != expected {
        return None;
    }

    let request_id = object.get("requestId")?.as_str()?;
    if request_id != expected_request_id {
        return None;
    }
    let status = object.get("status")?.as_str()?.to_string();
    let reason_code = object.get("reasonCode")?.as_str()?.to_string();

    let mut dismiss_panel = false;
    if names.contains("dismissPanel") {
        if status != "completed" || reason_code != "completed" {
            return None;
        }
        if object.get("dismissPanel")?.as_bool()? != true {
            return None;
        }
        dismiss_panel = true;
    }

    let ok = match status.as_str() {
        "accepted" => reason_code == "accepted",
        "completed" => reason_code == "completed",
        "cancelled" => reason_code == "cancelled",
        "rejected" => REJECTED_REASONS.contains(&reason_code.as_str()),
        "failed" => FAILED_REASONS.contains(&reason_code.as_str()),
        _ => false,
    };
    ok.then_some(InvokeOutcome {
        status,
        reason_code,
        dismiss_panel,
    })
}

/// RFC-4122 style id, exactly the 8-4-4-4-12 shape the host validates.
pub fn is_request_id(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 5 {
        return false;
    }
    let lengths = [8usize, 4, 4, 4, 12];
    parts
        .iter()
        .zip(lengths)
        .all(|(part, len)| part.len() == len && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_valid_ref(reference: &Value) -> bool {
    let Some(object) = reference.as_object() else {
        return false;
    };
    let (Some(plugin), Some(feature)) = (
        object.get("pluginId").and_then(Value::as_str),
        object.get("featureId").and_then(Value::as_str),
    ) else {
        return false;
    };
    is_valid_plugin_id(plugin) && is_valid_feature_id(feature)
}

pub fn is_valid_plugin_id(value: &str) -> bool {
    let len = value.len();
    if !(3..=128).contains(&len) || RESERVED_PLUGIN_IDS.contains(&value) {
        return false;
    }
    is_dotted(value, &['.', '-'])
}

pub fn is_valid_feature_id(value: &str) -> bool {
    if value.is_empty() || value.len() > 64 {
        return false;
    }
    is_dotted(value, &['-'])
}

/// Lower-case alphanumeric segments joined by separators, no empty segments.
fn is_dotted(value: &str, separators: &[char]) -> bool {
    let mut previous_separator = true;
    for ch in value.chars() {
        if separators.contains(&ch) {
            if previous_separator {
                return false;
            }
            previous_separator = true;
        } else if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            previous_separator = false;
        } else {
            return false;
        }
    }
    !previous_separator
}

fn is_selected_text(context: &Value) -> bool {
    context.get("type").and_then(Value::as_str) == Some("text")
        && context.get("source").and_then(Value::as_str) == Some("selected-text")
}

fn is_valid_context(context: &Value, source: &str) -> bool {
    let Some(object) = context.as_object() else {
        return false;
    };
    let Some(kind) = object.get("type").and_then(Value::as_str) else {
        return false;
    };
    let Some(context_source) = object.get("source").and_then(Value::as_str) else {
        return false;
    };
    let captured_at = object.get("capturedAtUnixMs").and_then(Value::as_i64);
    if captured_at.is_none_or(|value| value < 0) {
        return false;
    }
    let items = object.get("items").filter(|value| !value.is_null());

    if kind == "text" {
        let expected = if source == "launcher" {
            "launcher-input"
        } else {
            "selected-text"
        };
        if context_source != expected || items.is_some() {
            return false;
        }
        let Some(text) = object.get("text").and_then(Value::as_str) else {
            return false;
        };
        if text.trim().is_empty() {
            return false;
        }
        let limit = if source == "launcher" {
            MAX_TEXT_LAUNCHER
        } else {
            MAX_TEXT_SUPER_PANEL
        };
        return text.len() <= limit;
    }

    if kind != "files" && kind != "directories" {
        return false;
    }
    if source != "super-panel" || context_source != "explorer-selection" {
        return false;
    }
    if object.get("text").is_some_and(|value| !value.is_null()) {
        return false;
    }
    is_valid_item_list(kind, items)
}

fn is_valid_item_list(kind: &str, items: Option<&Value>) -> bool {
    let Some(items) = items.and_then(Value::as_array) else {
        return false;
    };
    if items.is_empty() || items.len() > MAX_ITEMS {
        return false;
    }
    let mut paths: Vec<String> = Vec::with_capacity(items.len());
    let mut has_file = false;
    let mut has_directory = false;
    for item in items {
        let Some(object) = item.as_object() else {
            return false;
        };
        let Some(item_kind) = object.get("kind").and_then(Value::as_str) else {
            return false;
        };
        let Some(path) = object.get("path").and_then(Value::as_str) else {
            return false;
        };
        match item_kind {
            "file" => has_file = true,
            "directory" => has_directory = true,
            _ => return false,
        }
        if !is_fully_qualified(path) {
            return false;
        }
        paths.push(path.to_lowercase());
    }
    paths.sort();
    paths.dedup();
    if paths.len() != items.len() {
        return false;
    }
    match kind {
        "directories" => has_directory && !has_file,
        "files" => has_file,
        _ => false,
    }
}

fn is_fully_qualified(path: &str) -> bool {
    if path.is_empty() || path.len() > 1024 || path.chars().any(char::is_control) {
        return false;
    }
    if path.starts_with("\\\\?\\") || path.starts_with("\\\\.\\") {
        return false;
    }
    let bytes = path.as_bytes();
    let absolute = path.starts_with("\\\\")
        || (bytes.len() >= 3 && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'));
    if !absolute {
        return false;
    }
    let rest = if bytes.len() >= 2 && bytes[1] == b':' {
        &path[2..]
    } else {
        path
    };
    !rest.contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request_id() -> String {
        "3f2a1b4c-5d6e-4f70-8a91-b2c3d4e5f607".into()
    }

    fn text_request(text: &str, source: &str) -> Value {
        json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": source,
            "context": {
                "type": "text",
                "source": if source == "launcher" { "launcher-input" } else { "selected-text" },
                "text": text,
                "capturedAtUnixMs": 1_700_000_000_000u64,
            },
        })
    }

    #[test]
    fn accepts_panel_text_request() {
        assert!(is_valid_feature_invoke(&text_request(
            "hello",
            "super-panel"
        )));
    }

    #[test]
    fn accepts_launcher_request_without_context() {
        let payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "launcher",
        });
        assert!(is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_text_over_panel_limit() {
        let text = "字".repeat(MAX_TEXT_SUPER_PANEL + 1);
        assert!(!is_valid_feature_invoke(&text_request(
            &text,
            "super-panel"
        )));
    }

    #[test]
    fn rejects_text_with_items() {
        let mut payload = text_request("hello", "super-panel");
        payload["context"]["items"] = json!([{ "kind": "file", "path": "C:\\a.txt" }]);
        assert!(!is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_files_context_from_launcher() {
        let payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "launcher",
            "context": {
                "type": "files",
                "source": "selected-items",
                "capturedAtUnixMs": 1u64,
                "items": [{ "kind": "file", "path": "C:\\a.txt" }],
            },
        });
        assert!(!is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_empty_item_list() {
        let payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "super-panel",
            "context": {
                "type": "files",
                "source": "selected-items",
                "capturedAtUnixMs": 1u64,
                "items": [],
            },
        });
        assert!(!is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_duplicate_paths_ignoring_case() {
        let payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "super-panel",
            "context": {
                "type": "files",
                "source": "selected-items",
                "capturedAtUnixMs": 1u64,
                "items": [
                    { "kind": "file", "path": "C:\\A.txt" },
                    { "kind": "file", "path": "C:\\a.txt" },
                ],
            },
        });
        assert!(!is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_relative_path() {
        let payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "super-panel",
            "context": {
                "type": "files",
                "source": "selected-items",
                "capturedAtUnixMs": 1u64,
                "items": [{ "kind": "file", "path": "a.txt" }],
            },
        });
        assert!(!is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_directories_holding_files() {
        let payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "super-panel",
            "context": {
                "type": "directories",
                "source": "selected-items",
                "capturedAtUnixMs": 1u64,
                "items": [{ "kind": "file", "path": "C:\\a.txt" }],
            },
        });
        assert!(!is_valid_feature_invoke(&payload));
    }

    #[test]
    fn rejects_bad_identifiers() {
        for (plugin, feature) in [
            ("Webtools.Translation", "translate-text"),
            ("ab", "translate-text"),
            ("__proto__", "translate-text"),
            ("webtools.translation", "Translate"),
            ("webtools.translation", ""),
        ] {
            let payload = json!({
                "requestId": request_id(),
                "ref": { "pluginId": plugin, "featureId": feature },
                "source": "launcher",
            });
            assert!(
                !is_valid_feature_invoke(&payload),
                "accepted {plugin}:{feature}"
            );
        }
    }

    #[test]
    fn rejects_quick_result_outside_panel_text() {
        let mut payload = json!({
            "requestId": request_id(),
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "source": "super-panel",
            "quickResult": { "version": 1 },
            "context": {
                "type": "text",
                "source": "launcher-input",
                "text": "hello",
                "capturedAtUnixMs": 1u64,
            },
        });
        assert!(!is_valid_feature_invoke(&payload));
        payload["context"]["source"] = json!("selected-text");
        assert!(is_valid_feature_invoke(&payload));
    }

    #[test]
    fn parses_completed_result() {
        let value =
            json!({ "requestId": request_id(), "status": "completed", "reasonCode": "completed" });
        let outcome = parse_invoke_result(&value, &request_id()).unwrap();
        assert!(outcome.completed());
        assert!(!outcome.dismiss_panel);
    }

    #[test]
    fn parses_dismiss_flag_for_completed_only() {
        let ok = json!({
            "requestId": request_id(),
            "status": "completed",
            "reasonCode": "completed",
            "dismissPanel": true,
        });
        assert!(
            parse_invoke_result(&ok, &request_id())
                .unwrap()
                .dismiss_panel
        );

        let wrong = json!({
            "requestId": request_id(),
            "status": "accepted",
            "reasonCode": "accepted",
            "dismissPanel": true,
        });
        assert!(parse_invoke_result(&wrong, &request_id()).is_none());
    }

    #[test]
    fn rejects_unknown_keys_and_ids() {
        let extra = json!({
            "requestId": request_id(),
            "status": "accepted",
            "reasonCode": "accepted",
            "detail": "x",
        });
        assert!(parse_invoke_result(&extra, &request_id()).is_none());

        let other =
            json!({ "requestId": request_id(), "status": "accepted", "reasonCode": "accepted" });
        assert!(parse_invoke_result(&other, "00000000-0000-4000-8000-000000000000").is_none());
    }

    #[test]
    fn rejects_unknown_reason_for_status() {
        let value =
            json!({ "requestId": request_id(), "status": "rejected", "reasonCode": "nope" });
        assert!(parse_invoke_result(&value, &request_id()).is_none());
        let value =
            json!({ "requestId": request_id(), "status": "rejected", "reasonCode": "queue-full" });
        assert!(parse_invoke_result(&value, &request_id()).is_some());
    }
}
