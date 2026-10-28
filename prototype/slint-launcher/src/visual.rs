use super::*;
use serde_json::Value;
use webtools_ipc::projection::FeatureRow;

#[derive(Clone)]
pub(crate) enum Target {
    Application(std::sync::Arc<index::Index>, usize),
    Website(String),
    Feature {
        plugin: String,
        feature: String,
        launcher_text: Option<String>,
    },
    Translation(String),
    WebSearch(String),
    SearchFiles(String),
    FilePath(String),
}

// Strong application matches should compete with direct website-name matches in
// the general launcher, including Chinese pinyin initials such as "wx".
fn local_rank(score: i32, application: bool, remembered: bool) -> i32 {
    let source_bonus = if application {
        if score >= 600 { 400 } else { 100 }
    } else {
        0
    };
    score + source_bonus + if remembered { 25 } else { 0 }
}

#[cfg(test)]
mod ranking_tests {
    use super::{index, local_rank};

    #[test]
    fn application_initials_beat_website_prefix_for_normal_search() {
        let app = index::score_entry("微信", &[], "", "wx").unwrap();
        let website = index::score_entry("wx.mail.qq.com", &[], "", "wx").unwrap();
        assert!(local_rank(app, true, false) > local_rank(website, false, false));
    }

    #[test]
    fn old_selection_is_a_small_bonus_not_an_unconditional_first_place() {
        let weak = index::score_entry("微信开发者工具", &[], "", "wx").unwrap();
        let exact = index::score_entry("wx", &[], "", "wx").unwrap();
        assert!(local_rank(weak, true, true) < local_rank(exact, true, false));
        assert!(local_rank(weak, true, true) > local_rank(weak, true, false));
    }

    #[test]
    fn direct_website_match_beats_weak_application_metadata_match() {
        let website = index::score_entry("wx.mail.qq.com", &[], "", "wx.mail.qq.com").unwrap();
        let app = index::score_entry("工具", &[], "wx.mail.qq.com", "wx.mail.qq.com").unwrap();
        assert!(local_rank(website, false, false) > local_rank(app, true, false));
    }
}

std::thread_local! {
    static SURFACE_CREATE_HOOK: std::cell::Cell<windows_sys::Win32::UI::WindowsAndMessaging::HHOOK> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
    static SURFACE_SHOW_HOOK: std::cell::Cell<windows_sys::Win32::UI::WindowsAndMessaging::HHOOK> =
        const { std::cell::Cell::new(std::ptr::null_mut()) };
}

