//! WPF Launcher shortcut projection: saved websites and six projected features.
use crate::visual::Target;
use serde_json::Value;
use webtools_ipc::projection::FeatureRow;

#[derive(Clone)]
pub struct Shortcut {
    pub id: String,
    pub title: String,
    pub fallback: String,
    pub icon_token: String,
    pub icon_reference: Option<String>,
    pub target: Target,
}

impl Shortcut {
    pub fn visual(&self, icon: Option<slint::Image>) -> crate::VisualRow {
        crate::VisualRow {
            title: self.title.as_str().into(),
            glyph: self.fallback.as_str().into(),
            icon_token: self.icon_token.as_str().into(),
            icon_index: -1,
            has_icon: icon.is_some(),
            icon: icon.unwrap_or_default(),
            ..Default::default()
        }
    }
}

pub fn project(websites: &Value, features: &[FeatureRow]) -> (Vec<Shortcut>, Vec<Shortcut>) {
    let sites = websites
        .as_array()
        .into_iter()
        .flatten()
        .map(|site| {
            let url = site["url"].as_str().unwrap_or_default();
            Shortcut {
                id: site["id"].as_str().unwrap_or_default().into(),
                title: site["name"].as_str().unwrap_or_default().into(),
                fallback: website_initial(url),
                icon_token: String::new(),
                icon_reference: site["favicon"].as_str().map(str::to_string),
                target: Target::Website(url.into()),
            }
        })
        .collect();
    let mut features = features
        .iter()
        .filter(|feature| feature.feature_kind != "declarative-compatibility")
        .collect::<Vec<_>>();
    features.sort_by_key(|feature| {
        !(feature.plugin_id == "webtools.translation" && feature.feature_id == "translate-text")
    });
    let plugins = features
        .into_iter()
        .take(6)
        .map(|feature| Shortcut {
            id: format!("{}/{}", feature.plugin_id, feature.feature_id),
            title: feature.title.clone(),
            fallback: "⌘".into(),
            icon_token: feature.icon_token.clone(),
            icon_reference: feature.icon_data_url.clone(),
            target: Target::Feature {
                plugin: feature.plugin_id.clone(),
                feature: feature.feature_id.clone(),
                launcher_text: None,
            },
        })
        .collect();
    (sites, plugins)
}

pub fn website_initial(url: &str) -> String {
    let host = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    let host = if host
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("www."))
    {
        &host[4..]
    } else {
        host
    };
    host.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".into())
}

