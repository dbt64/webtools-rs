//! Explicit opt-in acceptance test: owns a fresh profile and its own Manager process.
use super::*;
use std::time::Instant;

#[test]
#[ignore = "opens isolated native and Manager windows; requires WT_LIFECYCLE_MANAGER"]
fn manager_surfaces_release_ownership_and_exit() {
    if std::env::var_os("WT_LIFECYCLE_CHILD").is_none() {
        use std::os::windows::process::CommandExt;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/manager-lifecycle")
            .join(webtools_ipc::new_request_id());
        std::fs::create_dir_all(&root).unwrap();
        let log = std::fs::File::create(root.join("lifecycle.log")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "app::lifecycle_tests::manager_surfaces_release_ownership_and_exit",
                "--nocapture",
            ])
            .env("WT_LIFECYCLE_CHILD", "1")
            .env("WT_PROFILE_ROOT", &root)
            .env("WT_PROTOCOL_TRACE", "1")
            .creation_flags(0x08000000)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .status()
            .unwrap();
        eprintln!("evidence={}", root.display());
        assert!(
            status.success(),
            "{}",
            std::fs::read_to_string(root.join("lifecycle.log")).unwrap()
        );
        return;
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()
        .unwrap();
    let (events, receiver) = mpsc::channel();
    let app = Rc::new(NativeApp {
        launcher: Baseline::new().unwrap(),
        state: RefCell::default(),
        panel: RefCell::new(None),
        rows: RefCell::new(vec![]),
        projection_cache: webtools_ipc::projection_cache::ProjectionCache::memory(),
        targets: RefCell::new(vec![]),
        website_targets: RefCell::new(Vec::new()),
        plugin_targets: RefCell::new(Vec::new()),
        launcher_icons: RefCell::new(std::collections::HashMap::new()),
        config: Config {
            manager_exe: Some(std::env::var("WT_LIFECYCLE_MANAGER").unwrap()),
            ..Default::default()
        },
        runtime: RefCell::new(None),
        ownership: webtools_ipc::HostOwnership::default(),
        active_session: RefCell::new(None),
        pending_handoff: RefCell::new(None),
        focus_request_pending: Cell::new(false),
        launcher_focus_pending: Rc::new(Cell::new(false)),
        panel_result: RefCell::new(None),
        host_events: RefCell::new(None),
        events,
        receiver,
        external: Cell::new(0),
        workers: RefCell::new(vec![]),
        capture_cancel: RefCell::new(None),
        settings: RefCell::new(webtools_ipc::state::default_launcher_state()),
        launcher_state: Arc::new(std::sync::Mutex::new(
            webtools_ipc::state::LauncherStateStore::load(&crate::config::profile_root()).unwrap(),
        )),
        input: RefCell::new(None),
        panel_settings: Rc::new(RefCell::new(PanelSettingsController::load(
            &crate::config::profile_root(),
        ))),
        search_generation: Arc::new(AtomicU64::new(0)),
        file_hits: RefCell::new(Vec::new()),
        file_icon_pixels: RefCell::new(Vec::new()),
        file_category: Cell::new(crate::file_search::FileCategory::All),
    });
    app.wire();
    // Owns its tray/Manager only; no singleton, global hotkeys or foreign windows.
    let tray = Rc::new(crate::tray::install(app.events.clone()));
    app.state.borrow_mut().launcher();
    app.launcher.show().unwrap();
    let generation = app.search_generation.load(Ordering::SeqCst);
    app.event(AppEvent::FileSearchStatus {
        generation,
        status: "current progress".into(),
    });
    assert_eq!(app.launcher.get_status().as_str(), "current progress");
    app.event(AppEvent::FileSearchStatus {
        generation: generation + 1,
        status: "stale progress".into(),
    });
    assert_eq!(app.launcher.get_status().as_str(), "current progress");
    app.hide();
    assert!(
        app.search_generation.load(Ordering::SeqCst) > generation,
        "hiding cancels the active file query"
    );
    app.launcher.set_status("hidden".into());
    app.event(AppEvent::FileSearchStatus {
        generation: app.search_generation.load(Ordering::SeqCst),
        status: "late progress".into(),
    });
    assert_eq!(app.launcher.get_status().as_str(), "hidden");
    app.state.borrow_mut().launcher();
    app.launcher.show().unwrap();
    app.begin_manager_session();
    if std::env::var_os("WT_LIFECYCLE_FORCE_START_FAILURE").is_some() {
        let (events, receiver) = mpsc::channel();
        let failed = HostRuntime::start(
            crate::config::profile_root()
                .join("missing-manager.exe")
                .to_string_lossy()
                .into_owned(),
            vec!["--phase4g-manager-test".into(), "--manager-only".into()],
            Duration::from_secs(30),
            events,
            app.ownership.clone(),
            app.launcher_state.clone(),
            webtools_ipc::projection_cache::ProjectionCache::memory(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while failed.handle().send(HostCommand::SyncOwnership).is_ok() {
            assert!(
                Instant::now() < deadline,
                "failed supervisor did not terminate"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        app.host_events.replace(Some(receiver));
        app.runtime.replace(Some(failed));
        eprintln!("RECOVERY REPRO: previous supervisor has exited; next W click must start a fresh Manager");
    }
    app.launcher.invoke_open_manager();
    let started = Instant::now();
    let mut stage = 0;
    let mut stage_at = Instant::now();
    let mut pid = 0;
    let mut reopened_session = String::new();
    let timer = slint::Timer::default();
    let test_app = app.clone();
    let failure = Rc::new(RefCell::new(None));
    let timer_failure = failure.clone();
    let exit_pid = Rc::new(Cell::new(0));
    let final_pid = exit_pid.clone();
    let test_tray = tray.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(30), move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let app = &test_app;
        app.pump();
        assert!(started.elapsed() < Duration::from_secs(60), "lifecycle stalled stage={stage} pid={pid} owners={} active={:?}", app.ownership.has_owners(), app.active_session.borrow());
        assert!(stage_at.elapsed() < Duration::from_secs(15), "lifecycle stage={stage} timed out owners={} active={:?}", app.ownership.has_owners(), app.active_session.borrow());
        if pid == 0 { pid = manager_pid(); }
        match stage {
            0 if !app.ownership.has_owners() && pid != 0 => {
                assert!(!app.launcher.window().is_visible());
                assert!(!app.ownership.has_owners());
                assert!(running(pid), "Manager surface must keep Electron alive");
                if close_owned_windows(pid) == 0 { return; }
                eprintln!("W entry acknowledged, native ownership empty, close Manager pid={pid}");
                stage = 1; stage_at = Instant::now();
            }
            1 if !running(pid) => {
                eprintln!("PASS W entry -> close Manager -> natural process exit after {:?}", stage_at.elapsed());
                app.state.borrow_mut().launcher(); app.launcher.show().unwrap(); app.begin_manager_session();
                app.targets.replace(vec![visual::Target::Feature { plugin: "lifecycle.missing".into(), feature: "missing".into(), launcher_text: None }]);
                app.launcher.set_selected(0);
                app.launcher.invoke_submit();
                stage = 2; stage_at = Instant::now(); pid = 0;
            }
            1 => assert!(stage_at.elapsed() < Duration::from_secs(10), "Electron remained after W Manager close"),
            2 if pid != 0 && !running(pid) => {
                assert!(!app.ownership.has_owners());
                eprintln!("PASS rejected Launcher feature -> native hidden -> natural process exit");
                app.state.borrow_mut().launcher(); app.launcher.show().unwrap();
                app.begin_manager_session(); app.ensure_manager();
                stage = 3; stage_at = Instant::now(); pid = 0;
            }
            2 => assert!(stage_at.elapsed() < Duration::from_secs(15), "Electron retained after hidden Launcher feature: owners={} active={:?}", app.ownership.has_owners(), app.active_session.borrow()),
            3 if app.rows.borrow().iter().any(|row| row.plugin_id == "webtools.translation" && row.feature_id == "translate-text") => {
                let previous = app.active_session.borrow().clone().unwrap();
                app.launcher.invoke_open_manager();
                assert_eq!(app.active_session.borrow().as_deref(), Some(previous.as_str()), "source session remains until the window handoff completes");
                assert!(app.pending_handoff.borrow().is_some(), "window handoff must be pending");
                reopened_session = previous;
                stage = 30; stage_at = Instant::now();
            }
            30 if app.active_session.borrow().is_none() && !app.launcher.window().is_visible() => {
                let previous = std::mem::take(&mut reopened_session);
                app.state.borrow_mut().launcher(); app.launcher.show().unwrap();
                reopened_session = app.begin_manager_session();
                assert_ne!(previous, reopened_session, "reopened Launcher requires a fresh owner id");
                stage = 4; stage_at = Instant::now();
            }
            4 if pid != 0 && stage_at.elapsed() > Duration::from_secs(1) => {
                assert_eq!(app.active_session.borrow().as_deref(), Some(reopened_session.as_str()));
                assert!(app.ownership.has_owners(), "previous Manager target ack cannot release reopened Launcher");
                if close_owned_windows(pid) == 0 { return; }
                stage = 5; stage_at = Instant::now();
            }
            5 if stage_at.elapsed() > Duration::from_millis(300) => {
                assert!(running(pid), "visible native Launcher must keep Electron alive after Manager window close");
                assert!(app.launcher.window().is_visible());
                app.hide(); stage = 6; stage_at = Instant::now();
            }
            6 if !running(pid) => {
                assert!(!app.ownership.has_owners());
                eprintln!("PASS successful Manager target -> reopen Launcher with fresh owner -> close Manager retains Electron -> hide Launcher -> natural exit");
                app.state.borrow_mut().launcher(); app.launcher.show().unwrap(); app.begin_manager_session();
                app.launcher.invoke_open_manager();
                stage = 7; stage_at = Instant::now(); pid = 0;
            }
            7 if pid != 0 && !app.ownership.has_owners() => {
                final_pid.set(pid);
                eprintln!("TRAY EXIT REPRO: Manager pid={pid} is live; dispatch actual tray Exit");
                test_tray.request_test_exit(); stage = 8; stage_at = Instant::now();
            }
            3..=8 | 30 => assert!(stage_at.elapsed() < Duration::from_secs(10), "Manager target ownership stalled at stage={stage}"),
            _ => {}
        }
        }));
        if let Err(error) = outcome {
            timer_failure.replace(Some(error));
            slint::quit_event_loop().unwrap();
        }
    });
    slint::run_event_loop_until_quit().unwrap();
    timer.stop();
    drop(timer);
    drop(tray);
    if let Some(runtime) = app.runtime.borrow_mut().take() {
        runtime.shutdown();
    };
    if let Some(error) = failure.borrow_mut().take() {
        std::panic::resume_unwind(error);
    };
    assert_ne!(
        exit_pid.get(),
        0,
        "must exercise tray Exit while Manager is alive"
    );
    assert!(
        !running(exit_pid.get()),
        "tray Exit must stop its Manager process"
    );
    eprintln!("PASS actual tray Exit -> event loop exits -> runtime joins -> Manager process gone");
}

