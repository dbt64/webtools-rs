//! Explicit process-level test of the normal app; never compiled into a release.
use super::*;
use serde_json::json;
use std::{path::Path, sync::atomic::AtomicUsize};
use webtools_ipc::{state::LauncherStateStore, Connection, Envelope, HostSession, PipeServer};

#[test]
#[ignore = "opens its own test windows; run explicitly with --ignored --exact"]
fn settings_roundtrip_normal_entry() {
    if std::env::var_os("WT_SETTINGS_TEST_CHILD").is_some() {
        slint::BackendSelector::new()
            .backend_name("winit".into())
            .renderer_name("software".into())
            .select()
            .unwrap();
        run(false).unwrap();
        return;
    }
    use std::os::windows::process::CommandExt;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.tmp/r2-settings-smoke")
        .join(webtools_ipc::new_request_id());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.json"), "{}").unwrap();
    let shortcuts = available_shortcuts();
    LauncherStateStore::load(&root)
        .unwrap()
        .apply_settings(&json!({"quickSearchShortcut":shortcuts[0]}))
        .unwrap();
    for stage in ["apply", "restore"] {
        let log = std::fs::File::create(root.join(format!("{stage}.log"))).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "app::settings_tests::settings_roundtrip_normal_entry",
                "--nocapture",
            ])
            .env("WT_SETTINGS_TEST_CHILD", stage)
            .env("WT_PROFILE_ROOT", &root)
            .env("WT_CONFIG", root.join("config.json"))
            .env("WT_PANEL_FAKE_SELECTION", "设置验证选区")
            .env("WT_SETTINGS_TEST_SHORTCUT", &shortcuts[1])
            .env_remove("WT_APP_SMOKE")
            .env_remove("WT_PREWARM_BENCH")
            .creation_flags(0x08000000)
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "{}",
            std::fs::read_to_string(root.join(format!("{stage}.log"))).unwrap()
        );
    }
    eprintln!(
        "PASS settings apply + fresh-process restore; evidence={}",
        root.display()
    );
}

fn available_shortcuts() -> Vec<String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
    let mut result = Vec::new();
    for n in 1..=10 {
        if unsafe { RegisterHotKey(std::ptr::null_mut(), 600 + n, 14, 0x6f + n as u32) } != 0 {
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), 600 + n);
            }
            result.push(format!("Control+Shift+Super+F{n}"));
            if result.len() == 2 {
                break;
            }
        }
    }
    assert_eq!(result.len(), 2);
    result
}

fn assert_registered(shortcut: &str) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
    let webtools_ipc::hotkey::Binding::Chord { modifiers, key } =
        webtools_ipc::hotkey::Binding::parse(shortcut).unwrap()
    else {
        panic!("chord expected")
    };
    let acquired = unsafe { RegisterHotKey(std::ptr::null_mut(), 999, modifiers, key) } != 0;
    if acquired {
        unsafe {
            UnregisterHotKey(std::ptr::null_mut(), 999);
        }
    }
    assert!(
        !acquired,
        "normal input thread must own the persisted shortcut"
    );
}

pub(super) struct Driver {
    timer: slint::Timer,
    threads: Vec<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}
impl Driver {
    pub(super) fn finish(self) {
        self.timer.stop();
        self.stop.store(true, Ordering::SeqCst);
        for thread in self.threads {
            thread.join().unwrap();
        }
    }
}

