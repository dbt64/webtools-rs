use std::time::Duration;
use webtools_ipc::{Connection, Envelope, HostReady};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pipe = std::env::args()
        .nth(1)
        .expect("usage: handshake <pipe-name>");
    eprintln!("connecting to {pipe}");
    let mut connection = Connection::connect(&pipe, Duration::from_secs(10))?;
    eprintln!("connected");

    let ready = HostReady {
        process_id: std::process::id(),
        manager_version: "rust-probe".into(),
        capabilities: vec!["app-memory".into(), "feature-invoke".into()],
        development_authentication_token: None,
    };
    let hello = Envelope::new("electron-host-ready", serde_json::to_value(ready)?);
    eprintln!("sending hello");
    let ack = connection.request(&hello, Duration::from_secs(5))?;
    eprintln!("hello acked");
    println!("hello ok={} result={:?}", ack.ok, ack.result);

    let memory = Envelope::new("app-memory-get", serde_json::json!({}));
    let started = std::time::Instant::now();
    let ack = connection.request(&memory, Duration::from_secs(5))?;
    println!(
        "app-memory-get ok={} in {:?} -> {:?}",
        ack.ok,
        started.elapsed(),
        ack.result
    );
    Ok(())
}
