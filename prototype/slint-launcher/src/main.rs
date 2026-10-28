#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod app_state;
mod native_input;
mod panel_controller;

mod clipboard;
mod config;
mod file_search;
mod mouse_gesture;
mod panel_settings;
mod settings_ui;
mod startup;

mod host_runtime;

mod icons;
mod launcher_shortcuts;

mod index;

mod tray;

mod panel_model;
mod pins;
mod search;
mod shell;

mod shell_context;

mod visual;

use std::{
    cell::RefCell,
    io::{self, BufRead, Write},
    sync::{Arc, Mutex},
    time::Instant,
};

use serde::Deserialize;

use serde_json::{json, Value};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

slint::include_modules!();

#[derive(Deserialize)]

struct Catalog {
    apps: Vec<CatalogEntry>,
}

#[derive(Deserialize)]

struct CatalogEntry {
    name: String,
    aliases: Vec<String>,
}

/// UI state survives window teardown so hide/show keeps the user's query and selection.

struct UiState {
    entries: Arc<Vec<search::Entry>>,

    query: String,

    rows: Vec<SharedString>,

    icons: Vec<slint::Image>,

    selected: i32,

    status: String,

    scale: f32,
}

impl UiState {
    fn new() -> Self {
        Self {
            entries: Arc::new(Vec::new()),
            query: String::new(),
            rows: Vec::new(),
            icons: Vec::new(),
            selected: -1,

            status: String::from("247 fixed entries · text only · no external actions"),
            scale: 1.0,
        }
    }
}

use std::sync::atomic::{AtomicBool, Ordering};

/// Some renderers (software/softbuffer) do not support the rendering notifier, so the harness

/// snapshot is emitted right after show instead of after the first rendered frame.

static NOTIFIER_ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {

    // Dropping this handle drops the winit window and the FemtoVG/GL surface behind it.

    static UI: RefCell<Option<Launcher>> = RefCell::new(None);

    static PANEL: RefCell<Option<Panel>> = RefCell::new(None);

    static FOUND_WINDOWS: RefCell<Vec<isize>> = RefCell::new(Vec::new());

    static WANTED_TITLE: RefCell<String> = RefCell::new(String::new());

    static STATE: RefCell<UiState> = RefCell::new(UiState::new());

}

fn emit(value: Value) {
    let mut output = io::stdout().lock();

    writeln!(output, "{value}").unwrap();

    output.flush().unwrap();
}

fn rows_for(entries: &[search::Entry], text: &str) -> Vec<SharedString> {
    let mut rows: Vec<SharedString> = Vec::with_capacity(10);

    rows.extend(
        search::search(entries, text)
            .iter()
            .map(|entry| SharedString::from(entry.name.as_str())),
    );

    if !text.trim().is_empty() {
        rows.push("翻译 · fixture 占位（不执行）".into());
        rows.push("搜索文件 · fixture 占位（不执行）".into());
    }

    rows
}

fn with_ui<R>(body: impl FnOnce(&Launcher) -> R) -> Option<R> {
    UI.with(|slot| slot.borrow().as_ref().map(body))
}

fn push_state(ui: &Launcher) {
    STATE.with(|state| {
        let state = state.borrow();

        ui.set_query(state.query.as_str().into());

        ui.set_selected(state.selected);

        ui.set_status(state.status.as_str().into());

        ui.set_rows(ModelRc::new(VecModel::from(state.rows.clone())));

        ui.set_row_icons(ModelRc::new(VecModel::from(state.icons.clone())));
    });
}

fn set_query(text: &str) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();

        state.query = text.to_owned();

        state.rows = rows_for(&state.entries, text);

        state.selected = if state.rows.is_empty() { -1 } else { 0 };
    });

    with_ui(push_state);
}

fn hide_now() {
    set_query("");

    if teardown_enabled() {
        teardown();
    } else {
        with_ui(|ui| {
            let _ = ui.hide();
        });
    }
}

