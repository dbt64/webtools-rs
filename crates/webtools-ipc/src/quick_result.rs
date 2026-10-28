//! Quick result payloads (super-panel-quick-result-v1).
//!
//! Mirrors NativeFeatureQuickResultProtocol and src/shared/feature-quick-result-contracts.ts.
//! The manager only sends these when the host advertises the capability, so the
//! capability string lives here next to the parser.

use serde_json::Value;

pub const CAPABILITY: &str = "super-panel-quick-result-v1";
pub const MAX_PAYLOAD_BYTES: usize = 65_536;
pub const MAX_TEXT_SCALARS: usize = 8_192;
pub const MAX_TEXT_BYTES: usize = 32_768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickResult {
    pub request_id: String,
    pub plugin_id: String,
    pub feature_id: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AIConfirmation {
    pub request_id: String,
    pub plugin_id: String,
    pub feature_id: String,
    pub consent_id: String,
    pub plugin_name: String,
    pub provider_name: String,
    pub preview: String,
}

/// ECMAScript trim set, which differs from Rust's char::is_whitespace for NEL/BOM.
fn is_ecma_blank(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n'
            | ' '
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    ) || ('\u{2000}'..='\u{200a}').contains(&ch)
}

pub fn normalize_text(input: &str) -> Option<String> {
    if input.len() > MAX_PAYLOAD_BYTES {
        return None;
    }
    let text = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut scalars = 0;
    for ch in text.chars() {
        scalars += 1;
        if scalars > MAX_TEXT_SCALARS {
            return None;
        }
        let value = ch as u32;
        if (0xd800..=0xdfff).contains(&value) {
            return None;
        }
        if value < 32 && value != 9 && value != 10 {
            return None;
        }
    }
    if text.chars().all(is_ecma_blank) {
        return None;
    }
    if text.len() > MAX_TEXT_BYTES {
        return None;
    }
    Some(text)
}

fn exact_fields<'a>(
    value: &'a Value,
    allowed: &[&str],
) -> Option<&'a serde_json::Map<String, Value>> {
    let object = value.as_object()?;
    if object.len() != allowed.len() || object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return None;
    }
    Some(object)
}

fn parse_ref(value: &Value) -> Option<(String, String)> {
    let object = exact_fields(value, &["pluginId", "featureId"])?;
    let plugin = object.get("pluginId")?.as_str()?.to_string();
    let feature = object.get("featureId")?.as_str()?.to_string();
    if !crate::contract::is_valid_plugin_id(&plugin)
        || !crate::contract::is_valid_feature_id(&feature)
    {
        return None;
    }
    Some((plugin, feature))
}

/// Parses the feature-result-v1 payload; anything malformed is dropped rather than
/// rendering a half-valid card.
pub fn parse_quick_result(value: &Value) -> Option<QuickResult> {
    if serde_json::to_vec(value)
        .map(|bytes| bytes.len() > MAX_PAYLOAD_BYTES)
        .unwrap_or(true)
    {
        return None;
    }
    let object = exact_fields(value, &["resultVersion", "requestId", "ref", "text"])?;
    if object.get("resultVersion")?.as_u64()? != 1 {
        return None;
    }
    let request_id = object.get("requestId")?.as_str()?.to_string();
    if !crate::contract::is_request_id(&request_id) {
        return None;
    }
    let (plugin_id, feature_id) = parse_ref(object.get("ref")?)?;
    let text = normalize_text(object.get("text")?.as_str()?)?;
    Some(QuickResult {
        request_id,
        plugin_id,
        feature_id,
        text,
    })
}

/// Parses a feature-result-control payload (requestId + ref, nothing else).
pub fn parse_control(value: &Value) -> Option<(String, String, String)> {
    let object = exact_fields(value, &["requestId", "ref"])?;
    let request_id = object.get("requestId")?.as_str()?.to_string();
    if !crate::contract::is_request_id(&request_id) {
        return None;
    }
    let (plugin_id, feature_id) = parse_ref(object.get("ref")?)?;
    Some((request_id, plugin_id, feature_id))
}

/// Parses the confirmation prompt sent by the Manager before a shared-AI call.
pub fn parse_ai_confirmation(value: &Value) -> Option<AIConfirmation> {
    if serde_json::to_vec(value)
        .map(|bytes| bytes.len() > MAX_PAYLOAD_BYTES)
        .unwrap_or(true)
    {
        return None;
    }
    let object = exact_fields(
        value,
        &[
            "requestId",
            "ref",
            "consentId",
            "pluginName",
            "providerName",
            "preview",
        ],
    )?;
    let request_id = object.get("requestId")?.as_str()?.to_string();
    let consent_id = object.get("consentId")?.as_str()?.to_string();
    if !crate::contract::is_request_id(&request_id) || !crate::contract::is_request_id(&consent_id)
    {
        return None;
    }
    let (plugin_id, feature_id) = parse_ref(object.get("ref")?)?;
    let plugin_name = object.get("pluginName")?.as_str()?.to_string();
    let provider_name = object.get("providerName")?.as_str()?.to_string();
    let preview = object.get("preview")?.as_str()?.to_string();
    let utf16_len = |text: &str| text.encode_utf16().count();
    if !(1..=160).contains(&utf16_len(&plugin_name))
        || !(1..=160).contains(&utf16_len(&provider_name))
        || utf16_len(&preview) > 8192
    {
        return None;
    }
    Some(AIConfirmation {
        request_id,
        plugin_id,
        feature_id,
        consent_id,
        plugin_name,
        provider_name,
        preview,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload(text: &str) -> Value {
        json!({
            "resultVersion": 1,
            "requestId": "3f2a1b4c-5d6e-4f70-8a91-b2c3d4e5f607",
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
            "text": text,
        })
    }

    #[test]
    fn accepts_a_normal_result() {
        let parsed = parse_quick_result(&payload("你好")).unwrap();
        assert_eq!(parsed.plugin_id, "webtools.translation");
        assert_eq!(parsed.text, "你好");
    }

    #[test]
    fn normalizes_crlf() {
        assert_eq!(normalize_text("a\r\nb\rc").unwrap(), "a\nb\nc");
    }

    #[test]
    fn rejects_extra_or_missing_fields() {
        let mut extra = payload("x");
        extra["detail"] = json!("y");
        assert!(parse_quick_result(&extra).is_none());

        let mut missing = payload("x");
        missing.as_object_mut().unwrap().remove("ref");
        assert!(parse_quick_result(&missing).is_none());
    }

    #[test]
    fn rejects_blank_control_and_oversized_text() {
        assert!(parse_quick_result(&payload("   \n\t ")).is_none());
        assert!(parse_quick_result(&payload("bad\u{7}char")).is_none());
        assert!(parse_quick_result(&payload(&"字".repeat(MAX_TEXT_SCALARS + 1))).is_none());
    }

    #[test]
    fn rejects_wrong_version_and_bad_request_id() {
        let mut wrong = payload("x");
        wrong["resultVersion"] = json!(2);
        assert!(parse_quick_result(&wrong).is_none());

        let mut bad_id = payload("x");
        bad_id["requestId"] = json!("not-a-guid");
        assert!(parse_quick_result(&bad_id).is_none());
    }

    #[test]
    fn parses_control_payload() {
        let control = json!({
            "requestId": "3f2a1b4c-5d6e-4f70-8a91-b2c3d4e5f607",
            "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
        });
        assert!(parse_control(&control).is_some());
        let mut extra = control.clone();
        extra["text"] = json!("x");
        assert!(parse_control(&extra).is_none());
    }
}