pub(crate) fn build(
    raw_query: &str,
    websites: &Value,
    features: &[FeatureRow],
    settings: &Value,
    saved_icons: &std::collections::HashMap<String, slint::Image>,
    remembered_app_id: Option<&str>,
) -> (Vec<VisualRow>, Vec<Target>) {
    let raw_query = raw_query.trim();
    if raw_query.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if raw_query
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
    {
        return (Vec::new(), Vec::new());
    }
    if let Some(query) = raw_query.strip_prefix('?') {
        let query = query.trim();
        if query.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let engine_id = settings["defaultSearchEngineId"]
            .as_str()
            .unwrap_or("google");
        let engine = settings["searchEngines"]
            .as_array()
            .and_then(|rows| {
                rows.iter()
                    .find(|row| row["id"] == engine_id && row["enabled"] == true)
            })
            .or_else(|| {
                settings["searchEngines"]
                    .as_array()
                    .and_then(|rows| rows.iter().find(|row| row["enabled"] == true))
            });
        let template = engine
            .and_then(|row| row["template"].as_str())
            .unwrap_or("https://www.google.com/search?q=%s");
        let engine_name = engine
            .and_then(|row| row["name"].as_str())
            .unwrap_or("Google");
        let title = format!("搜索网页：{query}");
        return (
            vec![VisualRow {
                title: title.as_str().into(),
                title_parts: highlight_parts(&title, query),
                subtitle: format!("使用 {engine_name} 搜索").into(),
                icon_index: -1,
                glyph: "⌕".into(),
                ..Default::default()
            }],
            vec![Target::WebSearch(
                template.replace("%s", &percent_encode(query)),
            )],
        );
    }

    let saved_only = raw_query.starts_with('/');
    let query = if saved_only {
        raw_query[1..].trim()
    } else {
        raw_query
    };
    if query.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let needle = query.to_lowercase();
    let index = index::try_global();
    let mut local = Vec::<(i32, String, Target, String)>::new();
    if let Some(index) = index.as_ref().filter(|_| !saved_only) {
        for hit in index.search(query, index.apps.len()) {
            let app = &index.apps[hit.index];
            local.push((
                hit.score,
                app.name.clone(),
                Target::Application(index.clone(), hit.index),
                String::new(),
            ));
        }
    }
    if let Some(items) = websites.as_array() {
        for site in items {
            let name = site["name"].as_str().unwrap_or_default();
            let url = site["url"].as_str().unwrap_or_default();
            let description = site["description"].as_str().unwrap_or_default();
            let aliases = vec![url.to_string()];
            let Some(score) = index::score_entry(name, &aliases, description, &needle) else {
                continue;
            };
            local.push((
                score,
                name.to_string(),
                Target::Website(url.to_string()),
                site["id"].as_str().unwrap_or_default().to_string(),
            ));
        }
    }
    local.sort_by(|left, right| {
        let rank = |item: &(i32, String, Target, String)| {
            let app = match &item.2 {
                Target::Application(index, position) => Some(&index.apps[*position]),
                _ => None,
            };
            let remembered = app.is_some_and(|app| remembered_app_id.is_some_and(|id| app.id == id));
            local_rank(item.0, app.is_some(), remembered)
        };
        rank(right)
            .cmp(&rank(left))
            .then_with(|| matches!(right.2, Target::Application(..)).cmp(&matches!(left.2, Target::Application(..))))
            .then_with(|| index::compare_names_zh(&left.1, &right.1))
    });
    local.truncate(8);

    let mut rows = Vec::new();
    let mut targets = Vec::new();
    for (_, name, target, icon_key) in local {
        let (subtitle, glyph, icon) = match &target {
            Target::Application(index, position) => {
                let app = &index.apps[*position];
                (
                    "本地应用".to_string(),
                    "⌘".to_string(),
                    icons::for_entry(&app.launch),
                )
            }
            Target::Website(url) => (
                url.clone(),
                crate::launcher_shortcuts::website_initial(url),
                saved_icons.get(&icon_key).cloned(),
            ),
            _ => unreachable!(),
        };
        rows.push(VisualRow {
            title: name.into(),
            subtitle: subtitle.into(),
            glyph: glyph.into(),
            icon_index: -1,
            has_icon: icon.is_some(),
            icon: icon.unwrap_or_default(),
            ..Default::default()
        });
        targets.push(target);
    }

    if !saved_only {
        let mut feature_hits: Vec<_> = features
            .iter()
            .filter_map(|feature| {
                if feature.feature_kind == "declarative-compatibility" {
                    return None;
                }
                let aliases = std::iter::once(feature.plugin_name.clone())
                    .chain(feature.commands.iter().cloned())
                    .collect::<Vec<_>>();
                let extra = feature
                    .accepted_contexts
                    .iter()
                    .flat_map(|context| context.sources.iter())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" ");
                let score = index::score_entry(&feature.title, &aliases, &extra, &needle)?;
                let launcher_text = feature.accepted_contexts.iter().any(|context| {
                    context.kind == webtools_ipc::projection::ContextKind::Text
                        && context
                            .sources
                            .iter()
                            .any(|source| source == "launcher-input")
                });
                Some((score, feature.title.clone(), feature, launcher_text))
            })
            .collect();
        feature_hits.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| index::compare_names_zh(&left.1, &right.1))
        });
        feature_hits.truncate(6);
        let translation_feature_visible = feature_hits.iter().any(|(_, _, feature, _)| {
            feature.plugin_id == "webtools.translation" && feature.feature_id == "translate-text"
        });
        for (_, _, feature, accepts_text) in feature_hits {
            let icon = saved_icons
                .get(&format!("{}/{}", feature.plugin_id, feature.feature_id))
                .cloned()
                .or_else(|| {
                    feature
                        .icon_data_url
                        .as_deref()
                        .and_then(icons::from_data_url)
                });
            rows.push(VisualRow {
                title: feature.title.as_str().into(),
                subtitle: feature.plugin_name.as_str().into(),
                icon_index: -1,
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
                icon_token: feature.icon_token.as_str().into(),
                ..Default::default()
            });
            targets.push(Target::Feature {
                plugin: feature.plugin_id.clone(),
                feature: feature.feature_id.clone(),
                launcher_text: accepts_text.then(|| query.to_string()),
            });
        }
        if !translation_feature_visible && query.chars().count() <= 20_000 {
            rows.push(VisualRow {
                title: "翻译".into(),
                subtitle: format!("翻译“{query}”").into(),
                icon_index: -1,
                icon_token: "translate".into(),
                ..Default::default()
            });
            targets.push(Target::Translation(query.to_string()));
        }
        rows.push(VisualRow {
            title: "搜索文件".into(),
            subtitle: format!("搜索文件“{query}”").into(),
            icon_index: -1,
            glyph: "⌕".into(),
            ..Default::default()
        });
        targets.push(Target::SearchFiles(query.to_string()));
    }
    for row in &mut rows {
        row.title_parts = highlight_parts(row.title.as_str(), query);
    }
    (rows, targets)
}