fn snapshot(kind: &str) -> Value {
    let mut foreground_pid = 0;

    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        GetWindowThreadProcessId(GetForegroundWindow(), &mut foreground_pid);
    }

    STATE.with(|state| {

        let state = state.borrow();

        let (visible, focused, scale) = UI.with(|slot| match slot.borrow().as_ref() {

            Some(ui) => (ui.window().is_visible(), ui.get_input_focused(), ui.window().scale_factor()),

            None => (false, false, state.scale),

        });

        let physical = UI.with(|slot| slot.borrow().as_ref().map(|ui| ui.window().size())).unwrap_or_default();

        json!({"ok":true,"type":kind,"processId":std::process::id(),"query":state.query,

            "windowPhysical":[physical.width, physical.height],

            "windowLogical":[physical.width as f32 / scale.max(0.5), physical.height as f32 / scale.max(0.5)],

            "resultCount": if visible { state.rows.len() } else { 0 },

            "windowVisible":visible,"queryHasKeyboardFocus":focused,

            "isActive": foreground_pid == std::process::id(),"scaleFactor":scale,"selected":state.selected})

    })
}

/// Off by default: with winit+FemtoVG, dropping the window keeps the GL context alive and

/// re-creating it stacks a second surface, so this is a measured trade-off, not a free win.

fn teardown_enabled() -> bool {
    matches!(
        std::env::var("WT_TEARDOWN").as_deref(),
        Ok("1") | Ok("true")
    )
}

/// Drops the component (and therefore the window + GPU surface) while keeping the state.

fn teardown() {
    let dropped = UI.with(|slot| slot.borrow_mut().take());

    if let Some(ui) = dropped {
        let scale = ui.window().scale_factor();

        STATE.with(|state| state.borrow_mut().scale = scale);

        drop(ui);
    }
}

fn build(
    pending: Option<Arc<Mutex<Option<String>>>>,
    started: Instant,
) -> Result<Launcher, slint::PlatformError> {
    let ui = Launcher::new()?;

    ui.on_query_edited(|text| set_query(&text));

    ui.on_dismiss(|| hide_now());

    ui.on_move_selection(|direction| {
        STATE.with(|state| {
            let mut state = state.borrow_mut();

            state.selected = search::move_selection(state.selected, direction, state.rows.len());
        });

        with_ui(|ui| STATE.with(|state| ui.set_selected(state.borrow().selected)));
    });

    ui.on_choose(|| {
        let line = with_ui(|ui| ui.get_rows().row_data(ui.get_selected() as usize)).flatten();

        if let Some(row) = line {
            STATE.with(|state| {
                state.borrow_mut().status = format!("Fixture selection: {row} · 未执行外部动作")
            });
            with_ui(|ui| STATE.with(|state| ui.set_status(state.borrow().status.as_str().into())));
        }
    });

    if let Some(pending) = pending {
        // AfterRendering is a CPU/render submission boundary, not a monitor-present timestamp.

        let registered = ui.window().set_rendering_notifier(move |state, _| {
            if matches!(state, slint::RenderingState::AfterRendering) {
                if let Some(kind) = pending.lock().unwrap().take() {
                    let mut value = snapshot(&kind);

                    value["processInternalMs"] = json!(started.elapsed().as_secs_f64() * 1000.0);

                    emit(value);
                }
            }
        });

        NOTIFIER_ACTIVE.store(registered.is_ok(), Ordering::SeqCst);
    }

    Ok(ui)
}

/// Creates the window if it was torn down; safe to call while the window already exists.

fn ensure_ui(
    pending: &Arc<Mutex<Option<String>>>,
    started: Instant,
) -> Result<(), slint::PlatformError> {
    let exists = UI.with(|slot| slot.borrow().is_some());

    if !exists {
        let ui = build(Some(pending.clone()), started)?;

        UI.with(|slot| *slot.borrow_mut() = Some(ui));
    }

    with_ui(|ui| {
        push_state(ui);

        let _ = ui.show();

        ui.invoke_focus_input();
    });

    Ok(())
}

/// Frame hand-off: wait for the renderer's AfterRendering, or emit immediately when it has none.

fn next_frame(pending: &Arc<Mutex<Option<String>>>, started: Instant) {
    if NOTIFIER_ACTIVE.load(Ordering::SeqCst) {
        with_ui(|ui| ui.window().request_redraw());
    } else if let Some(kind) = pending.lock().unwrap().take() {
        let mut value = snapshot(&kind);

        value["processInternalMs"] = json!(started.elapsed().as_secs_f64() * 1000.0);

        emit(value);
    }
}