pub(super) fn drive(app: Rc<NativeApp>) -> Driver {
    let root = crate::config::profile_root();
    let shortcut = std::env::var("WT_SETTINGS_TEST_SHORTCUT").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let timer = slint::Timer::default();
    if std::env::var("WT_SETTINGS_TEST_CHILD").as_deref() == Ok("restore") {
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(350),
            move || {
                assert!(app.launcher.global::<crate::HostTheme>().get_light());
                assert!(app.launcher.get_expanded());
                assert_eq!(app.settings.borrow()["quickSearchShortcut"], shortcut);
                assert_registered(&shortcut);
                snapshot(app.launcher.window(), &root.join("restored.bmp"));
                app.event(AppEvent::Quit);
            },
        );
        return Driver {
            timer,
            threads: vec![],
            stop,
        };
    }
    let (host_events, receiver) = mpsc::channel();
    app.host_events.replace(Some(receiver));
    let name = format!("WebTools.R2.ui.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let store = LauncherStateStore::load(&root).unwrap();
    let host_stop = stop.clone();
    let server_thread = std::thread::spawn(move || {
        let connection = server
            .accept_until(&host_stop, Duration::from_secs(8))
            .unwrap();
        let mut session = HostSession::with_state_store(connection, store);
        session.set_settings_handler(crate::host_runtime::settings_handler(
            host_events,
            host_stop,
        ));
        for _ in 0..5 {
            assert!(session.pump(Duration::from_secs(8)).unwrap());
        }
    });
    let stage = Arc::new(AtomicUsize::new(0));
    let client_stage = stage.clone();
    let client_root = root.clone();
    let client_shortcut = shortcut.clone();
    let (proceed, proceed_rx) = mpsc::sync_channel(1);
    let client_thread = std::thread::spawn(move || {
        let old_shortcut = LauncherStateStore::load(&client_root).unwrap().snapshot()
            ["quickSearchShortcut"]
            .as_str()
            .unwrap()
            .to_string();
        let mut client = Connection::connect(&name, Duration::from_secs(5)).unwrap();
        let mut request = |kind: &str, payload| {
            client
                .request(&Envelope::new(kind, payload), Duration::from_secs(5))
                .unwrap()
        };
        assert!(
            request(
                "hello",
                json!({"processId":std::process::id(),"capabilities":[]})
            )
            .ok
        );
        let applied = request(
            "launcher-settings-update",
            json!({"theme":"light","launcherDisplayMode":"expanded","quickSearchShortcut":client_shortcut,"everythingEnabled":true}),
        );
        assert!(applied.ok, "{:?}", applied.error);
        assert_eq!(applied.result.unwrap()["theme"], "light");
        client_stage.store(1, Ordering::SeqCst);
        proceed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let invalid = request(
            "launcher-settings-update",
            json!({"quickSearchShortcut":"bad"}),
        );
        assert_eq!(invalid.error.unwrap().code, "INVALID_HOTKEY");
        let startup = request("launcher-settings-update", json!({"launchOnStartup":true}));
        assert_eq!(startup.error.unwrap().code, "STARTUP_UNAVAILABLE");
        use std::os::windows::fs::OpenOptionsExt;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(client_root.join("launcher-state.json"))
            .unwrap();
        let failed = request(
            "launcher-settings-update",
            json!({"theme":"dark","quickSearchShortcut":old_shortcut}),
        );
        assert_eq!(failed.error.unwrap().code, "INVALID_PAYLOAD");
        drop(lock);
        let webtools_ipc::hotkey::Binding::Chord { modifiers, key } =
            webtools_ipc::hotkey::Binding::parse(&old_shortcut).unwrap()
        else {
            unreachable!()
        };
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
        let released = unsafe { RegisterHotKey(std::ptr::null_mut(), 991, modifiers, key) } != 0;
        if released {
            unsafe {
                UnregisterHotKey(std::ptr::null_mut(), 991);
            }
        }
        assert!(released, "failed update must release its temporary chord");
        assert_eq!(
            LauncherStateStore::load(&client_root).unwrap().snapshot()["theme"],
            "light"
        );
        client_stage.store(2, Ordering::SeqCst);
    });
    let started = std::time::Instant::now();
    let mut step = 0;
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(60),
        move || {
            if started.elapsed() > Duration::from_secs(12) {
                app.event(AppEvent::Quit);
                panic!("settings UI roundtrip did not finish");
            }
            if stage.load(Ordering::SeqCst) == 1 {
                assert!(app.launcher.global::<crate::HostTheme>().get_light());
                assert_eq!(app.settings.borrow()["quickSearchShortcut"], shortcut);
                assert_eq!(app.settings.borrow()["everythingEnabled"], true);
                assert_registered(&shortcut);
                if step == 0 {
                    app.show_launcher();
                    assert!(app.launcher.get_expanded());
                    step = 1;
                } else if step == 1
                    && app.launcher.window().size().height as f32
                        >= 325.0 * app.launcher.window().scale_factor()
                {
                    snapshot(app.launcher.window(), &root.join("light-expanded.bmp"));
                    app.capture();
                    step = 2;
                } else if step == 2 {
                    if let Some(panel) = app.panel.borrow().as_ref() {
                        assert!(panel.ui.global::<crate::HostTheme>().get_light());
                        snapshot(panel.ui.window(), &root.join("light-panel.bmp"));
                        let _ = proceed.try_send(());
                        step = 3;
                    }
                }
            }
            if stage.load(Ordering::SeqCst) == 2 {
                if step!=4 {
                    assert!(app.launcher.global::<crate::HostTheme>().get_light());assert_registered(&shortcut);
                    assert!(app.panel.borrow().as_ref().unwrap().ui.global::<crate::HostTheme>().get_light());
                    let mut system=app.settings.borrow().clone();system["theme"]=json!("system");app.apply_visual_settings(&system);
                    app.launcher.global::<crate::HostTheme>().set_light(!crate::settings_ui::system_light());
                    unsafe {windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(crate::window_by_title("WebTools Launcher Replica") as _,windows_sys::Win32::UI::WindowsAndMessaging::WM_SETTINGCHANGE,0,0);}
                    step=4;
                }else{
                    assert_eq!(app.launcher.global::<crate::HostTheme>().get_light(),crate::settings_ui::system_light());
                    app.apply_visual_settings(&LauncherStateStore::load(&root).unwrap().snapshot());
                    eprintln!("PASS native settings UI application, persistence rollback and system theme message");app.event(AppEvent::Quit);
                }
            }
        },
    );
    Driver {
        timer,
        threads: vec![server_thread, client_thread],
        stop,
    }
}

fn snapshot(window: &slint::Window, path: &Path) {
    use std::io::Write;
    let pixels = window.take_snapshot().unwrap();
    let width = pixels.width();
    let height = pixels.height();
    let white = pixels
        .as_slice()
        .iter()
        .filter(|p| p.r > 230 && p.g > 230 && p.b > 230)
        .count();
    assert!(
        white > pixels.as_slice().len() / 5,
        "light surface must actually render light pixels"
    );
    let mut header = [0u8; 54];
    header[..2].copy_from_slice(b"BM");
    header[2..6].copy_from_slice(&(54 + width * height * 4).to_le_bytes());
    header[10..14].copy_from_slice(&54u32.to_le_bytes());
    header[14..18].copy_from_slice(&40u32.to_le_bytes());
    header[18..22].copy_from_slice(&width.to_le_bytes());
    header[22..26].copy_from_slice(&height.to_le_bytes());
    header[26..28].copy_from_slice(&1u16.to_le_bytes());
    header[28..30].copy_from_slice(&32u16.to_le_bytes());
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    for row in pixels.as_slice().chunks(width as usize).rev() {
        for pixel in row {
            file.write_all(&[pixel.b, pixel.g, pixel.r, pixel.a])
                .unwrap();
        }
    }
}
