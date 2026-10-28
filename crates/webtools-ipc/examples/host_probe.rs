use std::time::{Duration, Instant};
use webtools_ipc::{HostOptions, HostProcess, HostSession, PipeServer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let exe = args.next().expect("usage: host_probe <exe> [app-args...]");
    let app_args: Vec<String> = args.collect();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let digits = format!("{nanos:x}");
    let run_id = digits[digits.len().saturating_sub(16)..].to_string();
    let temp_root = std::env::temp_dir();
    let pipe = format!("WebTools.NativeHost.Manager.v1.Phase4E.{run_id}");
    let profile = temp_root.join(format!("wt-profile-{run_id}"));
    std::fs::create_dir_all(&profile)?;

    let server = PipeServer::bind(&pipe)?;
    println!("pipe bound: {pipe}");

    let started = Instant::now();
    let mut host = HostProcess::launch(&HostOptions {
        exe: exe.into(),
        args: app_args,
        pipe: pipe.clone(),
        token: None,
        profile,
        trace_dir: None,
        temp_dir: Some(temp_root),
    })?;
    println!("spawned pid={} after {:?}", host.pid(), started.elapsed());

    let connection = server.accept()?;
    println!("client connected after {:?}", started.elapsed());
    let mut session = match std::env::var_os("WT_STATE_ROOT") {
        Some(root) => HostSession::with_state_store(
            connection,
            webtools_ipc::state::LauncherStateStore::load(std::path::Path::new(&root))?,
        ),
        None => HostSession::new(connection),
    };

    let hello = session.wait_for_hello(Duration::from_secs(20))?;
    println!(
        "hello after {:?}: pid={} version={} caps={:?}",
        started.elapsed(),
        hello.process_id,
        hello.manager_version,
        hello.capabilities
    );

    // Host-only mode has no BrowserWindow, so renderer-ready never arrives.
    // Pump for a moment and report what the manager sends after the handshake.
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        session.pump(Duration::from_millis(100))?;
    }
    println!(
        "projection rows: {:?}",
        session.projection().map(|p| p.features.len())
    );
    println!("renderer ready: {}", session.renderer_ready);
    println!("alive after handshake: {}", host.is_running());

    if std::env::var("WT_SEQUENCE").as_deref() == Ok("1") {
        let request_id = webtools_ipc::new_request_id();
        let started_page = Instant::now();
        match session.open_page(&request_id, "favorites") {
            Ok(ack) => println!(
                "open-page ok={} after {:?} result={:?} error={:?}",
                ack.ok,
                started_page.elapsed(),
                ack.result,
                ack.error
            ),
            Err(error) => println!(
                "open-page failed after {:?}: {error}",
                started_page.elapsed()
            ),
        }
    }

    if let Ok(target) = std::env::var("WT_INVOKE") {
        let (plugin_id, feature_id) = target.split_once(':').unwrap_or((target.as_str(), ""));
        let request_id = webtools_ipc::new_request_id();
        let started_invoke = Instant::now();
        match session.feature_invoke(&request_id, plugin_id, feature_id, "launcher") {
            Ok(ack) => println!(
                "feature-invoke({plugin_id}:{feature_id}) ok={} after {:?} result={:?} error={:?}",
                ack.ok,
                started_invoke.elapsed(),
                ack.result,
                ack.error
            ),
            Err(error) => println!(
                "feature-invoke failed after {:?}: {error}",
                started_invoke.elapsed()
            ),
        }
    }

    host.kill();
    println!("killed, running={}", host.is_running());
    Ok(())
}