const WS_EX_NOACTIVATE_STYLE: isize = 0x0800_0000;

const WS_EX_TOOLWINDOW_STYLE: isize = 0x0000_0080;

const WS_EX_TOPMOST_STYLE: isize = 0x0000_0008;

const GWL_EXSTYLE_INDEX: i32 = -20;

unsafe extern "system" fn collect_window(
    hwnd: windows_sys::Win32::Foundation::HWND,
    _: windows_sys::Win32::Foundation::LPARAM,
) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut pid = 0u32;

    GetWindowThreadProcessId(hwnd, &mut pid);

    if pid == std::process::id() {
        FOUND_WINDOWS.with(|found| found.borrow_mut().push(hwnd as isize));
    }

    1
}

/// Enumerates this process's top-level windows; avoids an unstable-winit dependency.

fn process_windows() -> Vec<isize> {
    FOUND_WINDOWS.with(|found| found.borrow_mut().clear());

    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(Some(collect_window), 0) };

    FOUND_WINDOWS.with(|found| found.borrow().clone())
}

unsafe extern "system" fn match_title(
    hwnd: windows_sys::Win32::Foundation::HWND,
    _: windows_sys::Win32::Foundation::LPARAM,
) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextW, GetWindowThreadProcessId};

    let mut owner = 0;
    GetWindowThreadProcessId(hwnd, &mut owner);
    if owner != std::process::id() {
        return 1;
    }

    let mut buffer = [0u16; 256];

    let len = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);

    if len > 0 {
        let title = String::from_utf16_lossy(&buffer[..len as usize]);

        let wanted = WANTED_TITLE.with(|slot| slot.borrow().clone());

        if !wanted.is_empty() && title == wanted {
            FOUND_WINDOWS.with(|found| found.borrow_mut().push(hwnd as isize));
        }
    }

    1
}

/// Finds a top-level window of this process by title; more reliable than diffing window lists.

fn window_by_title(title: &str) -> isize {
    FOUND_WINDOWS.with(|found| found.borrow_mut().clear());

    WANTED_TITLE.with(|slot| *slot.borrow_mut() = title.to_owned());

    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows(Some(match_title), 0) };

    FOUND_WINDOWS.with(|found| found.borrow().first().copied().unwrap_or(0))
}

fn foreground_window() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

fn ex_style(hwnd: isize) -> isize {
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE_INDEX)
    }
}

/// Applies the Super Panel window semantics: never activate, never appear in the taskbar.

fn apply_panel_style(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        let style = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE_INDEX);

        let patched = style | WS_EX_NOACTIVATE_STYLE | WS_EX_TOOLWINDOW_STYLE | WS_EX_TOPMOST_STYLE;

        SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE_INDEX, patched);

        SetWindowPos(
            hwnd as _,
            std::ptr::null_mut(),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    }
}

static PANEL_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

/// Human-in-the-loop probe: show the Super Panel while the user works in another app.

/// WT_PANEL_DELAY_MS gives the tester time to click back into their target window,

/// WT_PANEL_DWELL_MS keeps the panel on screen long enough to judge, and WT_PANEL_HUMAN

/// skips the automated refocus so the foreground is whatever the user chose.