pub(crate) fn highlight_parts(title: &str, query: &str) -> ModelRc<HighlightPart> {
    let ranges = index::match_highlight_ranges(title, query);
    if ranges.is_empty() {
        return ModelRc::default();
    }
    let mut parts = Vec::new();
    let mut offset = 0;
    for range in ranges {
        if offset < range.start {
            parts.push(HighlightPart {
                text: title[offset..range.start].into(),
                highlighted: false,
            });
        }
        parts.push(HighlightPart {
            text: title[range.clone()].into(),
            highlighted: true,
        });
        offset = range.end;
    }
    if offset < title.len() {
        parts.push(HighlightPart {
            text: title[offset..].into(),
            highlighted: false,
        });
    }
    ModelRc::new(VecModel::from(parts))
}

#[cfg(test)]
mod highlight_visual_tests {
    #[test]
    fn website_title_highlighting_does_not_fabricate_an_alias_match() {
        use slint::Model;
        let websites = serde_json::json!([{
            "id": "site", "name": "Visual Studio Code", "url": "https://example.test/"
        }]);
        let icons = std::collections::HashMap::new();
        let (rows, targets) =
            super::build("vs", &websites, &[], &serde_json::json!({}), &icons, None);
        let position = targets.iter().position(|target| matches!(target, super::Target::Website(url) if url == "https://example.test/")).unwrap();
        let parts = &rows[position].title_parts;
        assert_eq!(parts.row_data(0).unwrap().text.as_str(), "V");
        assert!(parts.row_data(0).unwrap().highlighted);
        assert_eq!(parts.row_data(2).unwrap().text.as_str(), "S");
        assert!(parts.row_data(2).unwrap().highlighted);

        let (rows, targets) = super::build(
            "example",
            &websites,
            &[],
            &serde_json::json!({}),
            &icons,
            None,
        );
        let position = targets.iter().position(|target| matches!(target, super::Target::Website(url) if url == "https://example.test/")).unwrap();
        assert_eq!(rows[position].title.as_str(), "Visual Studio Code");
        assert_eq!(rows[position].title_parts.row_count(), 0);

        let (rows, _) = super::build(
            "?example",
            &websites,
            &[],
            &serde_json::json!({}),
            &icons,
            None,
        );
        assert!(rows[0].title_parts.row_data(1).unwrap().highlighted);
        assert_eq!(
            rows[0].title_parts.row_data(1).unwrap().text.as_str(),
            "example"
        );
    }

