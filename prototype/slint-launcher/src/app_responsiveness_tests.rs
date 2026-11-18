//! Explicit desktop probe using the reported saved icon, without global hooks or Manager.
use super::*;
use std::time::Instant;

fn isolated_app() -> Rc<NativeApp> {
    let source = std::path::PathBuf::from(std::env::var_os("WT_ICON_PROBE_PROFILE").unwrap());
    let mut settings: serde_json::Value =
        serde_json::from_reader(std::fs::File::open(source.join("launcher-state.json")).unwrap())
            .unwrap();
    settings["launcherDisplayMode"] = "compact".into();
    let data: serde_json::Value = serde_json::from_reader(
        std::fs::File::open(source.join("runtime/wt-manager-profile/nook-data.json")).unwrap(),
    )
    .unwrap();
    let entries = data["webEntries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|site| serde_json::json!({"id":site["id"], "favicon":site["favicon"]}))
        .collect::<Vec<_>>();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/launcher-responsiveness")
        .join(webtools_ipc::new_request_id());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("nook-data.json"),
        serde_json::to_vec(&serde_json::json!({"webEntries":entries})).unwrap(),
    )
    .unwrap();
    std::env::set_var("WT_PROFILE_ROOT", &root);
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(-4isize as _);
    }
    visual::native_backend(&std::env::var("WT_FOCUS_PROBE_RENDERER").unwrap_or_else(|_| "software".into())).select().unwrap();
    let (events, receiver) = mpsc::channel();
    let app = Rc::new(NativeApp {
        launcher: Baseline::new().unwrap(),
        state: RefCell::default(),
        panel: RefCell::new(None),
        rows: RefCell::new(Vec::new()),
        projection_cache: webtools_ipc::projection_cache::ProjectionCache::memory(),
        targets: RefCell::new(Vec::new()),
        website_targets: RefCell::new(Vec::new()),
        plugin_targets: RefCell::new(Vec::new()),
        launcher_icons: RefCell::new(std::collections::HashMap::new()),
        config: Config::default(),
        runtime: RefCell::new(None),
        mcp_server: RefCell::new(None),
        ownership: webtools_ipc::HostOwnership::default(),
        active_session: RefCell::new(None),
        pending_handoff: RefCell::new(None),
        focus_request_pending: Cell::new(false),
        launcher_focus_pending: Rc::new(Cell::new(false)),
        panel_result: RefCell::new(None),
        host_events: RefCell::new(None),
        events,
        receiver,
        external: Cell::new(crate::paste_target::PasteTarget::default()),
        pending_launcher_origin: Cell::new(None),
        workers: RefCell::new(Vec::new()),
        capture_cancel: RefCell::new(None),
        settings: RefCell::new(settings),
        input: RefCell::new(None),
        launcher_state: Arc::new(std::sync::Mutex::new(
            webtools_ipc::state::LauncherStateStore::memory(),
        )),
        panel_settings: Rc::new(RefCell::new(PanelSettingsController::load(&root))),
        search_generation: Arc::new(AtomicU64::new(0)),
        file_hits: RefCell::new(Vec::new()),
        file_icon_pixels: RefCell::new(Vec::new()),
        file_category: Cell::new(crate::file_search::FileCategory::All),
    });
    app.launcher.set_fixture_icons(false);
    app.wire();
    app
}

#[test]
#[ignore = "opens an isolated desktop window; reads WT_ICON_PROBE_PROFILE without modifying it"]
fn saved_site_toggle_probe() {
    let app = isolated_app();
    eprintln!("toggle-probe: begin-show pid={}", std::process::id());
    let at = Instant::now();
    app.show_launcher();
    eprintln!(
        "toggle-probe: initial-show-ms={:.2}",
        at.elapsed().as_secs_f64() * 1000.0
    );
    for (delay, action) in [(250, 0), (700, 1), (1000, 2), (1400, 3)] {
        let app = app.clone();
        slint::Timer::single_shot(Duration::from_millis(delay), move || {
            let at = Instant::now();
            eprintln!("toggle-probe: begin-action={action}");
            match action {
                0 | 2 => {
                    app.launcher.set_expanded(true);
                    app.launcher.invoke_refresh_shortcuts();
                }
                1 => {
                    app.hide();
                    app.show_launcher();
                }
                _ => {
                    app.hide();
                    let _ = slint::quit_event_loop();
                }
            }
            eprintln!(
                "toggle-probe: action={action} ms={:.2}",
                at.elapsed().as_secs_f64() * 1000.0
            );
        });
    }
    slint::run_event_loop_until_quit().unwrap();
}

#[test]
#[ignore = "opens an isolated Launcher to inspect actual Windows taskbar styles"]
fn launcher_retains_activation_when_skip_taskbar_is_enabled() {
    let app = isolated_app();
    app.show_launcher();
    let style = Rc::new(Cell::new(None));
    let observed = style.clone();
    let owner = app.clone();
    slint::Timer::single_shot(Duration::from_millis(100), move || {
        let hwnd = crate::window_by_title("WebTools Launcher Replica");
        if hwnd != 0 {
            observed.set(Some(unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongPtrW(
                    hwnd as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::GWL_EXSTYLE,
                )
            }));
        }
        owner.hide();
        let _ = slint::quit_event_loop();
    });
    slint::run_event_loop_until_quit().unwrap();
    let style = style.get().expect("real Launcher HWND");
    eprintln!("launcher: exstyle=0x{style:x}");
    // winit's skip-taskbar API uses ITaskbarList::DeleteTab, not WS_EX_TOOLWINDOW.
    // Taskbar suppression is checked against the actual Shell UI separately.
    assert_eq!(
        style & 0x8000000,
        0,
        "Launcher must still accept keyboard focus"
    );
}