fn panel_test() {
    use std::sync::atomic::Ordering as AtomicOrdering;

    let no_activate = matches!(
        std::env::var("WT_NOACTIVATE").as_deref(),
        Ok("1") | Ok("true")
    );

    let human = matches!(
        std::env::var("WT_PANEL_HUMAN").as_deref(),
        Ok("1") | Ok("true")
    );

    let delay: u64 = std::env::var("WT_PANEL_DELAY_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if human { 5000 } else { 300 });

    let dwell: u64 = std::env::var("WT_PANEL_DWELL_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(if human { 4000 } else { 700 });

    let launcher_hwnd = process_windows().first().copied().unwrap_or(0);

    slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
        let panel = Panel::new().unwrap();

        let fg_before = foreground_window();

        panel.show().ok();

        PANEL.with(|slot| *slot.borrow_mut() = Some(panel));

        // The window is mapped by the event loop, so locate it on the next tick.

        slint::Timer::single_shot(std::time::Duration::from_millis(500), move || {
            let panel_hwnd = find_panel_window(launcher_hwnd);

            if no_activate && panel_hwnd != 0 {
                apply_panel_style(panel_hwnd);
            }

            PANEL_HWND.store(panel_hwnd, AtomicOrdering::SeqCst);

            slint::Timer::single_shot(std::time::Duration::from_millis(dwell), move || {
                let fg_after = foreground_window();

                let (w, h) = window_size(panel_hwnd);

                emit(
                    json!({"ok":true,"type":"panel-test","human":human,"noActivate":no_activate,

                    "delayMs":delay,"dwellMs":dwell,"launcherHwnd":launcher_hwnd,"panelHwnd":panel_hwnd,

                    "panelWidth":w,"panelHeight":h,

                    "styleFinal":format!("{:#x}", ex_style(panel_hwnd)),

                    "hasNoActivate":ex_style(panel_hwnd) & WS_EX_NOACTIVATE_STYLE != 0,

                    "fgBefore":fg_before,"fgAfter":fg_after,

                    "panelIsForeground":panel_hwnd != 0 && fg_after == panel_hwnd,

                    "foregroundChanged":fg_before != fg_after}),
                );

                let _ = slint::quit_event_loop();
            });
        });
    });
}

/// Locates the panel among this process's top-level windows: by title first, then by the

/// known 420x140 + frame geometry. The launcher window is excluded explicitly.

fn find_panel_window(launcher_hwnd: isize) -> isize {
    let by_title = window_by_title("WebTools Super Panel");

    if by_title != 0 && by_title != launcher_hwnd {
        return by_title;
    }

    process_windows()
        .into_iter()
        .filter(|hwnd| *hwnd != launcher_hwnd)
        .find(|hwnd| {
            let (w, h) = window_size(*hwnd);
            (300..800).contains(&w) && (100..400).contains(&h)
        })
        .unwrap_or(0)
}

fn window_size(hwnd: isize) -> (i32, i32) {
    if hwnd == 0 {
        return (0, 0);
    }

    let mut rect: windows_sys::Win32::Foundation::RECT = unsafe { std::mem::zeroed() };

    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd as _, &mut rect) };

    (rect.right - rect.left, rect.bottom - rect.top)
}

/// Loads app icons for the scroll benchmark; missing files fall back to an empty image.

fn load_icons(count: usize) -> Vec<slint::Image> {
    let dir = std::env::var("WT_ICON_DIR").unwrap_or_else(|_| String::from("icons"));

    (0..count)
        .map(|i| {
            slint::Image::load_from_path(std::path::Path::new(&format!("{dir}/{i}.png")))
                .unwrap_or_default()
        })
        .collect()
}

/// Scroll benchmark: full-window repaint cost with 247 entries and real icons.

/// Selection cycles every tick so the viewport is invalidated; the software renderer

/// has no damage tracking, so each tick costs one full raster of the window.