    #[test]
    #[ignore = "opens a software-rendered fixture window"]
    fn highlight_is_visible_in_software_renderer() {
        use slint::{ComponentHandle, ModelRc, VecModel};
        use std::{cell::RefCell, rc::Rc, time::Duration};

        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .unwrap();
        let ui = crate::Baseline::new().unwrap();
        ui.set_query("vs".into());
        ui.set_selected(0);
        ui.set_fixture_icons(false);
        ui.set_rows(ModelRc::new(VecModel::from(vec![
            crate::VisualRow {
                title: "Visual Studio Code".into(),
                title_parts: super::highlight_parts("Visual Studio Code", "vs"),
                subtitle: "本地应用".into(),
                ..Default::default()
            },
            crate::VisualRow {
                title: "系统恢复设置".into(),
                title_parts: super::highlight_parts("系统恢复设置", "hui"),
                subtitle: "本地应用".into(),
                ..Default::default()
            },
            crate::VisualRow {
                title: "Visual Studio Code — 一个非常长的结果名称，一个非常长的结果名称，一个非常长的结果名称，一个非常长的结果名称，一个非常长的结果名称".into(),
                title_parts: super::highlight_parts("Visual Studio Code — 一个非常长的结果名称，一个非常长的结果名称，一个非常长的结果名称，一个非常长的结果名称，一个非常长的结果名称", "vs"),
                subtitle: "长标题截断示例".into(),
                ..Default::default()
            },
        ])));
        ui.show().unwrap();
        let result = Rc::new(RefCell::new(None));
        let capture = result.clone();
        let weak = ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(400), move || {
            let ui = weak.unwrap();
            *capture.borrow_mut() = Some((
                ui.window().take_snapshot().unwrap(),
                ui.window().scale_factor(),
            ));
            let _ = slint::quit_event_loop();
        });
        slint::run_event_loop().unwrap();
        let result = result.borrow();
        let (image, scale) = result.as_ref().unwrap();
        let width = image.width() as usize;
        let count_orange = |top: f32| {
            let mut count = 0;
            for y in (top * scale) as usize..((top + 22.0) * scale) as usize {
                for x in (73.0 * scale) as usize..(360.0 * scale) as usize {
                    let pixel = image.as_slice()[y * width + x];
                    if pixel.r > 180 && pixel.g > 60 && pixel.g < 180 && pixel.b < 100 {
                        count += 1;
                    }
                }
            }
            count
        };
        if let Some(path) = std::env::var_os("WT_HIGHLIGHT_SNAPSHOT") {
            use std::io::Write;
            let mut header = [0u8; 54];
            header[..2].copy_from_slice(b"BM");
            header[2..6].copy_from_slice(&(54 + image.width() * image.height() * 4).to_le_bytes());
            header[10..14].copy_from_slice(&54u32.to_le_bytes());
            header[14..18].copy_from_slice(&40u32.to_le_bytes());
            header[18..22].copy_from_slice(&image.width().to_le_bytes());
            header[22..26].copy_from_slice(&image.height().to_le_bytes());
            header[26..28].copy_from_slice(&1u16.to_le_bytes());
            header[28..30].copy_from_slice(&32u16.to_le_bytes());
            let mut file = std::fs::File::create(path).unwrap();
            file.write_all(&header).unwrap();
            for row in image.as_slice().chunks(width).rev() {
                for pixel in row {
                    file.write_all(&[pixel.b, pixel.g, pixel.r, pixel.a])
                        .unwrap();
                }
            }
        }
        let english = count_orange(101.0);
        let chinese = count_orange(153.0);
        eprintln!(
            "highlight fixture: scale={scale} size={}x{} english={english} chinese={chinese}",
            image.width(),
            image.height()
        );
        assert!(english > 5, "English initials should render in orange");
        assert!(chinese > 5, "Pinyin match should render in orange");
    }
}

fn percent_encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.trim().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Centre horizontally on the monitor under the cursor, a fifth of the way down.
pub fn place_launcher_now() {
    place_launcher();
}

/// Disable per-window DWM transitions so showing and hiding the launcher surfaces is immediate.
pub fn disable_window_transitions(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe {
        use windows_sys::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
        };
        let disabled: i32 = 1;
        DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            (&disabled as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        ) >= 0
    }
}

