//! One normal application entry, owning both surfaces and one Manager runtime.
use crate::panel_settings::PanelSettingsController;

struct SingleInstanceGuard(windows_sys::Win32::Foundation::HANDLE);

impl SingleInstanceGuard {
    fn acquire() -> std::io::Result<Option<Self>> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{
            CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
        };
        use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

        let mutex_name = if crate::config::production_mode() {
            "Local\\WebTools.NativeHost"
        } else {
            "Local\\WebTools.NativeHost.Slint"
        };
        let name: Vec<u16> = std::ffi::OsStr::new(mutex_name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        match unsafe { WaitForSingleObject(handle, 0) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Some(Self(handle))),
            WAIT_TIMEOUT => {
                unsafe { CloseHandle(handle) };
                Ok(None)
            }
            _ => {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(handle) };
                Err(error)
            }
        }
    }
}

impl Drop for SingleInstanceGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.0);
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

use crate::{
    app_state::{Surface, SurfaceState},
    config::Config,
    host_runtime::{HostCommand, HostEvent, HostRuntime},
    native_input,
    panel_controller::PanelController,
    shell_context::ShellContext,
    visual, Baseline,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
use webtools_ipc::projection::FeatureRow;
use webtools_ipc::settings::{SettingsChange, SettingsError, SettingsPhase};
use webtools_ipc::super_panel::{PanelOperation, PanelStatus};

/// Bumped on every trigger so latency can be split into capture vs. first frame.
pub static TRIGGER_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

fn timing() -> bool {
    std::env::var_os("WT_PANEL_TIMING").is_some()
}

#[derive(Clone)]
struct PanelResultBinding {
    request_id: String,
    plugin: String,
    feature: String,
    cancellable: bool,
}

#[derive(Clone)]
struct PendingWindowHandoff {
    request_id: String,
    session_id: String,
    source: Surface,
}

fn is_translation_preview_candidate(text: &str) -> bool {
    text.encode_utf16().count() <= 3000
        && text.chars().any(|ch| ch.is_ascii_alphabetic())
        && text.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    ' ' | '\t'
                        | '\r'
                        | '\n'
                        | '.'
                        | ','
                        | '?'
                        | '!'
                        | ':'
                        | ';'
                        | '\''
                        | '"'
                        | '('
                        | ')'
                        | '_'
                        | '-'
                        | '\u{2018}'
                        | '\u{2019}'
                        | '\u{201c}'
                        | '\u{201d}'
                        | '\u{2013}'
                        | '\u{2014}'
                )
        })
}

fn manager_connection_notice(event: &HostEvent) -> Option<&'static str> {
    match event {
        HostEvent::Reconnecting { .. } => Some("正在连接 WebTools 管理端…"),
        HostEvent::Connected => Some(""),
        HostEvent::Disconnected(_) => Some("WebTools 管理端连接失败。"),
        _ => None,
    }
}

#[cfg(test)]
mod manager_connection_status_tests {
    use super::*;

    #[test]
    fn reconnect_events_update_the_launcher_notice_and_success_clears_it() {
        assert_eq!(
            manager_connection_notice(&HostEvent::Reconnecting { attempt: 0 }),
            Some("正在连接 WebTools 管理端…")
        );
        assert_eq!(
            manager_connection_notice(&HostEvent::Connected),
            Some("")
        );
        assert_eq!(
            manager_connection_notice(&HostEvent::Disconnected("pipe lost".into())),
            Some("WebTools 管理端连接失败。")
        );
    }
}

#[cfg(test)]
#[path = "app_lifecycle_tests.rs"]
mod lifecycle_tests;
#[cfg(test)]
#[path = "app_panel_tests.rs"]
mod panel_tests;
#[cfg(test)]
#[path = "app_responsiveness_tests.rs"]
mod responsiveness_tests;
#[cfg(test)]
#[path = "app_settings_tests.rs"]
mod settings_tests;

pub enum AppEvent {
    ShowLauncher,
    ToggleLauncher,
    TogglePanel,
    Dismiss,
    Escape,
    Quit,
    Captured {
        generation: u64,
        text: String,
        context: Option<ShellContext>,
        foreground: isize,
        anchor: Option<(i32, i32)>,
    },
    CaptureAborted(u64),
    Action(i32),
    Board(i32),
    Page(i32),
    BoardContextAction(i32, i32),
    BoardReorder(i32, i32),
    Add,
    Pin(i32),
    CloseResult,
    AIConsentDecision(bool),
    SystemThemeChanged,
    AppCatalogReady,
    MousePanel(u64),
    TrayPanel,
    TraySettings,
    FileSearchComplete {
        generation: u64,
        result: Result<crate::file_search::SearchOutput, String>,
    },
    FileSearchIconsComplete {
        generation: u64,
        icon_pixels: Vec<Option<(Vec<u8>, u32)>>,
    },
    FileSearchStatus {
        generation: u64,
        status: String,
    },
}