fn run_scroll_bench(duration_ms: u64) {
    let (rows, icons) = STATE.with(|state| {
        let state = state.borrow();

        let rows: Vec<SharedString> = state
            .entries
            .iter()
            .take(247)
            .map(|e| SharedString::from(e.name.as_str()))
            .collect();

        (rows, state.icons.clone())
    });

    let count = rows.len();

    with_ui(|ui| {
        ui.set_rows(ModelRc::new(VecModel::from(rows)));

        ui.set_row_icons(ModelRc::new(VecModel::from(icons)));
    });

    let timer = Box::leak(Box::new(slint::Timer::default()));

    let start = Instant::now();

    let mut ticks: u64 = 0;

    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(8), move || {

        ticks += 1;

        let selected = (ticks as usize % count.max(1)) as i32;

        with_ui(|ui| {

            ui.set_selected(selected);

            ui.window().request_redraw();

        });

        if start.elapsed().as_millis() as u64 >= duration_ms {

            emit(json!({"ok":true,"type":"scroll-bench","ticks":ticks,

                "elapsedMs":start.elapsed().as_millis(),"entries":count as u64,"icons":count as u64}));

            let _ = slint::quit_event_loop();

        }

    });
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--allow-foreground") {
        let process_id = std::env::args()
            .nth(2)
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or("missing or invalid foreground target process id")?;
        if visual::allow_foreground_process(process_id) {
            return Ok(());
        }
        return Err("Windows rejected foreground permission handoff".into());
    }

    if std::env::args().nth(1).as_deref() == Some("--file-index-helper") {
        return file_search::run_helper().map_err(Into::into);
    }

    if std::env::var("WT_SHELL_PROBE").is_ok() {
        use windows::Win32::Foundation::HWND;

        use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

        let hwnd = std::env::var("WT_SHELL_HWND")
            .ok()
            .and_then(|value| usize::from_str_radix(value.trim_start_matches("0x"), 16).ok())
            .map(|value| HWND(value as *mut core::ffi::c_void))
            .unwrap_or_else(|| unsafe { GetForegroundWindow() });

        match shell_context::capture_from(hwnd) {
            Some(context) => eprintln!("probe: {context:?}"),

            None => eprintln!("probe: none"),
        }

        return Ok(());
    }

    // Without a manifest the process is DPI-unaware and Windows bitmap-stretches the window,

    // so a 200% display shows a blurry, logically half-sized launcher.

    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            -4isize as *mut core::ffi::c_void,
        );
    }

    let started = Instant::now();

    let bench = std::env::args().any(|arg| arg == "--bench");

    // Renderer is a measurement variable, not a build-time constant: WT_RENDERER=femtovg|software.

    let requested = std::env::var("WT_RENDERER").unwrap_or_else(|_| String::from("software"));

    if let Err(error) = visual::native_backend(&requested).select() {
        eprintln!("renderer {requested} unavailable ({error}); falling back to software");

        visual::native_backend("software").select()?;
    }

    if std::env::args().any(|arg| arg == "--capture-test") {
        std::thread::sleep(std::time::Duration::from_millis(1500));

        match visual::capture_selection() {
            Some(text) => println!("captured={}", text.replace('\n', "\\n")),

            None => println!("captured=NONE"),
        }

        return Ok(());
    }

    if std::env::args().any(|arg| arg == "--panel-live") {
        return app::run(true);
    }

    if let Some(position) = std::env::args().position(|arg| arg == "--panel-demo") {
        let kind = std::env::args()
            .nth(position + 1)
            .unwrap_or_else(|| String::from("text"));

        let panel = SuperPanel::new()?;

        panel.set_context_kind(kind.as_str().into());

        match kind.as_str() {
            "folder" => {
                panel.set_context_items(ModelRc::new(VecModel::from(vec![
                    SharedString::from("web-tools"),
                    SharedString::from("2026 归档"),
                    SharedString::from("设计稿 v3"),
                ])));

                panel.set_actions(ModelRc::new(VecModel::from(vec![
                    PanelAction {
                        title: "在资源管理器中打开".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "复制全部路径".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "压缩为 zip".into(),
                        hint: "".into(),
                    },
                ])));
            }

            "files" => {
                panel.set_context_meta("3 个文件 · 选中".into());
                panel.set_context_items(ModelRc::new(VecModel::from(vec![
                    SharedString::from("proposal.docx"),
                    SharedString::from("budget-2026.xlsx"),
                    SharedString::from("mockup-final.png"),
                ])));

                panel.set_actions(ModelRc::new(VecModel::from(vec![
                    PanelAction {
                        title: "打开文件".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "复制路径".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "发送到翻译".into(),
                        hint: "".into(),
                    },
                ])));
            }

            "explorer" => {
                panel.set_context_name("示例文件夹".into());

                panel.set_context_path("C:\\Example".into());

                panel.set_actions(ModelRc::new(VecModel::from(vec![
                    PanelAction {
                        title: "打开终端".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "复制路径".into(),
                        hint: "".into(),
                    },
                ])));
            }

            _ => {
                panel.set_context_title("已选文本".into());
                panel.set_context_text(

                    "WebTools 是一个把搜索、翻译、文件查找整合到一个入口的桌面工具，选中文字后按下快捷键即可唤起面板。".into(),
                );

                panel.set_context_meta("86 个字符 · 来自 Notepad".into());
                panel.set_actions(ModelRc::new(VecModel::from(vec![
                    PanelAction {
                        title: "翻译为中文".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "搜索这个文本".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "复制到剪贴板".into(),
                        hint: "".into(),
                    },
                    PanelAction {
                        title: "添加到收藏".into(),
                        hint: "".into(),
                    },
                ])));
            }
        }

        let demo_glyphs: Vec<SharedString> = panel
            .get_actions()
            .iter()
            .map(|item| panel_glyph(&item.title))
            .collect();

        panel.set_action_glyphs(ModelRc::new(VecModel::from(demo_glyphs)));

        let previous = visual::current_foreground();

        panel.show()?;

        let weak = panel.as_weak();

        panel.on_action_chosen(move |index| {
            if let Some(panel) = weak.upgrade() {
                panel.set_notice(format!("已选择 {} 项（演示，无实际动作）", index + 1).into());
            }
        });

        let weak = panel.as_weak();

        panel.on_open_search(move || {
            if let Some(panel) = weak.upgrade() {
                panel.set_notice("打开 WebTools 搜索（演示）".into());
            }
        });

        for delay in [30u64, 120, 350, 800] {
            slint::Timer::single_shot(std::time::Duration::from_millis(delay), move || {
                visual::panel_noactivate();

                visual::panel_place();

                visual::restore_foreground(previous);
            });
        }

        slint::run_event_loop_until_quit()?;

        return Ok(());
    }

    if let Some(position) = std::env::args().position(|arg| arg == "--launch-test") {
        let query = std::env::args().nth(position + 1).unwrap_or_default();

        let really = std::env::args().any(|arg| arg == "--really");

        let index = index::global();

        for hit in index.search(&query, 3) {
            let app = &index.apps[hit.index];

            println!(
                "{:>5}  {:<26} -> {:?}",
                hit.score,
                app.name,
                index.command(hit.index)
            );
        }

        if really {
            if let Some(hit) = index.search(&query, 1).first() {
                match index.launch(hit.index) {
                    Ok(name) => println!("launched: {name}"),

                    Err(error) => println!("launch error: {error}"),
                }
            }
        }

        return Ok(());
    }

    if let Some(position) = std::env::args().position(|arg| arg == "--search-test") {
        let query = std::env::args().nth(position + 1).unwrap_or_default();

        let index = index::global();

        println!("index: {} apps from {}", index.apps.len(), index.origin);

        let started = Instant::now();

        let hits = index.search(&query, 12);

        println!(
            "query {:?}: {} hits in {:?}",
            query,
            hits.len(),
            started.elapsed()
        );

        for hit in &hits {
            let app = &index.apps[hit.index];

            let target = match &app.launch {
                index::Launch::Shortcut {
                    path, arguments, ..
                } => format!("shortcut {path} {arguments}"),

                index::Launch::Executable(path) => format!("executable {path}"),

                index::Launch::Packaged(id) => format!("packaged {id}"),

                index::Launch::System(app) => format!("system {app}"),

                index::Launch::None => String::from("none"),
            };

            println!(
                "  {:>5}  {:<28} [{}] {}",
                hit.score, app.name, app.source, target
            );
        }

        return Ok(());
    }

    if !bench || std::env::args().any(|arg| arg == "--baseline") {
        return app::run(false);
    }

    let catalog: Catalog =
        serde_json::from_str(include_str!("../../../fixtures/app-catalog.v1.json"))?;

    let entries: Arc<Vec<_>> = Arc::new(
        catalog
            .apps
            .into_iter()
            .map(|entry| search::Entry {
                name: entry.name,
                aliases: entry.aliases,
            })
            .collect(),
    );

    STATE.with(|state| {
        let mut state = state.borrow_mut();

        state.entries = entries.clone();

        state.icons = load_icons(247);

        state.status = format!(
            "{} fixed entries · {} icons · no external actions",
            entries.len(),
            state.icons.len()
        )
        .into();
    });

    let pending = Arc::new(Mutex::new(Some(String::from("ready"))));

    ensure_ui(&pending, started)?;

    next_frame(&pending, started);

    if std::env::args().any(|arg| arg == "--panel-test") {
        panel_test();
    }

    if let Some(position) = std::env::args().position(|arg| arg == "--scroll-bench") {
        let duration_ms: u64 = std::env::args()
            .nth(position + 1)
            .and_then(|v| v.parse().ok())
            .unwrap_or(10000);

        slint::Timer::single_shot(std::time::Duration::from_millis(700), move || {
            run_scroll_bench(duration_ms)
        });
    }

    if bench {
        let pending = pending.clone();

        std::thread::spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };

                let request: Value = match serde_json::from_str(&line) {
                    Ok(value) => value,

                    Err(error) => {
                        emit(json!({"ok":false,"error":error.to_string()}));
                        continue;
                    }
                };

                let command = request["type"].as_str().unwrap_or("").to_owned();

                let text = request["query"].as_str().unwrap_or("").to_owned();

                let pending = pending.clone();

                let dispatched = slint::invoke_from_event_loop(move || match command.as_str() {
                    "query" => {
                        if with_ui(|ui| ui.window().is_visible()).unwrap_or(false) {
                            set_query(&text);

                            *pending.lock().unwrap() = Some(String::from("query"));

                            next_frame(&pending, started);
                        } else {
                            emit(json!({"ok":false,"error":"query-while-hidden"}));
                        }
                    }

                    "show" => {
                        if ensure_ui(&pending, started).is_ok() {
                            *pending.lock().unwrap() = Some(String::from("show"));

                            next_frame(&pending, started);
                        }
                    }

                    "hide" => {
                        hide_now();

                        emit(snapshot("hide"));
                    }

                    "snapshot" => emit(snapshot("snapshot")),

                    "exit" => {
                        emit(snapshot("exit"));

                        let _ = slint::quit_event_loop();
                    }

                    _ => emit(json!({"ok":false,"error":"unsupported-command"})),
                });

                if dispatched.is_err() {
                    break;
                }
            }

            let _ = slint::quit_event_loop();
        });
    }

    let hotkey_pending = pending.clone();

    std::thread::spawn(move || unsafe {
        use windows_sys::Win32::UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*};

        let preferred = MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT;

        if RegisterHotKey(std::ptr::null_mut(), 1, preferred, VK_F11 as u32) == 0 {
            if RegisterHotKey(std::ptr::null_mut(), 1, MOD_NOREPEAT, VK_F10 as u32) == 0 {
                eprintln!("panel: hotkey registration failed for both F11 and F10");

                return;
            }

            eprintln!("panel: Ctrl+Alt+Shift+F11 is taken, falling back to F10");
        }

        let mut message: MSG = std::mem::zeroed();

        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            if message.message == WM_HOTKEY {
                let hotkey_pending = hotkey_pending.clone();

                let dispatched = slint::invoke_from_event_loop(move || {
                    let visible = with_ui(|ui| ui.window().is_visible()).unwrap_or(false);

                    if visible {
                        hide_now();
                    } else {
                        let _ = ensure_ui(&hotkey_pending, started);
                    }
                });

                if dispatched.is_err() {
                    break;
                }
            }
        }

        UnregisterHotKey(std::ptr::null_mut(), 1);
    });

    slint::run_event_loop_until_quit()?;

    Ok(())
}