/// Reveal the authenticated Electron target and hide the current Slint surface
/// in one Win32 batch, matching the WPF window handoff path.
pub fn show_target_and_hide_surface(target: u64, expected_process_id: u32, source: isize) -> bool {
    if target == 0 {
        return false;
    }
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            BeginDeferWindowPos, DeferWindowPos, EndDeferWindowPos, GetWindowThreadProcessId,
            IsIconic, IsWindow, SetForegroundWindow, SetLayeredWindowAttributes, ShowWindow,
            LWA_ALPHA, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE,
            SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
        };

        let target = target as usize as windows_sys::Win32::Foundation::HWND;
        if IsWindow(target) == 0 {
            return false;
        }
        let mut process_id = 0;
        GetWindowThreadProcessId(target, &mut process_id);
        if process_id != expected_process_id {
            return false;
        }
        if SetLayeredWindowAttributes(target, 0, u8::MAX, LWA_ALPHA) == 0 {
            return false;
        }
        let _ = disable_window_transitions(target as isize);
        if IsIconic(target) != 0 {
            ShowWindow(target, SW_SHOWNOACTIVATE);
        }

        let hide_source = source != 0 && source != target as isize;
        let mut batch = BeginDeferWindowPos(if hide_source { 2 } else { 1 });
        if batch.is_null() {
            return false;
        }
        batch = DeferWindowPos(
            batch,
            target,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOOWNERZORDER,
        );
        if batch.is_null() {
            return false;
        }
        if hide_source {
            let source = source as usize as windows_sys::Win32::Foundation::HWND;
            if IsWindow(source) == 0 {
                return false;
            }
            batch = DeferWindowPos(
                batch,
                source,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_HIDEWINDOW,
            );
            if batch.is_null() {
                return false;
            }
        }
        if EndDeferWindowPos(batch) == 0 {
            return false;
        }
        // Keep the batched reveal non-activating, then explicitly focus the
        // target after the source surface is hidden. This mirrors WPF's
        // NativeWindowHandoff.Focus(target) and avoids leaving Electron only
        // reachable from its taskbar button.
        SetForegroundWindow(target);
        true
    }
}

/// Keep the flag in winit's own window state, before HWND creation. Patching
/// GWL_EXSTYLE alone is overwritten when winit later updates its window flags.
pub fn native_backend(renderer: &str) -> slint::BackendSelector {
    use slint::winit_030::winit::platform::windows::WindowAttributesExtWindows;
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name(renderer.into())
        // Slint assigns component titles after this hook. All windows in this
        // native shell are resident surfaces; Electron runs in another process.
        .with_winit_window_attributes_hook(|attributes| attributes.with_skip_taskbar(true))
}

/// Apply the panel's native semantics while its HWND is still hidden.
pub fn prepare_panel_window_before_show(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let disabled = disable_window_transitions(hwnd);
    unsafe { apply_panel_noactivate_style(hwnd as _) };
    position_panel_hwnd(hwnd);
    disabled
}

/// Position a newly-created panel HWND before it becomes visible. Winit creates
/// the window at its default origin; moving it only after `show()` produces a
/// visible top-left flash on the first invocation.
fn position_panel_hwnd(hwnd: isize) {
    if hwnd == 0 {
        return;
    }
    unsafe {
        use windows_sys::Win32::{
            Foundation::POINT,
            Graphics::Gdi::{
                GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            },
            UI::WindowsAndMessaging::{
                GetCursorPos, GetWindowRect, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE,
            },
        };
        let mut cursor = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut cursor) == 0 {
            return;
        }
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let mut rect = std::mem::zeroed();
        if GetWindowRect(hwnd as _, &mut rect) == 0 {
            return;
        }
        let work = info.rcWork;
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return;
        }
        let gap = 12;
        let x = (cursor.x - width / 2).clamp(work.left, (work.right - width).max(work.left));
        let y = if cursor.y + gap + height <= work.bottom {
            cursor.y + gap
        } else {
            cursor.y - gap - height
        }
        .clamp(work.top, (work.bottom - height).max(work.top));
        SetWindowPos(
            hwnd as _,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Keep panel activation styles from the pre-create callback, and set DWM
/// transitions after HWND creation but before Windows processes the first show.
pub fn arm_surface_window_show_hook() -> bool {
    unsafe {
        use windows_sys::Win32::{
            System::Threading::GetCurrentThreadId,
            UI::WindowsAndMessaging::{SetWindowsHookExW, WH_CALLWNDPROC, WH_CBT},
        };
        let thread_id = GetCurrentThreadId();
        let create_hook = SetWindowsHookExW(
            WH_CBT,
            Some(surface_window_create_hook),
            std::ptr::null_mut(),
            thread_id,
        );
        let show_hook = SetWindowsHookExW(
            WH_CALLWNDPROC,
            Some(surface_window_show_hook),
            std::ptr::null_mut(),
            thread_id,
        );
        if create_hook.is_null() || show_hook.is_null() {
            if !create_hook.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(create_hook);
            }
            if !show_hook.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(show_hook);
            }
            return false;
        }
        SURFACE_CREATE_HOOK.with(|slot| {
            let previous = slot.replace(create_hook);
            if !previous.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(previous);
            }
        });
        SURFACE_SHOW_HOOK.with(|slot| {
            let previous = slot.replace(show_hook);
            if !previous.is_null() {
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(previous);
            }
        });
        true
    }
}

