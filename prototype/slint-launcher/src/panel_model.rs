//! What the panel may show, derived from the plugin projection.
//!
//! The panel is a renderer here: it never decides on its own which plugin or
//! feature is reachable. Every row comes from the manager's projection, whose
//! acceptedContexts were declared by the plugin itself. A plugin that does not
//! open itself to explorer selections simply does not appear for a folder.

use webtools_ipc::projection::{
    matches_selection, visible_features, ContextKind, FeatureRow, Selection,
};

use crate::shell_context::ShellContext;

/// Builds the invocation context for the current surface, or none for the board.
pub fn selection_for(context: &Option<ShellContext>, text: &str) -> Option<Selection> {
    if let Some(context) = context {
        if matches!(
            context.kind,
            crate::shell_context::ShellKind::Explorer
                | crate::shell_context::ShellKind::DesktopEmpty
                | crate::shell_context::ShellKind::TooManyItems
        ) {
            // These states carry no invokable selection: Explorer's current
            // directory is display-only; empty/overflow contexts have no items.
            return None;
        }
        let items: Vec<(ContextKind, String)> = context
            .items
            .iter()
            .map(|path| {
                let kind = if std::fs::metadata(path)
                    .map(|meta| meta.is_dir())
                    .unwrap_or(false)
                {
                    ContextKind::Directories
                } else {
                    ContextKind::Files
                };
                (kind, path.clone())
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        return Some(Selection::items(items));
    }
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(Selection::text(text))
    }
}

/// Features the plugin opened up for this surface, in projection order.
pub fn visible<'a>(rows: &'a [FeatureRow], selection: Option<&Selection>) -> Vec<&'a FeatureRow> {
    let visible = visible_features(rows, selection);
    match selection {
        // In a context surface, `none` means "show on the no-context board";
        // it must not make an otherwise incompatible feature appear beside a
        // selected text/file/folder. WPF applies MatchesSelection in this path.
        Some(selection) => visible
            .into_iter()
            .filter(|row| matches_selection(row, selection))
            .collect(),
        None => visible,
    }
}

/// Pinned features resolved against the projection, in saved order.
pub fn pinned_rows<'a>(
    rows: &'a [FeatureRow],
    pins: &[(String, String, String)],
) -> Vec<&'a FeatureRow> {
    pins.iter()
        .filter_map(|(plugin, feature, _)| {
            rows.iter()
                .find(|row| row.plugin_id == *plugin && row.feature_id == *feature)
        })
        .collect()
}

/// One page of the 3x3 board.
pub fn board_slice<'a>(
    rows: &'a [FeatureRow],
    pins: &[(String, String, String)],
    page: usize,
) -> Vec<&'a FeatureRow> {
    pinned_rows(rows, pins)
        .into_iter()
        .skip(page * 9)
        .take(9)
        .collect()
}

pub fn page_count(pins: &[(String, String, String)]) -> usize {
    ((pins.len() + 8) / 9).max(1)
}

/// Features a plugin exposed but the user has not pinned yet.
pub fn chooser_rows<'a>(
    rows: &'a [FeatureRow],
    pins: &[(String, String, String)],
) -> Vec<&'a FeatureRow> {
    rows.iter()
        .filter(|row| row.feature_kind == "runtime-v3" || row.feature_kind == "built-in")
        .filter(|row| {
            !pins
                .iter()
                .any(|(plugin, feature, _)| *plugin == row.plugin_id && *feature == row.feature_id)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use webtools_ipc::projection::parse_projection;

    fn rows() -> Vec<FeatureRow> {
        let snapshot = json!({
            "projectionVersion": 2,
            "hostSessionId": "0f1e2d3c-4b5a-4978-8a9b-0c1d2e3f4a5b",
            "revision": 1,
            "generatedAtUnixMs": 1,
            "features": [
                {
                    "ref": { "pluginId": "webtools.translation", "featureId": "translate-text" },
                    "pluginName": "翻译", "title": "翻译", "commands": [],
                    "acceptedContexts": [{ "type": "text", "sources": ["selected-text"] }],
                    "iconToken": "translate", "featureKind": "runtime-v3"
                },
                {
                    "ref": { "pluginId": "webtools.files", "featureId": "open-folder" },
                    "pluginName": "文件", "title": "打开文件夹", "commands": [],
                    "acceptedContexts": [{ "type": "directories", "sources": ["explorer-selection"] }],
                    "iconToken": "document", "featureKind": "runtime-v3"
                },
                {
                    "ref": { "pluginId": "webtools.core", "featureId": "open-manager" },
                    "pluginName": "核心", "title": "打开管理端", "commands": [],
                    "acceptedContexts": [{ "type": "none", "sources": [] }],
                    "iconToken": "plugin", "featureKind": "built-in"
                }
            ]
        });
        parse_projection(&snapshot).unwrap().features
    }

    fn titles(rows: &[&FeatureRow]) -> Vec<String> {
        rows.iter().map(|row| row.title.clone()).collect()
    }

    #[test]
    fn text_selection_hides_folder_and_general_features() {
        let rows = rows();
        let selection = selection_for(&None, "hello");
        assert_eq!(titles(&visible(&rows, selection.as_ref())), vec!["翻译"]);
    }

    #[test]
    fn board_shows_only_general_features() {
        let rows = rows();
        assert_eq!(titles(&visible(&rows, None)), vec!["打开管理端"]);
    }

    #[test]
    fn blank_text_has_no_selection() {
        assert!(selection_for(&None, "   ").is_none());
    }

    #[test]
    fn pins_keep_their_order_and_render_nine_per_page() {
        let rows = rows();
        let pins: Vec<(String, String, String)> = vec![
            (
                "webtools.translation".into(),
                "translate-text".into(),
                "翻译".into(),
            ),
            (
                "webtools.core".into(),
                "open-manager".into(),
                "打开管理端".into(),
            ),
        ];
        assert_eq!(
            titles(&pinned_rows(&rows, &pins)),
            vec!["翻译", "打开管理端"]
        );
        assert_eq!(board_slice(&rows, &pins, 0).len(), 2);
        assert_eq!(page_count(&pins), 1);
        let mut many = pins.clone();
        for index in 0..9 {
            many.push((
                "webtools.translation".into(),
                format!("feature-{index}"),
                "x".into(),
            ));
        }
        assert_eq!(page_count(&many), 2);
    }

    #[test]
    fn chooser_lists_exactly_the_unpinned_features() {
        let rows = rows();
        let pins: Vec<(String, String, String)> = vec![(
            "webtools.translation".into(),
            "translate-text".into(),
            "翻译".into(),
        )];
        assert_eq!(
            titles(&chooser_rows(&rows, &pins)),
            vec!["打开文件夹", "打开管理端"]
        );
    }
}