/// Panel actions come from the manager's feature projection, so the panel shows

/// exactly the features the manager will accept for selected text.

/// First visible character of a title, used when no icon is available.
fn panel_glyph(title: &str) -> SharedString {
    title
        .chars()
        .next()
        .map(|c| c.to_string().into())
        .unwrap_or_else(|| "\u{00B7}".into())
}

/// Sets the action list together with its placeholder glyphs.
fn apply_actions(panel: &SuperPanel, items: Vec<PanelAction>) {
    let glyphs: Vec<SharedString> = items.iter().map(|item| panel_glyph(&item.title)).collect();
    panel.set_action_glyphs(ModelRc::new(VecModel::from(glyphs)));
    panel.set_actions(ModelRc::new(VecModel::from(items)));
}

/// Sets one board page together with its glyphs and page count.
fn apply_board(panel: &SuperPanel, items: Vec<PanelAction>, page: usize, pages: usize) {
    let glyphs: Vec<SharedString> = items.iter().map(|item| panel_glyph(&item.title)).collect();
    panel.set_board_glyphs(ModelRc::new(VecModel::from(glyphs)));
    panel.set_board(ModelRc::new(VecModel::from(items)));
    panel.set_page_label(format!("{} / {}", page + 1, pages).into());
    panel.set_pages(pages as i32);
}