/// Cancel a pending hook if surface creation or presentation is abandoned.
pub fn cancel_surface_window_show_hook() {
    SURFACE_CREATE_HOOK.with(|slot| {
        let hook = slot.replace(std::ptr::null_mut());
        if !hook.is_null() {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook) };
        }
    });
    SURFACE_SHOW_HOOK.with(|slot| {
        let hook = slot.replace(std::ptr::null_mut());
        if !hook.is_null() {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(hook) };
        }
    });
}

unsafe extern "system" fn surface_window_create_hook(
    code: i32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, UnhookWindowsHookEx, CBT_CREATEWNDW, HCBT_CREATEWND,
    };

    let hook = SURFACE_CREATE_HOOK.with(|slot| slot.get());
    let mut matched_surface = false;
    if code == HCBT_CREATEWND as i32 && lparam != 0 {
        let create = &*(lparam as *const CBT_CREATEWNDW);
        if !create.lpcs.is_null() && (*create.lpcs).hwndParent.is_null() {
            let title = (*create.lpcs).lpszName;
            let matched_panel = wide_string_equals(title, "WebTools Super Panel");
            let matched_launcher = wide_string_equals(title, "WebTools Launcher Replica");
            matched_surface = matched_panel || matched_launcher;
            if matched_panel {
                apply_panel_noactivate_style(wparam as _);
            }
        }
    }

    let result = CallNextHookEx(hook, code, wparam, lparam);
    if matched_surface && !hook.is_null() {
        SURFACE_CREATE_HOOK.with(|slot| slot.set(std::ptr::null_mut()));
        UnhookWindowsHookEx(hook);
    }
    result
}

unsafe extern "system" fn surface_window_show_hook(
    code: i32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, UnhookWindowsHookEx, CWPSTRUCT, WM_SHOWWINDOW,
    };

    let hook = SURFACE_SHOW_HOOK.with(|slot| slot.get());
    let mut matched_surface = false;
    if code >= 0 && lparam != 0 {
        let message = &*(lparam as *const CWPSTRUCT);
        if message.message == WM_SHOWWINDOW && message.wParam != 0 {
            let title = window_title(message.hwnd);
            let matched_panel = title == "WebTools Super Panel";
            let matched_launcher = title == "WebTools Launcher Replica";
            matched_surface = matched_panel || matched_launcher;
            if matched_panel {
                prepare_panel_window_before_show(message.hwnd as isize);
            } else if matched_launcher {
                disable_window_transitions(message.hwnd as isize);
            }
        }
    }

    let result = CallNextHookEx(hook, code, wparam, lparam);
    if matched_surface && !hook.is_null() {
        SURFACE_SHOW_HOOK.with(|slot| slot.set(std::ptr::null_mut()));
        UnhookWindowsHookEx(hook);
    }
    result
}

unsafe fn window_title(hwnd: windows_sys::Win32::Foundation::HWND) -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowTextW;
    let mut buffer = [0u16; 128];
    let length = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

unsafe fn wide_string_equals(value: windows_sys::core::PCWSTR, expected: &str) -> bool {
    if value.is_null() || (value as usize) <= u16::MAX as usize {
        return false;
    }
    for (index, unit) in expected.encode_utf16().enumerate() {
        if *value.add(index) != unit {
            return false;
        }
    }
    *value.add(expected.encode_utf16().count()) == 0
}

unsafe fn apply_panel_noactivate_style(hwnd: windows_sys::Win32::Foundation::HWND) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE,
    };
    const WS_EX_NOACTIVATE: isize = 0x0800_0000;
    const WS_EX_TOOLWINDOW: isize = 0x0000_0080;
    const WS_EX_TOPMOST: isize = 0x0000_0008;
    let current = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    SetWindowLongPtrW(
        hwnd,
        GWL_EXSTYLE,
        current | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
    );
}