pub fn website_viewport(count: usize, expanded: bool) -> f32 {
    if expanded {
        count.div_ceil(6).clamp(1, 4) as f32 * 60.0
    } else {
        60.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use webtools_ipc::projection::{AcceptedContext, ContextKind};

    fn feature(id: &str, plugin: &str) -> FeatureRow {
        FeatureRow {
            plugin_id: plugin.into(),
            feature_id: id.into(),
            plugin_name: "插件".into(),
            title: id.into(),
            commands: Vec::new(),
            accepted_contexts: vec![AcceptedContext {
                kind: ContextKind::Text,
                sources: vec!["launcher-input".into()],
                extensions: None,
                max_items: None,
                text_match: None,
            }],
            icon_token: "translate".into(),
            feature_kind: "runtime-v3".into(),
            presentation: Some("window".into()),
            icon_data_url: Some("data:image/png;base64,projected-icon".into()),
        }
    }

    #[test]
    fn saved_websites_keep_user_order_and_open_the_original_urls() {
        let (sites, plugins) = project(
            &json!([
                {"id":"b","name":"乙网站","url":"https://www.b.example/path"},
                {"id":"a","name":"甲网站","url":"https://a.example/"}
            ]),
            &[],
        );
        assert_eq!(
            sites.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert!(
            matches!(&sites[0].target, Target::Website(url) if url == "https://www.b.example/path")
        );
        assert_eq!(sites[0].fallback, "B");
        assert!(
            plugins.is_empty(),
            "Do not synthesize a separate translation shortcut"
        );
    }

    #[test]
    fn projected_translation_is_first_without_removing_real_translation_features() {
        let mut features = (0..8)
            .map(|i| feature(&format!("open-{i}"), "test.plugin"))
            .collect::<Vec<_>>();
        features.push(feature("translate-text", "webtools.translation"));
        let (_, plugins) = project(&json!([]), &features);
        assert_eq!(plugins.len(), 6);
        assert_eq!(plugins[0].title, "translate-text");
        assert_eq!(plugins[1].title, "open-0");
        assert!(plugins[0].icon_reference.is_some());
        assert!(
            matches!(&plugins[0].target, Target::Feature { plugin, feature, launcher_text }
            if plugin == "webtools.translation" && feature == "translate-text" && launcher_text.is_none())
        );
    }

    #[test]
    fn projection_replacement_never_keeps_removed_feature_targets() {
        let (_, first) = project(&json!([]), &[feature("old", "test.plugin")]);
        let (_, next) = project(&json!([]), &[feature("new", "test.plugin")]);
        assert_eq!(first.len(), 1);
        assert_eq!(next.len(), 1);
        assert!(matches!(&next[0].target, Target::Feature { feature, .. } if feature == "new"));
        assert!(project(&json!([]), &[]).1.is_empty());
    }

    #[test]
    fn expanded_websites_follow_wpf_six_columns_and_four_row_limit() {
        assert_eq!(website_viewport(0, false), 60.0);
        assert_eq!(website_viewport(7, false), 60.0);
        assert_eq!(website_viewport(7, true), 120.0);
        assert_eq!(website_viewport(25, true), 240.0);
    }

    #[test]
    #[ignore = "opens only a fixture window for screenshot and pointer verification"]
    fn visual_fixture() {
        use slint::{ComponentHandle, ModelRc, VecModel};
        unsafe {
            windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(-4isize as _);
        }
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .unwrap();
        let ui = crate::Baseline::new().unwrap();
        let names = [
            "哔哩哔哩",
            "DeepSeek",
            "YouTube",
            "GitHub",
            "MDN Web 文档",
            "Minecraft",
            "Rust",
            "Slint",
            "网址 09",
            "网址 10",
            "网址 11",
            "网址 12",
            "网址 13",
            "网址 14",
        ];
        let sites = names.iter().enumerate().map(|(index, name)| json!({
            "id":format!("site-{index}"), "name":name, "url":format!("https://site-{index}.example/")
        })).collect::<Vec<_>>();
        let mut translation = feature("translate-text", "webtools.translation");
        translation.title = "翻译".into();
        translation.icon_data_url = None;
        let mut home = feature("home", "test.home");
        home.title = "Home".into();
        home.icon_token = "plugin".into();
        home.icon_data_url = None;
        let mut config = home.clone();
        config.feature_id = "config-check".into();
        config.title = "Config Check".into();
        let (sites, plugins) = project(&json!(sites), &[home, config, translation]);
        let icon_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.tmp/icons");
        ui.set_website_shortcuts(ModelRc::new(VecModel::from(
            sites
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let icon =
                        slint::Image::load_from_path(&icon_root.join(format!("{index}.png"))).ok();
                    item.visual(icon)
                })
                .collect::<Vec<_>>(),
        )));
        ui.set_plugin_shortcuts(ModelRc::new(VecModel::from(
            plugins
                .iter()
                .map(|item| item.visual(None))
                .collect::<Vec<_>>(),
        )));
        ui.set_expanded(true);
        ui.set_fixture_icons(false);
        ui.on_website_shortcut_chosen(|index| eprintln!("fixture: website-click={index}"));
        ui.on_plugin_shortcut_chosen(|index| eprintln!("fixture: plugin-click={index}"));
        ui.on_dismiss(|| {
            let _ = slint::quit_event_loop();
        });
        slint::Timer::single_shot(std::time::Duration::from_secs(180), || {
            let _ = slint::quit_event_loop();
        });
        ui.show().unwrap();
        eprintln!(
            "fixture: scale={} physical-size={:?}",
            ui.window().scale_factor(),
            ui.window().size()
        );
        ui.run().unwrap();
    }

    #[test]
    #[ignore = "opens a software-rendered fixture window to inspect loading frames"]
    fn loading_collapses_and_animates() {
        use slint::ComponentHandle;
        use std::{cell::RefCell, rc::Rc, time::Duration};

        slint::BackendSelector::new().backend_name("winit".into())
            .renderer_name("software".into()).select().unwrap();
        let ui = crate::Baseline::new().unwrap();
        ui.set_query("example".into());
        ui.show().unwrap();
        let evidence = Rc::new(RefCell::new((Vec::new(), Vec::new(), Vec::new())));
        let recorded = evidence.clone();
        let weak = ui.as_weak();
        let mut stage = 0;
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
            let ui = weak.unwrap();
            let size = || ui.window().size().height as f32 / ui.window().scale_factor();
            let pixels = || {
                let image = ui.window().take_snapshot().unwrap();
                let scale = ui.window().scale_factor();
                let width = image.width() as usize;
                let mut result = Vec::new();
                for y in (26.0 * scale) as usize..(76.0 * scale) as usize {
                    for x in (781.0 * scale) as usize..(831.0 * scale) as usize {
                        let p = image.as_slice()[y * width + x];
                        result.push((p.r, p.g, p.b));
                    }
                }
                result
            };
            match stage {
                0 => { recorded.borrow_mut().0.push(size()); ui.set_plugin_loading(true); }
                1 => { recorded.borrow_mut().0.push(size()); recorded.borrow_mut().1 = pixels(); }
                3 => { recorded.borrow_mut().2 = pixels(); ui.set_plugin_loading(false); }
                4 => { recorded.borrow_mut().0.push(size()); ui.set_query("".into()); ui.set_expanded(true); }
                5 => { recorded.borrow_mut().0.push(size()); ui.set_plugin_loading(true); }
                6 => { recorded.borrow_mut().0.push(size()); let _ = slint::quit_event_loop(); }
                _ => {}
            }
            stage += 1;
        });
        slint::run_event_loop().unwrap();
        let evidence = evidence.borrow();
        assert!(evidence.0[0] > 400.0 && evidence.0[1] < 140.0 && evidence.0[2] > 400.0,
            "query results should collapse while loading and restore afterward: {:?}", evidence.0);
        assert!(evidence.0[3] > 250.0 && evidence.0[4] < 140.0,
            "expanded shortcuts should collapse while loading: {:?}", evidence.0);
        let changed = evidence.1.iter().zip(&evidence.2).filter(|(a, b)| a != b).count();
        assert!(changed > 30, "loading ring must visibly change frames, changed pixels={changed}");
    }
}
