//! Feature Projection v2: parsing plus the Super Panel visibility rules.
//!
//! The panel never decides on its own what it may show. Each plugin feature
//! declares in its manifest which contexts it accepts; the manager publishes
//! that as acceptedContexts, and the rules below are a direct port of the host
//! matcher (SuperPanelFeatureMatcher) so both sides filter identically.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextKind {
    Text,
    Files,
    Directories,
    None,
}

impl ContextKind {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "text" => Some(Self::Text),
            "files" => Some(Self::Files),
            "directories" => Some(Self::Directories),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptedContext {
    #[serde(rename = "type")]
    pub kind: ContextKind,
    pub sources: Vec<String>,
    pub extensions: Option<Vec<String>>,
    pub max_items: Option<usize>,
    pub text_match: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeatureRow {
    pub plugin_id: String,
    pub feature_id: String,
    pub plugin_name: String,
    pub title: String,
    pub commands: Vec<String>,
    pub accepted_contexts: Vec<AcceptedContext>,
    pub icon_token: String,
    pub feature_kind: String,
    pub presentation: Option<String>,
    pub icon_data_url: Option<String>,
}

impl FeatureRow {
    /// True when the feature can run without any selection (the board case).
    pub fn is_general(&self) -> bool {
        self.accepted_contexts.is_empty()
            || self
                .accepted_contexts
                .iter()
                .any(|rule| rule.kind == ContextKind::None)
    }

    pub fn accepts_any_selection(&self) -> bool {
        self.accepted_contexts
            .iter()
            .any(|rule| rule.kind != ContextKind::None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection {
    pub host_session_id: String,
    pub revision: i64,
    pub features: Vec<FeatureRow>,
}

/// One captured selection, shaped exactly like the invocation context payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub kind: ContextKind,
    pub source: String,
    pub text: Option<String>,
    pub items: Vec<(ContextKind, String)>,
}

impl Selection {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            kind: ContextKind::Text,
            source: "selected-text".into(),
            text: Some(text.into()),
            items: Vec::new(),
        }
    }

    pub fn items(items: Vec<(ContextKind, String)>) -> Self {
        let has_file = items.iter().any(|(kind, _)| *kind == ContextKind::Files);
        Self {
            kind: if has_file {
                ContextKind::Files
            } else {
                ContextKind::Directories
            },
            source: "explorer-selection".into(),
            text: None,
            items,
        }
    }

    /// Parses a context payload as published by the manager.
    pub fn from_payload(value: &Value) -> Option<Self> {
        let kind = ContextKind::parse(value.get("type")?.as_str()?)?;
        let source = value.get("source")?.as_str()?.to_string();
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_string);
        let mut items = Vec::new();
        if let Some(list) = value.get("items").and_then(Value::as_array) {
            for item in list {
                let item_kind = ContextKind::parse(item.get("kind")?.as_str()?)
                    .filter(|k| *k != ContextKind::Text)?;
                items.push((item_kind, item.get("path")?.as_str()?.to_string()));
            }
        }
        Some(Self {
            kind,
            source,
            text,
            items,
        })
    }
}

/// Parses a whole projection snapshot. Any malformed row rejects the snapshot,
/// matching the host, which never applies a partially valid projection.
pub fn parse_projection(value: &Value) -> Option<Projection> {
    let version = value.get("projectionVersion")?.as_u64()?;
    if version != 2 {
        return None;
    }
    let host_session_id = value.get("hostSessionId")?.as_str()?.to_string();
    let revision = value.get("revision")?.as_i64()?;
    let rows = value.get("features")?.as_array()?;
    let mut features = Vec::with_capacity(rows.len());
    for row in rows {
        features.push(parse_row(row)?);
    }
    Some(Projection {
        host_session_id,
        revision,
        features,
    })
}

/// Serializes a Manager projection using the same v2 field names accepted by
/// `parse_projection`, for the rebuildable local presentation cache.
pub fn projection_cache_value(projection: &Projection) -> Value {
    serde_json::json!({
        "projectionVersion": 2,
        "hostSessionId": projection.host_session_id,
        "revision": projection.revision,
        "features": projection.features.iter().map(|feature| serde_json::json!({
            "ref": {
                "pluginId": feature.plugin_id,
                "featureId": feature.feature_id,
            },
            "pluginName": feature.plugin_name,
            "title": feature.title,
            "commands": feature.commands,
            "acceptedContexts": feature.accepted_contexts,
            "iconToken": feature.icon_token,
            "featureKind": feature.feature_kind,
            "presentation": feature.presentation,
            "iconDataUrl": feature.icon_data_url,
        })).collect::<Vec<_>>(),
    })
}

fn parse_row(value: &Value) -> Option<FeatureRow> {
    let reference = value.get("ref")?;
    let plugin_id = reference.get("pluginId")?.as_str()?.to_string();
    let feature_id = reference.get("featureId")?.as_str()?.to_string();
    if !crate::contract::is_valid_plugin_id(&plugin_id)
        || !crate::contract::is_valid_feature_id(&feature_id)
    {
        return None;
    }
    let mut contexts = Vec::new();
    for rule in value.get("acceptedContexts")?.as_array()? {
        contexts.push(parse_context(rule)?);
    }
    Some(FeatureRow {
        plugin_id,
        feature_id,
        plugin_name: value.get("pluginName")?.as_str()?.to_string(),
        title: value.get("title")?.as_str()?.to_string(),
        commands: string_list(value.get("commands"))?,
        accepted_contexts: contexts,
        icon_token: value.get("iconToken")?.as_str()?.to_string(),
        feature_kind: value.get("featureKind")?.as_str()?.to_string(),
        presentation: value
            .get("presentation")
            .and_then(Value::as_str)
            .map(str::to_string),
        icon_data_url: value
            .get("iconDataUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn parse_context(value: &Value) -> Option<AcceptedContext> {
    let kind = ContextKind::parse(value.get("type")?.as_str()?)?;
    let sources = string_list(value.get("sources"))?;
    let extensions = match value.get("extensions") {
        Some(Value::Null) | None => None,
        Some(list) => Some(string_list(Some(list))?),
    };
    let max_items = match value.get("maxItems") {
        Some(Value::Null) | None => None,
        Some(raw) => Some(raw.as_u64()? as usize),
    };
    let text_match = value
        .get("textMatch")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(AcceptedContext {
        kind,
        sources,
        extensions,
        max_items,
        text_match,
    })
}

fn string_list(value: Option<&Value>) -> Option<Vec<String>> {
    let list = value?.as_array()?;
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        out.push(entry.as_str()?.to_string());
    }
    Some(out)
}

/// The panel's action list: features the plugin opened up for this selection.
pub fn matches_selection(row: &FeatureRow, selection: &Selection) -> bool {
    if selection.kind == ContextKind::Text {
        let text = selection.text.as_deref().unwrap_or("");
        return row.accepted_contexts.iter().any(|rule| {
            rule.kind == ContextKind::Text
                && rule
                    .sources
                    .iter()
                    .any(|source| *source == selection.source)
                && match rule.text_match.as_deref() {
                    None => true,
                    Some("http-url") => selection.source == "selected-text" && is_http_url(text),
                    Some(_) => false,
                }
        });
    }

    if selection.items.is_empty() {
        return false;
    }
    let mut kinds: Vec<ContextKind> = selection.items.iter().map(|(kind, _)| *kind).collect();
    kinds.dedup();
    kinds.iter().all(|kind| {
        row.accepted_contexts.iter().any(|rule| {
            let expected = if *kind == ContextKind::Files {
                ContextKind::Files
            } else {
                ContextKind::Directories
            };
            rule.kind == expected
                && rule
                    .sources
                    .iter()
                    .any(|source| *source == selection.source)
                && rule
                    .max_items
                    .is_none_or(|limit| selection.items.len() <= limit)
                && (*kind != ContextKind::Files || extensions_accept(rule, &selection.items))
        })
    })
}

/// Context-free features run anywhere; otherwise the selection decides.
pub fn matches(row: &FeatureRow, selection: Option<&Selection>) -> bool {
    let Some(selection) = selection else {
        return row.is_general();
    };
    row.is_general() || matches_selection(row, selection)
}

/// Keeps only the features a plugin actually exposed for this surface.
pub fn visible_features<'a>(
    rows: &'a [FeatureRow],
    selection: Option<&Selection>,
) -> Vec<&'a FeatureRow> {
    rows.iter()
        .filter(|row| row.feature_kind == "runtime-v3" || row.feature_kind == "built-in")
        .filter(|row| matches(row, selection))
        .collect()
}

fn extensions_accept(rule: &AcceptedContext, items: &[(ContextKind, String)]) -> bool {
    let Some(extensions) = rule.extensions.as_deref() else {
        return true;
    };
    if extensions.is_empty() {
        return true;
    }
    items
        .iter()
        .filter(|(kind, _)| *kind == ContextKind::Files)
        .all(|(_, path)| {
            let extension = std::path::Path::new(path)
                .extension()
                .map(|value| value.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            extensions
                .iter()
                .any(|allowed| allowed.trim_start_matches('.').to_lowercase() == extension)
        })
}

pub fn is_http_url(text: &str) -> bool {
    let value = text.trim();
    let lower = value.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return false;
    }
    let rest = value.splitn(2, "//").nth(1).unwrap_or("");
    !rest.is_empty() && !rest.starts_with('/') && rest.chars().any(|ch| ch == '.' || ch == ':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(feature: &str, contexts: Value) -> FeatureRow {
        parse_row(&json!({
            "ref": { "pluginId": "webtools.translation", "featureId": feature },
            "pluginName": "翻译",
            "title": feature,
            "commands": [],
            "acceptedContexts": contexts,
            "iconToken": "translate",
            "featureKind": "runtime-v3",
        }))
        .unwrap()
    }

    #[test]
    fn features_without_contexts_are_general() {
        let general = row("translate-text", json!([]));
        assert!(general.is_general());
        assert!(matches(&general, None));
        assert!(matches(&general, Some(&Selection::text("hello"))));
    }

    #[test]
    fn none_rule_is_general() {
        let general = row("open-manager", json!([{ "type": "none", "sources": [] }]));
        assert!(general.is_general());
    }

    #[test]
    fn text_feature_needs_selected_text_source() {
        let text_only = row(
            "translate-text",
            json!([{ "type": "text", "sources": ["selected-text"] }]),
        );
        assert!(matches_selection(&text_only, &Selection::text("hi")));
        assert!(!matches_selection(
            &text_only,
            &Selection::items(vec![(ContextKind::Files, "C:\\a.txt".into())])
        ));
        assert!(!matches(&text_only, None));
    }

    #[test]
    fn folder_feature_is_hidden_for_text_selection() {
        let folder = row(
            "open-folder",
            json!([{ "type": "directories", "sources": ["explorer-selection"] }]),
        );
        assert!(!matches_selection(&folder, &Selection::text("hi")));
        assert!(matches_selection(
            &folder,
            &Selection::items(vec![(ContextKind::Directories, "C:\\tmp".into())])
        ));
    }

    #[test]
    fn files_feature_respects_max_items_and_extensions() {
        let rules = json!([{
            "type": "files",
            "sources": ["explorer-selection"],
            "maxItems": 2,
            "extensions": [".txt"],
        }]);
        let files = row("summarize", rules);
        let two = Selection::items(vec![
            (ContextKind::Files, "C:\\a.txt".into()),
            (ContextKind::Files, "C:\\b.TXT".into()),
        ]);
        assert!(matches_selection(&files, &two));
        let three = Selection::items(vec![
            (ContextKind::Files, "C:\\a.txt".into()),
            (ContextKind::Files, "C:\\b.txt".into()),
            (ContextKind::Files, "C:\\c.txt".into()),
        ]);
        assert!(!matches_selection(&files, &three));
        let wrong_ext = Selection::items(vec![(ContextKind::Files, "C:\\a.png".into())]);
        assert!(!matches_selection(&files, &wrong_ext));
    }

    #[test]
    fn mixed_selection_requires_every_kind_to_match() {
        let dirs_only = row(
            "open-folder",
            json!([{ "type": "directories", "sources": ["explorer-selection"] }]),
        );
        let mixed = Selection::items(vec![
            (ContextKind::Directories, "C:\\tmp".into()),
            (ContextKind::Files, "C:\\tmp\\a.txt".into()),
        ]);
        assert!(!matches_selection(&dirs_only, &mixed));
    }

    #[test]
    fn http_url_rule_only_matches_urls() {
        let link = row(
            "open-link",
            json!([{ "type": "text", "sources": ["selected-text"], "textMatch": "http-url" }]),
        );
        assert!(matches_selection(
            &link,
            &Selection::text("https://example.com/a")
        ));
        assert!(!matches_selection(&link, &Selection::text("just words")));
    }

    #[test]
    fn parses_projection_and_rejects_bad_rows() {
        let snapshot = json!({
            "projectionVersion": 2,
            "hostSessionId": "0f1e2d3c-4b5a-4978-8a9b-0c1d2e3f4a5b",
            "revision": 7,
            "generatedAtUnixMs": 1,
            "features": [{
                "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
                "pluginName": "翻译",
                "title": "翻译",
                "commands": ["翻译"],
                "acceptedContexts": [{ "type": "text", "sources": ["selected-text"] }],
                "iconToken": "translate",
                "featureKind": "runtime-v3",
            }],
        });
        let projection = parse_projection(&snapshot).unwrap();
        assert_eq!(projection.revision, 7);
        assert_eq!(projection.features.len(), 1);
        assert_eq!(projection.features[0].feature_id, "translate-text");

        let mut broken = snapshot.clone();
        broken["features"][0]["ref"]["featureId"] = json!("Translate");
        assert!(parse_projection(&broken).is_none());
    }

    #[test]
    fn visible_features_skips_compatibility_entries() {
        let mut compatibility = row("legacy", json!([]));
        compatibility.feature_kind = "declarative-compatibility".into();
        let rows = vec![compatibility, row("translate-text", json!([]))];
        let visible = visible_features(&rows, None);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].feature_id, "translate-text");
    }
}