#[test]
#[ignore = "opens an isolated cold Launcher and sends keyboard input only to its own foreground window"]
fn cold_launcher_accepts_keyboard_without_a_click_or_second_show() {
    exercise_launcher_keyboard(false);
}

#[test]
#[ignore = "checks a foreground Launcher with no native keyboard focus using only its own HWND"]
fn foreground_launcher_recovers_missing_keyboard_focus() {
    exercise_launcher_keyboard(true);
}

fn exercise_launcher_keyboard(clear_native_focus: bool) {
    let app = isolated_app();
    let input = native_input::NativeInput::start(app.events.clone(), "Ctrl+Shift+F9".into(), Vec::new());
    let pump = slint::Timer::default();
    let owner = app.clone();
    pump.start(slint::TimerMode::Repeated, Duration::from_millis(25), move || owner.pump());
    let result = Rc::new(RefCell::new(None));
    slint::Timer::single_shot(Duration::from_millis(80), move || {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let key = |vk, flags| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
        let inputs = [key(VK_CONTROL, 0), key(VK_SHIFT, 0), key(VK_F9, 0), key(VK_F9, KEYEVENTF_KEYUP), key(VK_SHIFT, KEYEVENTF_KEYUP), key(VK_CONTROL, KEYEVENTF_KEYUP)];
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); }
    });
    let owner = app.clone();
    let observed = result.clone();
    let typed = Rc::new(Cell::new(false));
    let input_timer = slint::Timer::default();
    input_timer.start(slint::TimerMode::Repeated, Duration::from_millis(20), move || {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let hwnd = crate::window_by_title("WebTools Launcher Replica");
        if typed.get() || hwnd == 0 || visual::current_foreground() != hwnd { return; }
        typed.set(true);
        if clear_native_focus {
            unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(std::ptr::null_mut()); }
            owner.refocus_launcher_after_handoff();
        }
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            let mut info: GUITHREADINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            GetGUIThreadInfo(GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut()), &mut info);
            eprintln!("cold-focus: native-focus={:?} launcher={hwnd:x} ctrl={} shift={}", info.hwndFocus, GetAsyncKeyState(VK_CONTROL as i32), GetAsyncKeyState(VK_SHIFT as i32));
        }
        let key = |flags| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: 0, wScan: b'x' as u16, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
        let inputs = [key(KEYEVENTF_UNICODE), key(KEYEVENTF_UNICODE | KEYEVENTF_KEYUP)];
        unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); }
        let owner = owner.clone();
        let observed = observed.clone();
        slint::Timer::single_shot(Duration::from_millis(150), move || {
            let native_focus = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() as isize };
            *observed.borrow_mut() = Some((visual::current_foreground() == hwnd, native_focus == hwnd, owner.launcher.get_query().to_string()));
            owner.hide();
            let _ = slint::quit_event_loop();
        });
    });
    slint::Timer::single_shot(Duration::from_secs(4), || { let _ = slint::quit_event_loop(); });
    slint::run_event_loop_until_quit().unwrap();
    input_timer.stop();
    pump.stop();
    drop(input);
    assert_eq!(result.borrow().as_ref(), Some(&(true, true, "x".into())), "cold show must own native keyboard focus and receive input without a mouse click");
}

#[test]
#[ignore = "opens an isolated Launcher and routes a native tray Exit command through normal shutdown"]
fn tray_menu_exit_fixture() {
    let app = isolated_app();
    let tray = Rc::new(crate::tray::install(app.events.clone()));
    let exited = Rc::new(Cell::new(false));
    app.show_launcher();
    let timer = slint::Timer::default();
    let owner = app.clone();
    let result = exited.clone();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(25),
        move || {
            while let Ok(event) = owner.receiver.try_recv() {
                if matches!(event, AppEvent::Quit) {
                    result.set(true);
                    eprintln!("tray-fixture: exit-received");
                }
                owner.event(event);
            }
        },
    );
    let popup = tray.clone();
    slint::Timer::single_shot(Duration::from_millis(500), move || popup.open_test_menu());
    let selection = tray.clone();
    slint::Timer::single_shot(Duration::from_millis(900), move || {
        selection.request_test_exit()
    });
    // The brand click re-opens the real native tray menu in this test only.
    let popup = tray.clone();
    app.launcher.on_open_manager(move || popup.open_test_menu());
    slint::Timer::single_shot(Duration::from_secs(120), || {
        let _ = slint::quit_event_loop();
    });
    eprintln!("tray-fixture: ready pid={}", std::process::id());
    slint::run_event_loop_until_quit().unwrap();
    timer.stop();
    assert!(
        exited.get(),
        "Exit must reach the normal application shutdown path"
    );
    drop(tray);
}