fn place_launcher() {
    unsafe {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetCursorPos, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE,
        };
        let hwnd = window_by_title("WebTools Launcher Replica");
        if hwnd == 0 {
            return;
        }
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) == 0 {
            return;
        }
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let (width, height) = window_size(hwnd);
        let work = info.rcWork;
        let work_width = work.right - work.left;
        let work_height = work.bottom - work.top;
        let x = work.left + (work_width - width) / 2;
        let y = work.top + (work_height - height) * 22 / 100;
        // Match WPF MainWindow.Topmost=true on every show. Keeping NOACTIVATE here
        // preserves the separate foreground activation performed by the controller.
        SetWindowPos(
            hwnd as _,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
        if std::env::var("WT_PLACE_DEBUG").is_ok() {
            eprintln!("place: work={work_width}x{work_height} at ({},{}) window={width}x{height} -> ({x},{y})", work.left, work.top);
        }
    }
}

/// Re-apply the borderless tool-window styles after every show: winit resets
/// the extended style when the window is mapped, so this must run late.
/// Largest visible top-level window owned by this process.
// ---------------------------------------------------------------------------
// Selection capture: wait for the hotkey modifiers to be released, snapshot the
// clipboard, emulate Ctrl+C, read the new text, then put the clipboard back.
// This is the least invasive way to read a selection from a foreign process.
// ---------------------------------------------------------------------------
fn read_clipboard_text() -> Option<String> {
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, GetClipboardData, OpenClipboard,
        };
        use windows_sys::Win32::System::Memory::{GlobalLock, GlobalUnlock};
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT as u32);
        let mut text = None;
        if !handle.is_null() {
            let ptr = GlobalLock(handle) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                text = Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                    ptr, len,
                )));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
        text
    }
}

fn write_clipboard_text(text: &str) {
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        use windows_sys::Win32::System::DataExchange::{
            CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
        };
        use windows_sys::Win32::System::Memory::{
            GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
        };
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let handle = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2);
        if !handle.is_null() {
            let ptr = GlobalLock(handle) as *mut u16;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                GlobalUnlock(handle);
                SetClipboardData(CF_UNICODETEXT as u32, handle);
            }
        }
        CloseClipboard();
    }
}

fn send_copy() {
    unsafe {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            keybd_event, KEYEVENTF_KEYUP, VK_CONTROL,
        };
        keybd_event(VK_CONTROL as u8, 0, 0, 0);
        keybd_event(0x43, 0, 0, 0);
        keybd_event(0x43, 0, KEYEVENTF_KEYUP, 0);
        keybd_event(VK_CONTROL as u8, 0, KEYEVENTF_KEYUP, 0);
    }
}

pub fn capture_selection() -> Option<String> {
    capture_selection_while(|| true)
}

/// Stop before injecting Copy if the originating surface or foreground changed.
pub fn capture_selection_while(allowed: impl Fn() -> bool) -> Option<String> {
    if !allowed() {
        return None;
    }
    let started = std::time::Instant::now();
    unsafe {
        use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
            GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        };
        // The hotkey fires on key down; injecting Ctrl+C before the user lets go
        // would turn it into Ctrl+Alt+Shift+C in the target app.
        let modifiers = [VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN];
        for _ in 0..80 {
            if !allowed() {
                return None;
            }
            let held = modifiers
                .iter()
                .any(|key| GetAsyncKeyState(*key as i32) as u16 & 0x8000 != 0);
            if !held {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if std::env::var_os("WT_PANEL_TIMING").is_some() {
            eprintln!("timing: modifier-wait={:?}", started.elapsed());
        }
        let backup = read_clipboard_text();
        if std::env::var_os("WT_PANEL_TIMING").is_some() {
            eprintln!("timing: backup-read={:?}", started.elapsed());
        }
        let before = GetClipboardSequenceNumber();
        if !allowed() {
            return None;
        }
        send_copy();
        let mut captured = None;
        for _ in 0..50 {
            if !allowed() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            if GetClipboardSequenceNumber() != before {
                captured = read_clipboard_text();
                break;
            }
        }
        if let Some(text) = &backup {
            write_clipboard_text(text);
        }
        if std::env::var_os("WT_PANEL_TIMING").is_some() {
            eprintln!(
                "timing: copy-total={:?} hit={}",
                started.elapsed(),
                captured.is_some()
            );
        }
        captured.filter(|text| !text.trim().is_empty())
    }
}

#[cfg(test)]
mod capture_tests {
    #[test]
    fn cancelled_capture_returns_before_any_native_copy_work() {
        assert_eq!(super::capture_selection_while(|| false), None);
    }
}

pub fn current_foreground() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

pub fn allow_foreground_process(process_id: u32) -> bool {
    if process_id == 0 {
        return false;
    }
    let allowed = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(process_id) != 0
    };
    if std::env::var("WT_FOCUS_TRACE").is_ok() {
        eprintln!("focus: allow-process pid={process_id} allowed={allowed}");
    }
    allowed
}