struct NativeApp {
    launcher: Baseline,
    state: RefCell<SurfaceState>,
    panel: RefCell<Option<PanelController>>,
    rows: RefCell<Vec<FeatureRow>>,
    projection_cache: webtools_ipc::projection_cache::ProjectionCache,
    targets: RefCell<Vec<visual::Target>>,
    website_targets: RefCell<Vec<visual::Target>>,
    plugin_targets: RefCell<Vec<visual::Target>>,
    launcher_icons: RefCell<std::collections::HashMap<String, slint::Image>>,
    config: Config,
    runtime: RefCell<Option<HostRuntime>>,
    ownership: webtools_ipc::HostOwnership,
    active_session: RefCell<Option<String>>,
    pending_handoff: RefCell<Option<PendingWindowHandoff>>,
    focus_request_pending: Cell<bool>,
    launcher_focus_pending: Rc<Cell<bool>>,
    panel_result: RefCell<Option<PanelResultBinding>>,
    host_events: RefCell<Option<Receiver<HostEvent>>>,
    events: Sender<AppEvent>,
    receiver: Receiver<AppEvent>,
    external: Cell<isize>,
    workers: RefCell<Vec<std::thread::JoinHandle<()>>>,
    capture_cancel: RefCell<Option<Arc<AtomicBool>>>,
    settings: RefCell<serde_json::Value>,
    launcher_state: Arc<std::sync::Mutex<webtools_ipc::state::LauncherStateStore>>,
    input: RefCell<Option<native_input::NativeInputHandle>>,
    panel_settings: Rc<RefCell<PanelSettingsController>>,
    search_generation: Arc<AtomicU64>,
    file_hits: RefCell<Vec<crate::file_search::FileHit>>,
    file_icon_pixels: RefCell<Vec<Option<(Vec<u8>, u32)>>>,
    file_category: Cell<crate::file_search::FileCategory>,
}
impl NativeApp {
    fn update_panel_settings(
        &self,
        operation: PanelOperation,
    ) -> Result<PanelStatus, SettingsError> {
        let PanelOperation::Update(settings) = operation else {
            return Ok(self.panel_settings.borrow().status());
        };
        let mut configured = false;
        let status = self.panel_settings.borrow_mut().update(settings, |s| {
            let ok = self
                .input
                .borrow()
                .as_ref()
                .is_some_and(|input| input.configure_panel(s));
            configured |= ok;
            ok
        });
        if configured {
            let capture_pending = self.state.borrow().is_pending();
            if let Some(cancelled) = self.capture_cancel.borrow_mut().take() {
                cancelled.store(true, Ordering::SeqCst);
            }
            native_input::observe_capture(false);
            if capture_pending {
                self.hide();
            } else {
                self.state.borrow_mut().cancel_pending();
            }
        }
        if !status.settings.enabled && self.state.borrow().active == Surface::Panel {
            self.hide();
        }
        Ok(status)
    }
    fn apply_visual_settings(&self, state: &serde_json::Value) {
        *self.settings.borrow_mut() = state.clone();
        self.apply_theme();
        self.refresh_shortcuts();
    }
    fn apply_theme(&self) {
        let preference = self.settings.borrow()["theme"]
            .as_str()
            .unwrap_or("dark")
            .to_string();
        let light =
            crate::settings_ui::effective_light(&preference, crate::settings_ui::system_light());
        self.launcher.global::<crate::HostTheme>().set_light(light);
        if let Some(panel) = self.panel.borrow().as_ref() {
            panel.ui.global::<crate::HostTheme>().set_light(light);
        }
    }
    fn update_native_settings(
        &self,
        phase: SettingsPhase,
        change: &SettingsChange,
    ) -> Result<(), SettingsError> {
        let input = self.input.borrow();
        let input = input.as_ref().ok_or_else(SettingsError::unavailable)?;
        match phase {
            SettingsPhase::Prepare => {
                if change.next["launchOnStartup"] != change.previous["launchOnStartup"] && !crate::startup::available() {
                    return Err(SettingsError {
                        code: "STARTUP_UNAVAILABLE",
                        message: "Portable 预览不修改开机启动项，请在安装版 WebTools 中设置。",
                    });
                }
                if !input.prepare(change.next["quickSearchShortcut"].as_str().unwrap_or("")) {
                    return Err(SettingsError::hotkey_unavailable());
                }
            }
            SettingsPhase::Commit => {
                if !input.commit() {
                    return Err(SettingsError::unavailable());
                }
                if change.next["launchOnStartup"] != change.previous["launchOnStartup"] {
                    crate::startup::apply(change.next["launchOnStartup"].as_bool().unwrap_or(false))
                        .map_err(|_| SettingsError::unavailable())?;
                }
                self.apply_visual_settings(&change.next);
            }
            SettingsPhase::Rollback => {
                if !input.rollback() {
                    return Err(SettingsError::unavailable());
                }
                if change.next["launchOnStartup"] != change.previous["launchOnStartup"] && crate::startup::available() {
                    crate::startup::apply(change.previous["launchOnStartup"].as_bool().unwrap_or(false))
                        .map_err(|_| SettingsError::unavailable())?;
                }
                if self.settings.borrow()["quickSearchShortcut"]
                    != change.previous["quickSearchShortcut"]
                {
                    if !input.prepare(
                        change.previous["quickSearchShortcut"]
                            .as_str()
                            .unwrap_or(""),
                    ) || !input.commit()
                    {
                        return Err(SettingsError::hotkey_unavailable());
                    }
                }
                self.apply_visual_settings(&change.previous);
            }
        }
        Ok(())
    }
    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.launcher.on_query_edited(move |query| {
            if let Some(app) = weak.upgrade() {
                app.query(&query);
                if query.chars().count() >= app.config.min_chars.unwrap_or(2) {
                    app.ensure_manager();
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_move_selection(move |direction| {
            if let Some(app) = weak.upgrade() {
                app.launcher.set_selected(crate::search::move_selection(
                    app.launcher.get_selected(),
                    direction,
                    app.launcher.get_rows().row_count(),
                ));
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_submit(move || {
            let Some(app) = weak.upgrade() else { return };
            let target = app
                .targets
                .borrow()
                .get(app.launcher.get_selected().max(0) as usize)
                .cloned();
            app.activate_target(target);
        });
        let events = self.events.clone();
        self.launcher.on_dismiss(move || {
            let _ = events.send(AppEvent::Dismiss);
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_translate(move || {
            if let Some(app) = weak.upgrade() {
                app.open_translation(app.launcher.get_query().to_string());
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_refresh_shortcuts(move || {
            if let Some(app) = weak.upgrade() {
                app.refresh_shortcuts();
                app.ensure_manager();
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_website_shortcut_chosen(move |index| {
            if let Some(app) = weak.upgrade() {
                let target = app
                    .website_targets
                    .borrow()
                    .get(index.max(0) as usize)
                    .cloned();
                app.activate_target(target);
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_plugin_shortcut_chosen(move |index| {
            if let Some(app) = weak.upgrade() {
                let target = app
                    .plugin_targets
                    .borrow()
                    .get(index.max(0) as usize)
                    .cloned();
                app.activate_target(target);
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_open_manager(move || {
            if let Some(app) = weak.upgrade() {
                app.open_manager_page("favorites");
            }
        });
        let events = self.events.clone();
        self.launcher.on_close_result(move || {
            let _ = events.send(AppEvent::CloseResult);
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_file_category_chosen(move |index| {
            if let Some(app) = weak.upgrade() {
                let category = crate::file_search::FileCategory::from_index(index);
                app.file_category.set(category);
                app.launcher.set_file_category_selected(category.index());
                app.present_file_hits();
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_file_context_requested(move |index| {
            if let Some(app) = weak.upgrade() {
                if let Some(visual::Target::FilePath(path)) =
                    app.targets.borrow().get(index.max(0) as usize)
                {
                    app.launcher.set_selected(index);
                    app.launcher.set_file_category_focused(false);
                    app.launcher
                        .set_file_selected_is_folder(std::path::Path::new(path).is_dir());
                    app.launcher.set_file_menu_visible(true);
                }
            }
        });
        let weak = Rc::downgrade(self);
        self.launcher.on_file_operation_chosen(move |operation| {
            if let Some(app) = weak.upgrade() {
                app.launcher.set_file_menu_visible(false);
                let selected = app.launcher.get_selected().max(0) as usize;
                let path = match app.targets.borrow().get(selected) {
                    Some(visual::Target::FilePath(path)) => Some(path.clone()),
                    _ => None,
                };
                if let Some(path) = path {
                    match crate::file_search::perform_file_operation(&path, operation) {
                        Ok(()) if operation == 0 => app.hide(),
                        Ok(()) => app.launcher.set_status("操作已完成。".into()),
                        Err(message) => app.launcher.set_status(message.into()),
                    }
                }
            }
        });
        let launcher = self.launcher.as_weak();
        self.launcher.on_drag_window(move || {
            let launcher = launcher.clone();
            // Finish the Slint pointer-down dispatch before entering Windows' move loop.
            slint::Timer::single_shot(Duration::ZERO, move || unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::*;
                windows_sys::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
                let hwnd = crate::window_by_title("WebTools Launcher Replica");
                if hwnd != 0 {
                    SendMessageW(hwnd as _, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
                }
                if let Some(launcher) = launcher.upgrade() {
                    launcher.window().dispatch_event(slint::platform::WindowEvent::PointerExited);
                }
            });
        });
    }
    fn activate_target(&self, target: Option<visual::Target>) {
        match target {
            Some(visual::Target::Application(catalog, index)) => {
                match catalog.launch(index) {
                    Ok(_) => {
                        let query = self.launcher.get_query().to_string();
                        let app_id = &catalog.apps[index].id;
                        if let Err(error) =
                            self.launcher_state.lock().unwrap().remember(&query, app_id)
                        {
                            eprintln!("search memory: failed to remember query: {error}");
                        }
                        self.hide();
                    }
                    Err(error) => self
                        .launcher
                        .set_status(format!("启动失败：{error}").into()),
                }
            }
            Some(visual::Target::Website(url) | visual::Target::WebSearch(url)) => {
                if crate::shell::open_url(&url) {
                    self.hide();
                } else {
                    self.launcher.set_status("无法打开该网址".into());
                }
            }
            Some(visual::Target::FilePath(path)) => {
                match crate::file_search::perform_file_operation(&path, 0) {
                    Ok(()) => self.hide(),
                    Err(error) => self.launcher.set_status(error.into()),
                }
            }
            Some(visual::Target::Feature {
                plugin,
                feature,
                launcher_text,
            }) => {
                let command =
                    HostCommand::invoke_launcher(plugin.clone(), feature.clone(), launcher_text);
                if self.is_command_feature(&plugin, &feature) {
                    self.launcher.set_status("正在执行插件功能…".into());
                    self.command(command);
                } else {
                    self.launcher.set_status("正在打开插件功能…".into());
                    self.open_manager_target(command);
                }
            }
            Some(visual::Target::Translation(text)) => self.open_translation(text),
            Some(visual::Target::SearchFiles(query)) => {
                let file_query = format!("file:{query}");
                self.launcher.set_query(file_query.clone().into());
                self.query(&file_query);
                self.launcher.invoke_focus_input_at_end(file_query.len() as i32);
            }
            None => {}
        }
    }

    fn refresh_shortcuts(&self) {
        if self.state.borrow().active != Surface::Launcher {
            return;
        }
        let websites = self.settings.borrow()["websites"].clone();
        let features = self.rows.borrow();
        let (sites, plugins) = crate::launcher_shortcuts::project(&websites, &features);
        let mut images =
            crate::icons::saved_website_icons(&crate::config::profile_root(), &websites);
        for shortcut in &plugins {
            if let Some(image) = shortcut
                .icon_reference
                .as_deref()
                .and_then(crate::icons::from_data_url)
            {
                images.insert(shortcut.id.clone(), image);
            }
        }
        let render = |items: &[crate::launcher_shortcuts::Shortcut]| {
            items
                .iter()
                .map(|item| item.visual(images.get(&item.id).cloned()))
                .collect::<Vec<_>>()
        };
        self.launcher
            .set_website_shortcuts(ModelRc::new(VecModel::from(render(&sites))));
        self.launcher
            .set_plugin_shortcuts(ModelRc::new(VecModel::from(render(&plugins))));
        *self.website_targets.borrow_mut() = sites.into_iter().map(|site| site.target).collect();
        *self.plugin_targets.borrow_mut() =
            plugins.into_iter().map(|plugin| plugin.target).collect();
        *self.launcher_icons.borrow_mut() = images;
        if self.launcher.get_website_shortcuts().row_count() <= 6 {
            self.launcher.set_websites_expanded(false);
        }
    }

    fn query(&self, text: &str) {
        let generation = self.search_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let raw = text.trim();
        if raw
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("file:"))
        {
            self.launcher.set_file_search_mode(true);
            self.launcher.set_file_menu_visible(false);
            self.launcher.set_file_category_focused(false);
            self.launcher.set_file_engine_source("".into());
            self.file_hits.borrow_mut().clear();
            self.file_icon_pixels.borrow_mut().clear();
            self.file_category
                .set(crate::file_search::FileCategory::All);
            self.launcher.set_file_category_selected(0);
            let query = raw[5..].trim().to_string();
            self.launcher
                .set_rows(ModelRc::new(VecModel::from(Vec::<crate::VisualRow>::new())));
            self.targets.borrow_mut().clear();
            self.launcher.set_selected(-1);
            let settings = self.settings.borrow();
            if settings["everythingEnabled"] != true {
                self.launcher
                    .set_status("请先在 WebTools 设置中启用文件搜索。".into());
            } else if query.is_empty() {
                self.launcher
                    .set_status("输入关键词搜索文件和文件夹。".into());
            } else {
                self.launcher.set_status("正在搜索 Everything…".into());
                let path = settings["everythingEsPath"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                drop(settings);
                let events = self.events.clone();
                let current = self.search_generation.clone();
                self.workers.borrow_mut().push(std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(160));
                    if current.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    let result = crate::file_search::search_with_progress(
                        &path,
                        &query,
                        || current.load(Ordering::SeqCst) == generation,
                        |status| {
                            let _ = events.send(AppEvent::FileSearchStatus {
                                generation,
                                status: status.into(),
                            });
                        },
                    );
                    let icon_paths = result
                        .as_ref()
                        .map(|output| {
                            output
                                .hits
                                .iter()
                                .take(20)
                                .map(|hit| hit.path.clone())
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    if current.load(Ordering::SeqCst) == generation {
                        let _ = events.send(AppEvent::FileSearchComplete { generation, result });
                    }
                    if !icon_paths.is_empty() {
                        let mut icon_pixels = Vec::with_capacity(icon_paths.len());
                        for path in icon_paths {
                            if current.load(Ordering::SeqCst) != generation {
                                return;
                            }
                            icon_pixels.push(crate::icons::file_path_pixels(&path));
                        }
                        if current.load(Ordering::SeqCst) == generation {
                            let _ = events.send(AppEvent::FileSearchIconsComplete {
                                generation,
                                icon_pixels,
                            });
                        }
                    }
                }));
            }
            return;
        }
        self.launcher.set_file_search_mode(false);
        self.launcher.set_file_menu_visible(false);
        self.launcher.set_file_category_focused(false);
        self.launcher.set_file_engine_source("".into());
        self.file_hits.borrow_mut().clear();
        self.file_icon_pixels.borrow_mut().clear();
        let websites = self.settings.borrow()["websites"].clone();
        let settings = self.settings.borrow().clone();
        let features = self.rows.borrow().clone();
        let remembered_app = self.launcher_state.lock().unwrap().remembered(text);
        let (rows, targets) = visual::build(
            text,
            &websites,
            &features,
            &settings,
            &self.launcher_icons.borrow(),
            remembered_app.as_deref(),
        );
        *self.targets.borrow_mut() = targets;
        self.launcher
            .set_selected(if rows.is_empty() { -1 } else { 0 });
        self.launcher.set_rows(ModelRc::new(VecModel::from(rows)));
        self.launcher.set_status("".into());
    }

    fn present_file_hits(&self) {
        let category = self.file_category.get();
        let raw_query = self.launcher.get_query();
        let query = raw_query.as_str().get(5..).unwrap_or_default().trim();
        let mut rows = Vec::new();
        let mut targets = Vec::new();
        let hits = self.file_hits.borrow();
        let icon_pixels = self.file_icon_pixels.borrow();
        for (hit_index, hit) in hits.iter().enumerate() {
            let kind = crate::file_search::FileCategory::classify(
                &hit.path,
                std::path::Path::new(&hit.path).is_dir(),
            );
            if category != crate::file_search::FileCategory::All && category != kind {
                continue;
            }
            let icon = icon_pixels
                .get(hit_index)
                .and_then(Option::as_ref)
                .map(|(bytes, edge)| crate::icons::image_from_pixels(bytes, *edge));
            rows.push(crate::VisualRow {
                title: hit.name.as_str().into(),
                title_parts: visual::highlight_parts(&hit.name, query),
                subtitle: hit.location.as_str().into(),
                icon_index: kind.index(),
                has_icon: icon.is_some(),
                icon: icon.unwrap_or_default(),
                ..Default::default()
            });
            targets.push(visual::Target::FilePath(hit.path.clone()));
        }
        *self.targets.borrow_mut() = targets;
        self.launcher
            .set_selected(if rows.is_empty() { -1 } else { 0 });
        self.launcher.set_rows(ModelRc::new(VecModel::from(rows)));
        if self.launcher.get_rows().row_count() == 0 && !self.file_hits.borrow().is_empty() {
            self.launcher
                .set_status("此文件类型没有匹配的项目。".into());
        } else if self.file_hits.borrow().is_empty() {
            self.launcher
                .set_status("没有找到匹配的文件或文件夹。".into());
        } else {
            self.launcher.set_status("".into());
        }
    }
    fn hide(&self) {
        self.hide_surface(false);
    }
    fn hide_preserving_manager_session(&self) {
        self.hide_surface(true);
    }
    fn hide_surface(&self, preserve_manager_session: bool) {
        self.launcher_focus_pending.set(false);
        self.launcher.set_plugin_loading(false);
        self.search_generation.fetch_add(1, Ordering::SeqCst);
        visual::cancel_surface_window_show_hook();
        if let Some(cancelled) = self.capture_cancel.borrow_mut().take() {
            cancelled.store(true, Ordering::SeqCst);
        }
        self.state.borrow_mut().hide();
        native_input::observe_capture(false);
        native_input::observe_panel(0);
        self.cancel_panel_result();
        self.panel.borrow_mut().take();
        let _ = self.launcher.hide();
        // Only transient render/query data is cleared; persistent data belongs to state stores.
        self.launcher.set_query("".into());
        self.launcher.set_file_search_mode(false);
        self.launcher.set_file_menu_visible(false);
        self.launcher.set_file_category_focused(false);
        self.launcher.set_file_engine_source("".into());
        self.file_hits.borrow_mut().clear();
        self.file_icon_pixels.borrow_mut().clear();
        self.launcher
            .set_rows(ModelRc::new(VecModel::from(Vec::<crate::VisualRow>::new())));
        self.targets.borrow_mut().clear();
        self.website_targets.borrow_mut().clear();
        self.plugin_targets.borrow_mut().clear();
        self.launcher_icons.borrow_mut().clear();
        self.launcher.set_website_shortcuts(ModelRc::default());
        self.launcher.set_plugin_shortcuts(ModelRc::default());
        self.launcher.set_websites_expanded(false);
        if !preserve_manager_session {
            self.end_manager_session();
            self.pending_handoff.borrow_mut().take();
        }
        eprintln!("app: surface=hidden");
    }
    fn begin_manager_session(&self) -> String {
        if let Some(id) = self.active_session.borrow().clone() {
            return id;
        }
        let id = webtools_ipc::new_request_id();
        self.ownership.begin_session(&id);
        self.active_session.replace(Some(id.clone()));
        if let Some(runtime) = self.runtime.borrow().as_ref() {
            let _ = runtime.handle().send(HostCommand::SyncOwnership);
        }
        id
    }
    fn end_manager_session(&self) {
        let Some(id) = self.active_session.borrow_mut().take() else {
            return;
        };
        self.ownership.end_session(&id);
        if let Some(runtime) = self.runtime.borrow().as_ref() {
            let _ = runtime.handle().send(HostCommand::SyncOwnership);
        }
    }
    fn show_launcher(&self) -> bool {
        self.show_launcher_with_ack(None)
    }
    fn show_launcher_with_ack(
        &self,
        mut shown_reply: Option<std::sync::mpsc::SyncSender<bool>>,
    ) -> bool {
        let returning_from_temporary_window = shown_reply.is_some();
        let foreground = visual::current_foreground();
        if foreground != 0
            && foreground != crate::window_by_title("WebTools Super Panel")
            && foreground != crate::window_by_title("WebTools Launcher Replica")
        {
            self.external.set(foreground);
        }
        self.hide();
        self.launcher_focus_pending
            .set(returning_from_temporary_window);
        self.state.borrow_mut().launcher();
        self.launcher
            .set_expanded(self.settings.borrow()["launcherDisplayMode"] == "expanded");
        self.apply_theme();
        self.refresh_shortcuts();
        let launcher_hwnd = crate::window_by_title("WebTools Launcher Replica");
        if launcher_hwnd == 0 {
            let _ = visual::arm_surface_window_show_hook();
        } else {
            visual::disable_window_transitions(launcher_hwnd);
        }
        if let Err(error) = self.launcher.show() {
            eprintln!("app: Launcher show failed: {error}");
            self.launcher_focus_pending.set(false);
            self.state.borrow_mut().hide();
            if let Some(reply) = shown_reply.take() {
                let _ = reply.send(false);
            }
            return false;
        }
        self.begin_manager_session();
        if self.launcher.get_expanded() {
            self.ensure_manager();
        }
        let launcher_hwnd = crate::window_by_title("WebTools Launcher Replica");
        if launcher_hwnd != 0 && !visual::disable_window_transitions(launcher_hwnd) {
            eprintln!("app: Launcher window preparation failed hwnd=0x{launcher_hwnd:x}");
        }
        visual::place_launcher_now();
        let active = visual::restore_foreground(launcher_hwnd);
        if active {
            self.launcher.invoke_focus_input();
        }
        if !active && std::env::var("WT_FOCUS_TRACE").is_ok() {
            eprintln!("app: Launcher is visible but Windows did not activate it on first request");
        }
        // This acknowledgement means the Launcher surface was shown. Electron
        // closes the temporary window after this reply, then sends a second
        // focus request. Keep blur-to-hide suppressed until that handoff ends.
        if let Some(reply) = shown_reply.take() {
            let _ = reply.send(self.launcher.window().is_visible());
        }
        if !active && !returning_from_temporary_window {
            retry_launcher_focus(
                self.launcher.as_weak(),
                self.launcher_focus_pending.clone(),
                10,
            );
        }
        eprintln!("app: surface=launcher pid={}", std::process::id());
        true
    }
    fn refocus_launcher_after_handoff(&self) -> bool {
        if self.state.borrow().active != Surface::Launcher || !self.launcher.window().is_visible() {
            self.launcher_focus_pending.set(false);
            return false;
        }
        self.launcher_focus_pending.set(true);
        visual::place_launcher_now();
        let hwnd = crate::window_by_title("WebTools Launcher Replica");
        let active =
            hwnd != 0 && (visual::current_foreground() == hwnd || visual::restore_foreground(hwnd));
        if active {
            self.launcher_focus_pending.set(false);
            self.launcher.invoke_focus_input();
        } else {
            retry_launcher_focus(
                self.launcher.as_weak(),
                self.launcher_focus_pending.clone(),
                10,
            );
        }
        active
    }
    fn capture(&self) {
        if !self.panel_settings.borrow().enabled() {
            return;
        }
        *TRIGGER_AT.lock().unwrap() = Some(std::time::Instant::now());
        let toggle_off = {
            let state = self.state.borrow();
            state.active == Surface::Panel || state.is_pending()
        };
        if toggle_off {
            self.hide();
            return;
        }
        let panel_anchor = visual::cursor_position();
        let foreground = visual::current_foreground();
        let foreground = if foreground == crate::window_by_title("WebTools Launcher Replica") {
            self.external.get()
        } else {
            foreground
        };
        self.hide_preserving_manager_session();
        visual::restore_foreground(foreground);
        let generation = self.state.borrow_mut().capture();
        native_input::observe_capture(true);
        let capture_token = native_input::capture_token();
        let cancelled = Arc::new(AtomicBool::new(false));
        self.capture_cancel.replace(Some(cancelled.clone()));
        if !self.state.borrow_mut().show_pending_panel(generation) {
            self.hide();
            return;
        }
        let panel = match PanelController::new(
            String::new(),
            None,
            &self.rows.borrow(),
            self.events.clone(),
            self.panel_settings.clone(),
        ) {
            Ok(panel) => panel,
            Err(error) => {
                eprintln!("app: panel creation failed: {error}");
                self.hide();
                return;
            }
        };
        panel
            .ui
            .global::<crate::HostTheme>()
            .set_light(self.launcher.global::<crate::HostTheme>().get_light());
        let panel_hwnd = crate::window_by_title("WebTools Super Panel");
        if panel_hwnd == 0 {
            let _ = visual::arm_surface_window_show_hook();
        } else {
            visual::prepare_panel_window_before_show(panel_hwnd);
        }
        if panel.ui.show().is_err() {
            visual::cancel_surface_window_show_hook();
            self.hide();
            return;
        }
        let hwnd = crate::window_by_title("WebTools Super Panel");
        if !visual::disable_window_transitions(hwnd) {
            eprintln!("app: DWM transition disable failed for Super Panel hwnd=0x{hwnd:x}");
        }
        visual::prepare_panel(&panel.ui, panel_anchor);
        native_input::observe_panel(hwnd);
        if visual::current_foreground() == hwnd {
            visual::restore_foreground(foreground);
        }
        if timing() {
            if let Some(at) = *TRIGGER_AT.lock().unwrap() {
                eprintln!("timing: first-frame={:?}", at.elapsed());
            }
        }
        let weak = panel.ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(40), move || {
            if let Some(panel) = weak.upgrade() {
                if panel.window().is_visible() {
                    let hwnd = crate::window_by_title("WebTools Super Panel");
                    if hwnd != 0 {
                        visual::disable_window_transitions(hwnd);
                    }
                    visual::prepare_panel(&panel, panel_anchor);
                    native_input::observe_panel(hwnd);
                    if visual::current_foreground() == hwnd {
                        visual::restore_foreground(foreground);
                    }
                }
            }
        });
        self.panel.replace(Some(panel));
        let events = self.events.clone();
        self.workers.borrow_mut().push(std::thread::spawn(move || {
            let fixture = std::env::var("WT_PANEL_FAKE_SELECTION").ok();
            let current = || {
                !cancelled.load(Ordering::SeqCst)
                    && native_input::capture_current(capture_token)
                    && (fixture.is_some() || visual::current_foreground() == foreground)
            };
            if !current() {
                let _ = events.send(AppEvent::CaptureAborted(generation));
                return;
            }
            // Diagnostic fixture bypasses foreign application input entirely.
            let (text, context) = if let Some(text) = fixture.as_ref() {
                (text.clone(), None)
            } else {
                let shell = crate::shell_context::capture();
                if shell.is_some() {
                    (String::new(), shell)
                } else {
                    (
                        visual::capture_selection_while(current).unwrap_or_default(),
                        None,
                    )
                }
            };
            if !current() {
                let _ = events.send(AppEvent::CaptureAborted(generation));
                return;
            }
            if timing() {
                if let Some(at) = *TRIGGER_AT.lock().unwrap() {
                    eprintln!(
                        "timing: capture={:?} text={} shell={}",
                        at.elapsed(),
                        text.chars().count() > 0,
                        context.is_some()
                    );
                }
            }
            let _ = events.send(AppEvent::Captured {
                generation,
                text,
                context,
                foreground,
                anchor: panel_anchor,
            });
        }));
        self.begin_manager_session();
        self.ensure_manager();
        eprintln!("app: surface=panel pid={} hwnd={hwnd}", std::process::id());
    }
    fn present(
        &self,
        generation: u64,
        text: String,
        context: Option<ShellContext>,
        foreground: isize,
        anchor: Option<(i32, i32)>,
    ) {
        if std::env::var_os("WT_PANEL_FAKE_SELECTION").is_none()
            && visual::current_foreground() != foreground
        {
            let cancelled = self.state.borrow_mut().cancel(generation);
            if cancelled {
                self.hide();
            }
            return;
        }
        if !self.state.borrow_mut().present(generation) {
            return;
        }
        let preview_text = text.clone();
        // WPF starts automatic translation only for a text-selection surface.
        // Do not reuse text captured alongside an Explorer file/folder context.
        let selected_text_context = context.is_none();
        native_input::observe_capture(false);
        let mut panel_slot = self.panel.borrow_mut();
        let Some(panel) = panel_slot.as_mut() else {
            drop(panel_slot);
            self.hide();
            return;
        };
        panel.set_context(text, context, &self.rows.borrow());
        let hwnd = crate::window_by_title("WebTools Super Panel");
        if timing() {
            if let Some(at) = *TRIGGER_AT.lock().unwrap() {
                eprintln!("timing: context-ready={:?}", at.elapsed());
            }
        }
        visual::prepare_panel(&panel.ui, anchor);
        let weak = panel.ui.as_weak();
        slint::Timer::single_shot(Duration::from_millis(40), move || {
            if let Some(panel) = weak.upgrade() {
                if panel.window().is_visible() {
                    let hwnd = crate::window_by_title("WebTools Super Panel");
                    if hwnd != 0 {
                        visual::disable_window_transitions(hwnd);
                    }
                    visual::prepare_panel(&panel, anchor);
                    native_input::observe_panel(hwnd);
                    eprintln!("app: panel-mapped hwnd={hwnd}");
                    if visual::current_foreground() == hwnd {
                        visual::restore_foreground(foreground);
                    }
                }
            }
        });
        drop(panel_slot);
        if selected_text_context {
            self.start_translation_preview(&preview_text);
        }
        eprintln!("app: panel-context-ready hwnd={hwnd}");
    }
    fn cancel_pending_capture_after_interaction(&self) {
        if !self.state.borrow().is_pending() {
            return;
        }
        if let Some(cancelled) = self.capture_cancel.borrow_mut().take() {
            cancelled.store(true, Ordering::SeqCst);
        }
        native_input::observe_capture(false);
        self.state.borrow_mut().cancel_pending();
    }
    fn ensure_manager(&self) {
        let finished = self
            .runtime
            .borrow()
            .as_ref()
            .is_some_and(HostRuntime::is_finished);
        if finished {
            if let Some(runtime) = self.runtime.borrow_mut().take() {
                runtime.shutdown();
            }
            self.host_events.borrow_mut().take();
            eprintln!("app: replacing finished Manager runtime");
        }
        if self.runtime.borrow().is_some() {
            return;
        }
        if !self.ownership.has_owners() {
            return;
        }
        let Some(exe) = self.config.manager_exe() else {
            return;
        };
        let args = self
            .config
            .manager_args
            .clone()
            .unwrap_or_else(|| crate::config::default_manager_args(crate::config::production_mode()));
        // The experimental host must not start a Manager against a production profile.
        if !crate::config::production_mode() && !args.iter().any(|arg| arg == "--phase4g-manager-test") {
            eprintln!(
                "app: Manager configuration needs --phase4g-manager-test for isolated development"
            );
            return;
        }
        let (events, receiver) = mpsc::channel();
        let runtime = HostRuntime::start(
            exe,
            args,
            Duration::from_secs(self.config.idle_trim_seconds.unwrap_or(30)),
            events,
            self.ownership.clone(),
            self.launcher_state.clone(),
            self.projection_cache.clone(),
        );
        self.host_events.replace(Some(receiver));
        self.runtime.replace(Some(runtime));
        eprintln!("app: manager-owner=session-scoped");
    }
    fn command(&self, command: HostCommand) -> bool {
        let temporary_session = if self.active_session.borrow().is_none() {
            Some(self.begin_manager_session())
        } else {
            None
        };
        let command = command.with_request_ownership(temporary_session.clone());
        self.ensure_manager();
        if let Some(runtime) = self.runtime.borrow().as_ref() {
            match runtime.handle().send(command) {
                Ok(()) => true,
                Err(_) => {
                    if temporary_session.is_some() {
                        self.end_manager_session();
                    }
                    self.launcher.set_status("管理端未就绪".into());
                    false
                }
            }
        } else {
            if temporary_session.is_some() {
                self.end_manager_session();
            }
            self.launcher.set_status("管理端未就绪".into());
            false
        }
    }
    fn open_manager_page(&self, section: &'static str) {
        self.open_manager_target(HostCommand::open_page(section));
    }
    fn open_translation(&self, text: String) {
        self.open_manager_target(HostCommand::translation(text, "launcher"));
    }
    fn open_manager_target(&self, command: HostCommand) {
        let source = self.state.borrow().active;
        if source == Surface::Launcher {
            self.launcher.set_plugin_loading(true);
        }
        let session = self.begin_manager_session();
        let command = command.with_request_ownership(Some(session.clone()));
        let request_id = command.request_id().unwrap_or_default().to_owned();
        if matches!(source, Surface::Launcher | Surface::Panel) && !request_id.is_empty() {
            self.pending_handoff.replace(Some(PendingWindowHandoff {
                request_id,
                session_id: session,
                source,
            }));
            match source {
                Surface::Launcher => self.launcher.set_status("正在打开窗口…".into()),
                Surface::Panel => {
                    if let Some(panel) = self.panel.borrow().as_ref() {
                        panel.ui.set_status_text("正在打开窗口…".into());
                    }
                }
                Surface::Hidden => {}
            }
        }
        self.ensure_manager();
        let sent = self
            .runtime
            .borrow()
            .as_ref()
            .is_some_and(|runtime| runtime.handle().send(command).is_ok());
        if sent {
            // Keep the source visible until Electron reports that its target
            // HWND is ready. The request owns this session through that reply.
        } else {
            self.pending_handoff.borrow_mut().take();
            if source == Surface::Launcher {
                self.launcher.set_plugin_loading(false);
            }
            self.end_manager_session();
            match source {
                Surface::Launcher => self.launcher.set_status("管理端未就绪".into()),
                Surface::Panel => {
                    if let Some(panel) = self.panel.borrow().as_ref() {
                        panel.ui.set_status_text("管理端未就绪".into());
                    }
                }
                Surface::Hidden => {}
            }
        }
    }

    fn is_command_feature(&self, plugin: &str, feature: &str) -> bool {
        self.rows.borrow().iter().any(|row| {
            row.plugin_id == plugin
                && row.feature_id == feature
                && row.presentation.as_deref() == Some("command")
        })
    }

    fn complete_window_handoff(&self, request_id: &str, handle: u64, process_id: u32) -> bool {
        let Some(pending) = self
            .pending_handoff
            .borrow()
            .as_ref()
            .filter(|pending| pending.request_id == request_id)
            .cloned()
        else {
            return false;
        };
        let source = match pending.source {
            Surface::Launcher => crate::window_by_title("WebTools Launcher Replica"),
            Surface::Panel => crate::window_by_title("WebTools Super Panel"),
            Surface::Hidden => 0,
        };
        if !visual::show_target_and_hide_surface(handle, process_id, source) {
            if pending.source == Surface::Launcher {
                self.launcher.set_plugin_loading(false);
            }
            match pending.source {
                Surface::Launcher => self
                    .launcher
                    .set_status("插件窗口交接失败，请重试。".into()),
                Surface::Panel => {
                    if let Some(panel) = self.panel.borrow().as_ref() {
                        panel
                            .ui
                            .set_status_text("插件窗口交接失败，请重试。".into());
                    }
                }
                Surface::Hidden => {}
            }
            return false;
        }
        if self
            .pending_handoff
            .borrow()
            .as_ref()
            .is_some_and(|current| current.request_id == request_id)
        {
            self.pending_handoff.borrow_mut().take();
        }
        self.hide_surface(true);
        true
    }

    fn toggle_launcher_or_temporary_window(&self) {
        if self.state.borrow().active == Surface::Launcher {
            self.hide();
            return;
        }
        if self.focus_request_pending.get() {
            let connected = self
                .runtime
                .borrow()
                .as_ref()
                .is_some_and(HostRuntime::is_connected);
            if connected {
                return;
            }
            self.focus_request_pending.set(false);
        }
        let sent = self.runtime.borrow().as_ref().is_some_and(|runtime| {
            if !runtime.is_connected() {
                return false;
            }
            self.focus_request_pending.set(true);
            if runtime.toggle_current_temporary_window() {
                true
            } else {
                self.focus_request_pending.set(false);
                false
            }
        });
        if !sent {
            self.show_launcher();
        }
    }
    fn fallback_pending_focus_to_launcher(&self) {
        if !self.focus_request_pending.replace(false) {
            return;
        }
        let should_show = self.state.borrow().active == Surface::Hidden
            && !self.state.borrow().is_pending();
        if should_show {
            self.show_launcher();
        }
    }
    fn cancel_panel_result(&self) {
        let binding = self.panel_result.borrow_mut().take();
        if let Some(panel) = self.panel.borrow_mut().as_mut() {
            panel.clear_confirmation();
        }
        let Some(binding) = binding.filter(|binding| binding.cancellable) else {
            return;
        };
        let session_id = webtools_ipc::new_request_id();
        self.ownership.begin_session(&session_id);
        if let Some(runtime) = self.runtime.borrow().as_ref() {
            let tx = runtime.handle();
            if tx.send(HostCommand::SyncOwnership).is_ok()
                && tx
                    .send(HostCommand::CancelResult {
                        request_id: binding.request_id,
                        plugin: binding.plugin,
                        feature: binding.feature,
                        session_id: session_id.clone(),
                    })
                    .is_ok()
            {
                return;
            }
        }
        self.ownership.end_session(&session_id);
    }
    fn send_ai_consent_decision(
        &self,
        confirmation: webtools_ipc::quick_result::AIConfirmation,
        approved: bool,
    ) {
        if let Some(runtime) = self.runtime.borrow().as_ref() {
            let _ = runtime.handle().send(HostCommand::AIConsentDecision {
                confirmation,
                approved,
            });
        }
    }
    fn start_translation_preview(&self, text: &str) {
        if !self.panel_settings.borrow().auto_translation_preview()
            || !is_translation_preview_candidate(text)
        {
            return;
        }
        let request_id = webtools_ipc::new_request_id();
        let plugin = "webtools.translation".to_string();
        let feature = "translate-text".to_string();
        self.panel_result.replace(Some(PanelResultBinding {
            request_id: request_id.clone(),
            plugin: plugin.clone(),
            feature: feature.clone(),
            cancellable: true,
        }));
        if let Some(panel) = self.panel.borrow().as_ref() {
            panel.ui.set_result_source("翻译预览".into());
            panel.ui.set_result_body("正在翻译…".into());
            panel.ui.set_result_cancel_visible(true);
            panel.ui.set_result_visible(true);
        }
        let command = HostCommand::preview_translation(text.to_string(), request_id);
        self.ensure_manager();
        let sent = self
            .runtime
            .borrow()
            .as_ref()
            .is_some_and(|runtime| runtime.handle().send(command).is_ok());
        if !sent {
            self.panel_result.borrow_mut().take();
            if let Some(panel) = self.panel.borrow().as_ref() {
                panel.close_result();
                panel
                    .ui
                    .set_notice("翻译预览暂不可用，请重新唤出面板。".into());
            }
        }
    }
    fn escape(&self) {
        let consent_visible = self
            .panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| panel.ui.get_consent_visible());
        if consent_visible {
            if let Some(confirmation) = self
                .panel
                .borrow_mut()
                .as_mut()
                .and_then(PanelController::take_confirmation)
            {
                self.send_ai_consent_decision(confirmation, false);
            }
            return;
        }
        let result_visible = self
            .panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| panel.ui.get_result_visible());
        if result_visible {
            if let Some(panel) = self.panel.borrow().as_ref() {
                panel.close_result();
            }
            self.cancel_panel_result();
            return;
        }
        let chooser_visible = self
            .panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| panel.ui.get_chooser_visible());
        if chooser_visible {
            if let Some(panel) = self.panel.borrow().as_ref() {
                panel.ui.set_chooser_visible(false);
            }
            return;
        }
        self.hide();
    }
    fn event(&self, event: AppEvent) {
        match event {
            AppEvent::ShowLauncher => {
                self.show_launcher();
            }
            AppEvent::ToggleLauncher => self.toggle_launcher_or_temporary_window(),
            AppEvent::TogglePanel => self.capture(),
            AppEvent::MousePanel(version) => {
                if version == native_input::panel_version() {
                    self.capture();
                }
            }
            AppEvent::TrayPanel => {
                if self.panel_settings.borrow().enabled() {
                    self.capture();
                } else {
                    self.command(HostCommand::open_page("settings"));
                }
            }
            AppEvent::TraySettings => self.open_manager_page("settings"),
            AppEvent::AppCatalogReady => {
                let query = self.launcher.get_query();
                if !query.is_empty() && !self.launcher.get_file_search_mode() {
                    self.query(&query);
                }
            }
            AppEvent::Dismiss => self.hide(),
            AppEvent::Escape => self.escape(),
            AppEvent::Quit => {
                self.hide();
                crate::file_search::shutdown();
                let _ = slint::quit_event_loop();
            }
            AppEvent::Captured {
                generation,
                text,
                context,
                foreground,
                anchor,
            } => self.present(generation, text, context, foreground, anchor),
            AppEvent::CaptureAborted(generation) => {
                let cancelled = self.state.borrow_mut().cancel(generation);
                if cancelled {
                    self.hide();
                }
            }
            AppEvent::Action(i) | AppEvent::Board(i) => {
                self.cancel_pending_capture_after_interaction();
                let is_board = self
                    .panel
                    .borrow()
                    .as_ref()
                    .is_some_and(|p| p.ui.get_mode() == "board");
                let action = self
                    .panel
                    .borrow()
                    .as_ref()
                    .and_then(|p| p.action(i, is_board));
                if let Some((plugin, feature, selection)) = action {
                    if plugin == "webtools.host"
                        && selection.as_ref().is_some_and(|selection| {
                            selection.kind == webtools_ipc::projection::ContextKind::Text
                        })
                    {
                        let text = selection
                            .as_ref()
                            .and_then(|selection| selection.text.as_deref())
                            .unwrap_or_default();
                        let opened = match feature.as_str() {
                            "search-selected-text" => {
                                let settings = self.settings.borrow();
                                let engine_id = settings["defaultSearchEngineId"]
                                    .as_str()
                                    .unwrap_or("google");
                                let template = settings["searchEngines"]
                                    .as_array()
                                    .and_then(|engines| {
                                        engines.iter().find(|engine| {
                                            engine["id"] == engine_id && engine["enabled"] == true
                                        })
                                    })
                                    .or_else(|| {
                                        settings["searchEngines"].as_array().and_then(|engines| {
                                            engines.iter().find(|engine| engine["enabled"] == true)
                                        })
                                    })
                                    .and_then(|engine| engine["template"].as_str())
                                    .unwrap_or("https://www.google.com/search?q=%s");
                                crate::shell::open_search_template(template, text)
                            }
                            "open-selected-link" => crate::shell::open_url(text.trim()),
                            _ => false,
                        };
                        if opened {
                            self.hide();
                        } else if let Some(panel) = self.panel.borrow().as_ref() {
                            panel.ui.set_status_text("无法打开所选内容。".into());
                        }
                    } else if plugin == "webtools.translation" && feature == "translate-text" {
                        let selected_text = selection
                            .as_ref()
                            .filter(|selection| {
                                selection.kind == webtools_ipc::projection::ContextKind::Text
                            })
                            .and_then(|selection| selection.text.clone());
                        self.cancel_panel_result();
                        let command = selected_text.map_or_else(
                            || HostCommand::invoke(plugin, feature, selection),
                            |text| HostCommand::translation(text, "super-panel"),
                        );
                        self.open_manager_target(command);
                    } else if let Some(selection) = selection {
                        let command_feature = self.is_command_feature(&plugin, &feature);
                        if selection.kind == webtools_ipc::projection::ContextKind::Text
                            && command_feature
                        {
                            let request_id = webtools_ipc::new_request_id();
                            self.cancel_panel_result();
                            self.panel_result.replace(Some(PanelResultBinding {
                                request_id: request_id.clone(),
                                plugin: plugin.clone(),
                                feature: feature.clone(),
                                cancellable: true,
                            }));
                            self.command(HostCommand::invoke_with_request_id(
                                plugin,
                                feature,
                                Some(selection),
                                request_id,
                            ));
                        } else {
                            let command = HostCommand::invoke(plugin, feature, Some(selection));
                            if command_feature {
                                self.command(command);
                            } else {
                                self.open_manager_target(command);
                            }
                        }
                    } else {
                        let command_feature = self.is_command_feature(&plugin, &feature);
                        let command = HostCommand::invoke(plugin, feature, None);
                        if command_feature {
                            self.command(command);
                        } else {
                            self.open_manager_target(command);
                        }
                    }
                }
            }
            AppEvent::Page(delta) => {
                self.cancel_pending_capture_after_interaction();
                if let Some(p) = self.panel.borrow_mut().as_mut() {
                    p.page(delta, &self.rows.borrow());
                }
            }
            AppEvent::BoardContextAction(index, action) => {
                self.cancel_pending_capture_after_interaction();
                if let Some(panel) = self.panel.borrow_mut().as_mut() {
                    panel.board_context_action(index, action, &self.rows.borrow());
                }
            }
            AppEvent::BoardReorder(from, to) => {
                self.cancel_pending_capture_after_interaction();
                if let Some(panel) = self.panel.borrow_mut().as_mut() {
                    panel.reorder_board(from, to, &self.rows.borrow());
                }
            }
            AppEvent::Add => {
                self.cancel_pending_capture_after_interaction();
                if let Some(p) = self.panel.borrow_mut().as_mut() {
                    p.add(&self.rows.borrow());
                }
            }
            AppEvent::Pin(i) => {
                self.cancel_pending_capture_after_interaction();
                if let Some(p) = self.panel.borrow_mut().as_mut() {
                    p.pin(i, &self.rows.borrow());
                }
            }
            AppEvent::CloseResult => {
                if let Some(p) = self.panel.borrow().as_ref() {
                    p.close_result();
                }
                self.cancel_panel_result();
            }
            AppEvent::AIConsentDecision(approved) => {
                let confirmation = self
                    .panel
                    .borrow_mut()
                    .as_mut()
                    .and_then(PanelController::take_confirmation);
                if let Some(confirmation) = confirmation {
                    self.send_ai_consent_decision(confirmation, approved);
                    if let Some(panel) = self.panel.borrow().as_ref() {
                        panel.ui.set_status_text(
                            if approved {
                                "正在执行…"
                            } else {
                                "已取消发送。"
                            }
                            .into(),
                        );
                    }
                }
            }
            AppEvent::SystemThemeChanged => {
                if self.settings.borrow()["theme"] == "system" {
                    self.apply_theme();
                }
            }
            AppEvent::FileSearchComplete { generation, result } => {
                if self.search_generation.load(Ordering::SeqCst) != generation
                    || self.state.borrow().active != Surface::Launcher
                {
                    return;
                }
                match result {
                    Ok(output) => {
                        let hit_count = output.hits.len();
                        *self.file_hits.borrow_mut() = output.hits;
                        *self.file_icon_pixels.borrow_mut() =
                            std::iter::repeat_with(|| None).take(hit_count).collect();
                        self.launcher
                            .set_file_engine_source(output.engine_source.into());
                        self.present_file_hits();
                    }
                    Err(error) => {
                        self.file_hits.borrow_mut().clear();
                        self.file_icon_pixels.borrow_mut().clear();
                        self.targets.borrow_mut().clear();
                        self.launcher
                            .set_rows(ModelRc::new(VecModel::from(Vec::<crate::VisualRow>::new())));
                        self.launcher.set_status(error.into());
                    }
                }
            }
            AppEvent::FileSearchIconsComplete {
                generation,
                icon_pixels,
            } => {
                if self.search_generation.load(Ordering::SeqCst) != generation
                    || self.state.borrow().active != Surface::Launcher
                {
                    return;
                }
                let mut current = self.file_icon_pixels.borrow_mut();
                for (slot, icon) in current.iter_mut().zip(icon_pixels) {
                    *slot = icon;
                }
                drop(current);
                self.present_file_hits();
            }
            AppEvent::FileSearchStatus { generation, status } => {
                if self.search_generation.load(Ordering::SeqCst) == generation
                    && self.state.borrow().active == Surface::Launcher
                {
                    self.launcher.set_status(status.into());
                }
            }
        }
    }
    fn pump(&self) {
        while let Ok(event) = self.receiver.try_recv() {
            self.event(event);
        }
        if let Some(events) = self.host_events.borrow().as_ref() {
            while let Ok(event) = events.try_recv() {
                if let Some(notice) = manager_connection_notice(&event) {
                    self.launcher.set_connection_status(notice.into());
                }
                match event {
                    HostEvent::Settings {
                        phase,
                        change,
                        reply,
                    } => {
                        let _ = reply.send(self.update_native_settings(phase, &change));
                    }
                    HostEvent::PanelSettings { operation, reply } => {
                        let _ = reply.send(self.update_panel_settings(operation));
                    }
                    HostEvent::SurfaceRequest { request, reply } => {
                        match request {
                            webtools_ipc::session::SurfaceRequest::ShowEmptyLauncher => {
                                self.show_launcher_with_ack(Some(reply));
                            }
                            webtools_ipc::session::SurfaceRequest::FocusLauncher => {
                                let _ = reply.send(self.refocus_launcher_after_handoff());
                            }
                            webtools_ipc::session::SurfaceRequest::WindowPresented {
                                request_id,
                                window_handle,
                                process_id,
                            } => {
                                let result = self.complete_window_handoff(&request_id, window_handle, process_id);
                                let _ = reply.send(result);
                            }
                        }
                    }
                    HostEvent::TemporaryWindowToggleHandled(handled) => {
                        self.focus_request_pending.set(false);
                        let show_launcher = !handled
                            && self.state.borrow().active == Surface::Hidden
                            && !self.state.borrow().is_pending();
                        if show_launcher {
                            self.show_launcher();
                        }
                    }
                    HostEvent::Features(rows) => {
                        *self.rows.borrow_mut() = rows;
                        if let Some(panel) = self.panel.borrow_mut().as_mut() {
                            panel.refresh(&self.rows.borrow());
                        }
                        self.refresh_shortcuts();
                        let query = self.launcher.get_query().to_string();
                        if !query.is_empty() {
                            self.query(&query);
                        }
                    }
                    HostEvent::Websites(websites) => {
                        self.settings.borrow_mut()["websites"] = websites;
                        self.refresh_shortcuts();
                        let query = self.launcher.get_query().to_string();
                        if !query.is_empty() {
                            self.query(&query);
                        }
                    }
                    HostEvent::Result {
                        request_id,
                        plugin,
                        feature,
                        text,
                    } => {
                        let mut binding = self.panel_result.borrow_mut();
                        if let Some(current) = binding.as_mut().filter(|current| {
                            current.request_id == request_id
                                && current.plugin == plugin
                                && current.feature == feature
                        }) {
                            current.cancellable = false;
                            if let Some(panel) = self.panel.borrow_mut().as_mut() {
                                panel.clear_confirmation();
                                panel.ui.set_result_source(
                                    if plugin == "webtools.translation"
                                        && feature == "translate-text"
                                    {
                                        "翻译预览".into()
                                    } else {
                                        format!("{plugin} · {feature}").into()
                                    },
                                );
                                panel.ui.set_result_body(text.into());
                                panel.ui.set_result_cancel_visible(false);
                                panel.ui.set_result_visible(true);
                            }
                        }
                    }
                    HostEvent::AIConfirmation(confirmation) => {
                        let current = self.panel_result.borrow().as_ref().is_some_and(|binding| {
                            binding.request_id == confirmation.request_id
                                && binding.plugin == confirmation.plugin_id
                                && binding.feature == confirmation.feature_id
                        });
                        if current {
                            if let Some(panel) = self.panel.borrow_mut().as_mut() {
                                panel.show_confirmation(confirmation);
                            }
                        } else {
                            self.send_ai_consent_decision(confirmation, false);
                        }
                    }
                    HostEvent::SessionEnded(id) => {
                        if self.active_session.borrow().as_deref() == Some(id.as_str()) {
                            self.active_session.borrow_mut().take();
                        }
                        let abandoned = self
                            .pending_handoff
                            .borrow()
                            .as_ref()
                            .filter(|pending| pending.session_id == id)
                            .cloned();
                        if let Some(pending) = abandoned {
                            self.pending_handoff.borrow_mut().take();
                            if pending.source == Surface::Launcher {
                                self.launcher.set_plugin_loading(false);
                            }
                            if self.state.borrow().active == pending.source {
                                match pending.source {
                                    Surface::Launcher => {
                                        self.launcher.set_status("窗口未能打开，请重试。".into())
                                    }
                                    Surface::Panel => {
                                        if let Some(panel) = self.panel.borrow().as_ref() {
                                            panel
                                                .ui
                                                .set_status_text("窗口未能打开，请重试。".into());
                                        }
                                    }
                                    Surface::Hidden => {}
                                }
                            }
                        }
                    }
                    HostEvent::PreviewFinished {
                        request_id,
                        status,
                        reason,
                    } => {
                        if matches!(status.as_str(), "failed" | "rejected" | "cancelled") {
                            let mut binding = self.panel_result.borrow_mut();
                            if let Some(current) = binding
                                .as_mut()
                                .filter(|current| current.request_id == request_id)
                            {
                                current.cancellable = false;
                                if let Some(panel) = self.panel.borrow().as_ref() {
                                    panel.ui.set_result_cancel_visible(false);
                                    panel.ui.set_result_body(format!("翻译预览未完成（{reason}），可打开翻译页面查看服务状态。").into());
                                }
                            }
                        }
                    }
                    HostEvent::Reconnecting { attempt } => {
                        eprintln!("app: Manager reconnect attempt={attempt}");
                        self.fallback_pending_focus_to_launcher();
                    }
                    HostEvent::Connected => {
                        eprintln!("app: Manager connection established");
                    }
                    HostEvent::Disconnected(reason) => {
                        eprintln!("app: Manager disconnected {reason}");
                        self.fallback_pending_focus_to_launcher();
                    }
                }
            }
        }
        let panel_closed = self
            .panel
            .borrow()
            .as_ref()
            .is_some_and(|panel| !panel.ui.window().is_visible());
        if panel_closed {
            self.hide();
        }
        let mut workers = self.workers.borrow_mut();
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
    }
}

fn retry_launcher_focus(
    launcher: slint::Weak<Baseline>,
    handoff_pending: Rc<Cell<bool>>,
    attempts_left: u8,
) {
    slint::Timer::single_shot(Duration::from_millis(20), move || {
        let Some(launcher) = launcher.upgrade() else {
            handoff_pending.set(false);
            return;
        };
        if !launcher.window().is_visible() {
            if attempts_left > 1 {
                retry_launcher_focus(
                    launcher.as_weak(),
                    handoff_pending.clone(),
                    attempts_left - 1,
                );
            } else {
                handoff_pending.set(false);
            }
            return;
        }

        visual::place_launcher_now();
        let hwnd = crate::window_by_title("WebTools Launcher Replica");
        let active = hwnd != 0
            && (visual::current_foreground() == hwnd || visual::restore_foreground(hwnd));
        if active {
            launcher.invoke_focus_input();
            handoff_pending.set(false);
            return;
        }

        if attempts_left > 1 {
            retry_launcher_focus(
                launcher.as_weak(),
                handoff_pending.clone(),
                attempts_left - 1,
            );
        } else {
            handoff_pending.set(false);
        }
    });
}

pub fn run(panel_first: bool) -> Result<(), Box<dyn std::error::Error>> {
    let Some(_single_instance) = SingleInstanceGuard::acquire()? else {
        eprintln!("app: another Slint NativeHost instance is already running");
        return Ok(());
    };
    let profile_root = crate::config::profile_root();
    let launcher_state = Arc::new(std::sync::Mutex::new(
        webtools_ipc::state::LauncherStateStore::load(&profile_root)?,
    ));
    if crate::startup::available() {
        if let Ok(enabled) = crate::startup::enabled() {
            let mut state = launcher_state.lock().unwrap();
            if state.snapshot()["launchOnStartup"].as_bool() != Some(enabled) {
                state.apply_settings(&serde_json::json!({"launchOnStartup": enabled}))?;
            }
        }
    }
    let settings = launcher_state.lock().unwrap().snapshot();
    let projection_cache = webtools_ipc::projection_cache::ProjectionCache::new(&profile_root);
    let cached_projection = projection_cache.load();
    let initial_rows = cached_projection
        .projection
        .map(|projection| projection.features)
        .unwrap_or_default();
    eprintln!(
        "app: feature-projection-cache={:?} rows={}",
        cached_projection.status,
        initial_rows.len()
    );
    let launcher = Baseline::new()?;
    launcher.set_fixture_icons(false);
    let (events, receiver) = mpsc::channel();
    let catalog_events = events.clone();
    let index_profile = profile_root.clone();
    std::thread::spawn(move || {
        crate::index::load_and_refresh(&index_profile, || {
            let _ = catalog_events.send(AppEvent::AppCatalogReady);
        });
    });
    let app = Rc::new(NativeApp {
        launcher,
        state: RefCell::default(),
        panel: RefCell::new(None),
        rows: RefCell::new(initial_rows),
        projection_cache,
        targets: RefCell::new(Vec::new()),
        website_targets: RefCell::new(Vec::new()),
        plugin_targets: RefCell::new(Vec::new()),
        launcher_icons: RefCell::new(std::collections::HashMap::new()),
        config: crate::config::load(),
        runtime: RefCell::new(None),
        ownership: webtools_ipc::HostOwnership::default(),
        active_session: RefCell::new(None),
        pending_handoff: RefCell::new(None),
        focus_request_pending: Cell::new(false),
        launcher_focus_pending: Rc::new(Cell::new(false)),
        panel_result: RefCell::new(None),
        host_events: RefCell::new(None),
        events: events.clone(),
        receiver,
        external: Cell::new(visual::current_foreground()),
        workers: RefCell::new(Vec::new()),
        capture_cancel: RefCell::new(None),
        settings: RefCell::new(settings.clone()),
        launcher_state,
        input: RefCell::new(None),
        panel_settings: Rc::new(RefCell::new(PanelSettingsController::load(
            &crate::config::profile_root(),
        ))),
        search_generation: Arc::new(AtomicU64::new(0)),
        file_hits: RefCell::new(Vec::new()),
        file_icon_pixels: RefCell::new(Vec::new()),
        file_category: Cell::new(crate::file_search::FileCategory::All),
    });
    app.launcher
        .set_file_categories(ModelRc::new(VecModel::from(
            crate::file_search::FileCategory::ALL
                .iter()
                .map(|category| category.label().into())
                .collect::<Vec<slint::SharedString>>(),
        )));
    app.wire();
    let input = native_input::NativeInput::start(
        events.clone(),
        settings["quickSearchShortcut"].as_str().unwrap().into(),
    );
    app.input.replace(Some(input.handle()));
    app.panel_settings
        .borrow_mut()
        .initialize(|s| input.handle().configure_panel(s));
    eprintln!("panel: settings {:?}", app.panel_settings.borrow().status());
    app.apply_theme();
    let theme_observer = crate::settings_ui::ThemeObserver::install();
    let tray = crate::tray::install(events);
    if panel_first {
        app.capture();
    } else {
        app.show_launcher();
    }
    let weak = Rc::downgrade(&app);
    let timer = slint::Timer::default();
    let mut was_active = false;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(25),
        move || {
            let Some(app) = weak.upgrade() else { return };
            app.pump();
            let active = app.state.borrow().active == Surface::Launcher
                && visual::current_foreground()
                    == crate::window_by_title("WebTools Launcher Replica");
            if was_active
                && !active
                && app.state.borrow().active == Surface::Launcher
                && !app.launcher_focus_pending.get()
            {
                app.hide();
            }
            was_active = active;
        },
    );
    if std::env::var("WT_APP_SMOKE").as_deref() == Ok("1") {
        smoke(app.clone());
    }
    #[cfg(test)]
    let settings_driver =
        std::env::var_os("WT_SETTINGS_TEST_CHILD").map(|_| settings_tests::drive(app.clone()));
    #[cfg(test)]
    let panel_driver =
        std::env::var_os("WT_PANEL_SETTINGS_TEST_CHILD").map(|_| panel_tests::drive(app.clone()));
    slint::run_event_loop_until_quit()?;
    #[cfg(test)]
    if let Some(driver) = settings_driver {
        driver.finish();
    }
    #[cfg(test)]
    if let Some(driver) = panel_driver {
        driver.finish();
    }
    timer.stop();
    drop(tray);
    drop(theme_observer);
    drop(input);
    app.hide();
    crate::file_search::shutdown();
    if let Some(runtime) = app.runtime.borrow_mut().take() {
        runtime.shutdown();
    }
    for worker in app.workers.borrow_mut().drain(..) {
        // A pending UAC dialog must not block host exit. Its helper checks this
        // host before starting and owns its engine in a kill-on-close job.
        if worker.is_finished() || !crate::file_search::startup_pending() {
            let _ = worker.join();
        }
    }
    eprintln!("app: shutdown complete");
    Ok(())
}
fn smoke(app: Rc<NativeApp>) {
    // Drives the normal controller and actual HWNDs, not a second fixture implementation.
    for (at, step) in [
        (400, 0),
        (900, 1),
        (1500, 2),
        (1900, 3),
        (2250, 4),
        (2600, 5),
    ] {
        let app = app.clone();
        slint::Timer::single_shot(Duration::from_millis(at), move || match step {
            0 => {
                assert!(app.launcher.window().is_visible());
                app.capture();
            }
            1 => {
                assert!(!app.launcher.window().is_visible());
                let panel = app.panel.borrow();
                let panel = panel.as_ref().expect("normal entry must create panel");
                assert!(panel.ui.window().is_visible());
                unsafe {
                    let hwnd = crate::window_by_title("WebTools Super Panel");
                    let style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                        hwnd as _, -20,
                    );
                    assert_ne!(style & 0x08000000, 0, "panel NOACTIVATE");
                    let launcher = crate::window_by_title("WebTools Launcher Replica");
                    if launcher != 0 {
                        assert_eq!(
                            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                                launcher as _,
                                -20
                            ) & 0x08000000,
                            0,
                            "launcher must activate"
                        );
                    }
                }
                panel.ui.invoke_open_search();
            }
            2 => {
                assert!(app.launcher.window().is_visible());
                assert!(app.panel.borrow().is_none());
                app.capture();
            }
            3 => {
                let p = app.panel.borrow();
                let p = p.as_ref().unwrap();
                p.ui.set_result_visible(true);
                app.escape();
                assert!(!p.ui.get_result_visible());
                assert!(p.ui.window().is_visible());
            }
            4 => {
                app.escape();
                assert!(app.panel.borrow().is_none());
                assert!(!app.launcher.window().is_visible());
            }
            _ => {
                eprintln!("PASS app normal-entry shared-surfaces search-return noactivate result-first-escape panel-escape");
                app.event(AppEvent::Quit);
            }
        });
    }
}
