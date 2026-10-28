//! Normal-entry process/pipe/Win32-hook configuration integration, isolated profile only.
use super::*;
use serde_json::json;
use std::{path::Path, sync::atomic::AtomicUsize};
use webtools_ipc::{super_panel::*, Connection, Envelope, HostSession, PipeServer};

#[test]
#[ignore = "opens its own test windows; run explicitly with --ignored --exact"]
fn panel_settings_roundtrip_normal_entry() {
    if std::env::var_os("WT_PANEL_SETTINGS_TEST_CHILD").is_some() {
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
        .join("../../.tmp/r2-panel-settings-smoke")
        .join(webtools_ipc::new_request_id());
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("config.json"), "{}").unwrap();
    // This v1 profile must migrate in memory and keep its old pin until an explicit save.
    std::fs::write(root.join("super-panel-state.json"),json!({"schemaVersion":1,"enabled":true,"middleHoldEnabled":true,"holdMilliseconds":600,"pinned":[{"pluginId":"test.app","featureId":"open"}]}).to_string()).unwrap();
    for stage in ["apply", "restore"] {
        let log = std::fs::File::create(root.join(format!("{stage}.log"))).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "app::panel_tests::panel_settings_roundtrip_normal_entry",
                "--nocapture",
            ])
            .env("WT_PANEL_SETTINGS_TEST_CHILD", stage)
            .env("WT_PROFILE_ROOT", &root)
            .env("WT_CONFIG", root.join("config.json"))
            .env("WT_PANEL_FAKE_SELECTION", "面板设置验证选区")
            .env_remove("WT_APP_SMOKE")
            .env_remove("WT_SETTINGS_TEST_CHILD")
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
    eprintln!("PASS panel settings real pipe + native hook configuration + disable/rollback + fresh-process restore; evidence={}",root.display());
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
fn target() -> PanelSettings {
    PanelSettings {
        mouse_button: MouseButton::Xbutton1,
        gesture: Gesture::Press,
        hold_milliseconds: 450,
        auto_translation_preview: true,
        ..Default::default()
    }
}
pub(super) fn drive(app: Rc<NativeApp>) -> Driver {
    let root = crate::config::profile_root();
    let stop = Arc::new(AtomicBool::new(false));
    let timer = slint::Timer::default();
    if std::env::var("WT_PANEL_SETTINGS_TEST_CHILD").as_deref() == Ok("restore") {
        let started = std::time::Instant::now();
        let mut began = false;
        timer.start(slint::TimerMode::Repeated,Duration::from_millis(100),move||{
            assert!(started.elapsed()<Duration::from_secs(5),"restore panel was not presented");
            assert_eq!(app.panel_settings.borrow().status().settings,target());
            assert_eq!(app.panel_settings.borrow().status().trigger_status,TriggerStatus::Registered);
            assert_eq!(app.panel_settings.borrow().pins().len(),2);
            if !began{app.capture();began=true;return;}
            if let Some(p)=app.panel.borrow().as_ref(){assert!(p.ui.window().is_visible());}
            else{return;}
            eprintln!("PASS restart restores real registered trigger and pins; fake-selection panel visible");app.event(AppEvent::Quit);
        });
        return Driver {
            timer,
            threads: vec![],
            stop,
        };
    }
    let initial = app.panel_settings.borrow().status();
    assert_eq!(initial.trigger_status, TriggerStatus::Registered);
    assert_eq!(app.panel_settings.borrow().pins().len(), 1);
    let disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("super-panel-state.json")).unwrap())
            .unwrap();
    assert_eq!(disk["schemaVersion"], 1);
    let (host_events, rx) = mpsc::channel();
    app.host_events.replace(Some(rx));
    let name = format!("WebTools.R2.panelui.{}", webtools_ipc::new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let host_stop = stop.clone();
    let server_thread = std::thread::spawn(move || {
        let conn = server
            .accept_until(&host_stop, Duration::from_secs(10))
            .unwrap();
        let mut session = HostSession::new(conn);
        session.set_panel_handler(crate::host_runtime::panel_handler(
            host_events,
            host_stop.clone(),
        ));
        while !host_stop.load(Ordering::SeqCst) {
            if session.pump(Duration::from_millis(30)).is_err() {
                break;
            }
        }
    });
    let stage = Arc::new(AtomicUsize::new(0));
    let client_stage = stage.clone();
    let (proceed, proceed_rx) = mpsc::sync_channel(1);
    let client_root = root.clone();
    let client_thread = std::thread::spawn(move || {
        let mut client = Connection::connect(&name, Duration::from_secs(5)).unwrap();
        let mut request = |kind: &str, payload| {
            client
                .request(&Envelope::new(kind, payload), Duration::from_secs(5))
                .unwrap()
        };
        assert!(request("hello", json!({"capabilities":[CAPABILITY]})).ok);
        let before = request("super-panel-settings-get", json!({}));
        assert_eq!(before.result.unwrap()["triggerStatus"], "registered");
        let disabled = PanelSettings {
            enabled: false,
            ..target()
        };
        let updated = request(
            "super-panel-settings-update",
            serde_json::to_value(&disabled).unwrap(),
        );
        assert!(updated.ok);
        assert_eq!(updated.result.unwrap()["triggerStatus"], "disabled");
        client_stage.store(1, Ordering::SeqCst);
        proceed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let updated = request(
            "super-panel-settings-update",
            serde_json::to_value(target()).unwrap(),
        );
        assert!(updated.ok);
        assert_eq!(
            updated.result.unwrap()["activeTrigger"],
            serde_json::to_value(target()).unwrap()
        );
        client_stage.store(2, Ordering::SeqCst);
        proceed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        use std::os::windows::fs::OpenOptionsExt;
        let path = client_root.join("super-panel-state.json");
        let bytes = std::fs::read(&path).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        let failed = request(
            "super-panel-settings-update",
            serde_json::to_value(&disabled).unwrap(),
        );
        assert!(failed.ok);
        let status: PanelStatus = serde_json::from_value(failed.result.unwrap()).unwrap();
        assert_eq!(status.settings, target());
        assert_eq!(status.trigger_status, TriggerStatus::Registered);
        assert!(status.error.is_some());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        drop(lock);
        let bad = request("super-panel-settings-update", json!({"enabled":true}));
        assert!(!bad.ok);
        assert_eq!(bad.error.unwrap().code, "INVALID_PAYLOAD");
        client_stage.store(3, Ordering::SeqCst);
        proceed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            request(
                "super-panel-settings-update",
                serde_json::to_value(disabled).unwrap()
            )
            .ok
        );
        client_stage.store(4, Ordering::SeqCst);
        proceed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            request(
                "super-panel-settings-update",
                serde_json::to_value(target()).unwrap()
            )
            .ok
        );
        client_stage.store(5, Ordering::SeqCst);
        let event = client.next_event(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(event.kind, "super-panel-settings-changed");
    });
    let started = std::time::Instant::now();
    let mut ui_stage = 0;
    timer.start(slint::TimerMode::Repeated,Duration::from_millis(80),move||{
        assert!(started.elapsed()<Duration::from_secs(15),"panel settings UI roundtrip did not finish");
        match stage.load(Ordering::SeqCst){
            1 if ui_stage==0=>{
                assert_eq!(app.panel_settings.borrow().status().trigger_status,TriggerStatus::Disabled);
                app.capture();assert!(app.panel.borrow().is_none());assert!(app.launcher.window().is_visible());
                let pins=vec![("test.app".into(),"open".into(),"".into()),("test.app".into(),"second".into(),"".into())];
                assert!(app.panel_settings.borrow_mut().save_pins(&pins));assert!(!app.panel_settings.borrow().enabled());
                ui_stage=1;proceed.send(()).unwrap();
            },
            2 if ui_stage==1=>{
                let before=app.workers.borrow().len();let version=crate::native_input::panel_version();
                // Two press activations can already be queued before UI starts capture.
                app.event(AppEvent::MousePanel(version));app.event(AppEvent::MousePanel(version));
                assert!(!app.state.borrow().is_pending(),"second queued trigger must cancel pending capture, not start another");
                assert_eq!(app.workers.borrow().len(),before+1,"second trigger must not start another capture worker");
                ui_stage=2;
            },
            2 if ui_stage==2=>{
                assert!(app.panel.borrow().is_none(),"late result from cancelled capture must not reopen panel");
                app.capture();ui_stage=20;
            },
            2 if ui_stage==20=>{
                if let Some(p)=app.panel.borrow().as_ref(){assert!(p.ui.window().is_visible());}else{return;}
                assert_eq!(app.panel_settings.borrow().pins().len(),2);ui_stage=3;proceed.send(()).unwrap();
            },
            3 if ui_stage==3=>{
                assert!(app.panel.borrow().as_ref().unwrap().ui.window().is_visible());
                assert!(app.panel_settings.borrow().status().error.is_some());ui_stage=4;proceed.send(()).unwrap();
            },
            4 if ui_stage==4=>{assert!(app.panel.borrow().is_none());assert!(!app.launcher.window().is_visible());app.capture();assert!(app.panel.borrow().is_none());ui_stage=5;proceed.send(()).unwrap();},
            5 if ui_stage==5=>{assert_eq!(app.panel_settings.borrow().status().settings,target());assert_eq!(app.panel_settings.borrow().pins().len(),2);eprintln!("PASS live pipe update disables panel, configured hook re-enables, actual save failure restores previous trigger, pins retained");app.event(AppEvent::Quit);},
            _=>{}
        }
    });
    Driver {
        timer,
        threads: vec![server_thread, client_thread],
        stop,
    }
}