pub fn restore_foreground(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    let (requested, foreground) = unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetForegroundWindow, SetForegroundWindow,
        };
        let requested = SetForegroundWindow(hwnd as _) != 0;
        (requested, GetForegroundWindow() as isize)
    };
    let active = requested && foreground == hwnd;
    if std::env::var("WT_FOCUS_TRACE").is_ok() {
        eprintln!("focus: hwnd=0x{hwnd:x} requested={requested} foreground=0x{foreground:x} active={active}");
    }
    active
}

pub fn panel_noactivate() {
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let hwnd = window_by_title("WebTools Super Panel");
        if hwnd == 0 {
            return;
        }
        let current = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        apply_panel_noactivate_style(hwnd as _);
        let wanted = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        SetWindowPos(
            hwnd as _,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED | SWP_NOACTIVATE,
        );
        // NOTE: hide/show without activation blanks the software-rendered surface,
        // so we keep the window mapped and just refuse activation instead.
        if std::env::var("WT_PANEL_DEBUG").is_ok() {
            eprintln!("panel: exstyle {current:#x} -> {wanted:#x}");
        }
    }
}

/// Put the panel next to the cursor, flipping above it when there is no room below.
pub fn panel_place() {
    unsafe {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetCursorPos, GetWindowRect, SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOSIZE,
        };
        let hwnd = window_by_title("WebTools Super Panel");
        if hwnd == 0 {
            return;
        }
        let mut rect: windows_sys::Win32::Foundation::RECT = std::mem::zeroed();
        GetWindowRect(hwnd as _, &mut rect);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let gap = 16;
        let mut x = cursor.x + gap;
        let mut y = cursor.y + gap;
        if x + width > work.right {
            x = cursor.x - width - gap;
        }
        if y + height > work.bottom {
            y = cursor.y - height - gap;
        }
        x = x.max(work.left).min(work.right - width);
        y = y.max(work.top).min(work.bottom - height);
        SetWindowPos(
            hwnd as _,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE,
        );
        if std::env::var("WT_PANEL_DEBUG").is_ok() {
            eprintln!(
                "panel: cursor=({},{}) size={width}x{height} -> ({x},{y}) work={}x{}",
                cursor.x,
                cursor.y,
                work.right - work.left,
                work.bottom - work.top
            );
        }
    }
}

/// Capture the cursor once when the panel trigger is handled. Later layout passes
/// must reuse this point instead of following subsequent pointer movement.
pub fn cursor_position() -> Option<(i32, i32)> {
    unsafe {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut point = POINT { x: 0, y: 0 };
        (GetCursorPos(&mut point) != 0).then_some((point.x, point.y))
    }
}

/// Apply WPF work-area bounds and place the actual panel at the trigger position.
pub fn prepare_panel(panel: &SuperPanel, anchor: Option<(i32, i32)>) {
    unsafe {
        use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::*, UI::WindowsAndMessaging::*};
        let hwnd = window_by_title("WebTools Super Panel");
        if hwnd == 0 {
            return;
        }
        panel_noactivate();
        let Some((x, y)) = anchor else {
            return;
        };
        let point = POINT { x, y };
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let scale = panel.window().scale_factor().max(0.5);
        let work = info.rcWork;
        panel.set_available_width(
            ((work.right - work.left) as f32 / scale - 16.0).clamp(1.0, 330.0),
        );
        panel.set_available_height(
            ((work.bottom - work.top) as f32 / scale - 16.0).clamp(1.0, 560.0),
        );
        let mut rect = std::mem::zeroed();
        if GetWindowRect(hwnd as _, &mut rect) == 0 {
            return;
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let x = (point.x - width / 2).clamp(work.left, (work.right - width).max(work.left));
        let gap = (12.0 * scale).round() as i32;
        let y = if point.y + gap + height <= work.bottom {
            point.y + gap
        } else {
            point.y - gap - height
        };
        let y = y.clamp(work.top, (work.bottom - height).max(work.top));
        SetWindowPos(
            hwnd as _,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}