fn manager_pid() -> u32 {
    use windows_sys::Win32::{Foundation::CloseHandle, System::Diagnostics::ToolHelp::*};
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        assert_ne!(snapshot as isize, -1);
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut pid = 0;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let end = entry
                    .szExeFile
                    .iter()
                    .position(|c| *c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
                if entry.th32ParentProcessID == std::process::id()
                    && name.eq_ignore_ascii_case("WebTools.exe")
                    && running(entry.th32ProcessID)
                {
                    pid = entry.th32ProcessID;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        pid
    }
}
fn running(pid: u32) -> bool {
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return false;
        }
        let live = WaitForSingleObject(process, 0) == 258;
        CloseHandle(process);
        live
    }
}
fn close_owned_windows(pid: u32) -> usize {
    use windows_sys::Win32::{Foundation::*, UI::WindowsAndMessaging::*};
    unsafe extern "system" fn visit(hwnd: HWND, data: LPARAM) -> i32 {
        let state = &mut *(data as *mut (u32, usize));
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == state.0 && IsWindowVisible(hwnd) != 0 {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
            state.1 += 1;
        }
        1
    }
    let mut state = (pid, 0);
    unsafe {
        EnumWindows(Some(visit), &mut state as *mut _ as LPARAM);
    }
    state.1
}
