use std::fs::OpenOptions;
use std::io::{self, Write};
use std::sync::mpsc;
use std::time::Duration;
use webtools_ipc::{
    encode, new_request_id, Envelope, PipeServer, ProtocolError, MAX_MESSAGE_BYTES,
};

#[test]
fn incomplete_header_and_body_survive_short_polls() {
    let name = format!("WebTools.Test.{}", new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let mut writer = OpenOptions::new()
        .write(true)
        .open(format!(r"\\.\pipe\{name}"))
        .unwrap();
    let mut connection = server.accept().unwrap();
    let first = Envelope::new("first", serde_json::json!({"message":"测试"}));
    let second = Envelope::new("second", serde_json::json!({}));
    let wire = encode(&first).unwrap();
    let (advance, next) = mpsc::channel();
    let (written, received) = mpsc::channel();
    writer.write_all(&wire[..2]).unwrap();
    let peer = std::thread::spawn(move || {
        for chunk in [&wire[2..8], &wire[8..], &encode(&second).unwrap()] {
            if next.recv_timeout(Duration::from_millis(500)).is_err() {
                return;
            }
            writer.write_all(chunk).unwrap();
            if written.send(()).is_err() {
                return;
            }
        }
        let _ = next.recv_timeout(Duration::from_secs(2));
    });

    assert!(matches!(
        connection.poll(Duration::from_millis(30)),
        Ok(None)
    ));
    advance.send(()).unwrap();
    received.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        connection.poll(Duration::from_millis(30)),
        Ok(None)
    ));
    advance.send(()).unwrap();
    let accepted = connection.poll(Duration::from_secs(2)).unwrap().unwrap();
    assert_eq!(accepted.request_id, first.request_id);
    assert_eq!(accepted.payload, first.payload);
    advance.send(()).unwrap();
    assert_eq!(
        connection
            .poll(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .kind,
        "second"
    );
    advance.send(()).unwrap();
    peer.join().unwrap();
}

#[test]
fn abandoned_frame_times_out_with_the_pipe_still_open() {
    let name = format!("WebTools.Test.{}", new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let mut writer = OpenOptions::new()
        .write(true)
        .open(format!(r"\\.\pipe\{name}"))
        .unwrap();
    let mut connection = server.accept().unwrap();
    writer.write_all(&100u32.to_le_bytes()).unwrap();
    let (finish, finished) = mpsc::channel();
    let peer = std::thread::spawn(move || {
        let _ = finished.recv_timeout(Duration::from_secs(8));
        drop(writer);
    });

    let result = connection.poll(Duration::from_secs(7));
    let _ = finish.send(());
    peer.join().unwrap();
    assert!(matches!(result, Err(ProtocolError::Io(e)) if e.kind() == io::ErrorKind::TimedOut));
}

#[test]
fn maximum_frame_followed_by_another_frame_keeps_the_stream_aligned() {
    let name = format!("WebTools.Test.{}", new_request_id());
    let server = PipeServer::bind(&name).unwrap();
    let mut writer = OpenOptions::new()
        .write(true)
        .open(format!(r"\\.\pipe\{name}"))
        .unwrap();
    let mut connection = server.accept().unwrap();
    let empty = Envelope::new("maximum", serde_json::json!({"data":""}));
    let overhead = encode(&empty).unwrap().len() - 4;
    let payload_length = MAX_MESSAGE_BYTES - overhead;
    let first = Envelope {
        payload: serde_json::json!({"data":"x".repeat(payload_length)}),
        ..empty
    };
    let mut both = encode(&first).unwrap();
    both.extend(encode(&Envelope::new("next", serde_json::json!({}))).unwrap());
    let (finish, finished) = mpsc::channel();
    let peer = std::thread::spawn(move || {
        writer.write_all(&both).unwrap();
        let _ = finished.recv_timeout(Duration::from_secs(7));
    });

    let received = connection.poll(Duration::from_secs(5)).unwrap().unwrap();
    assert_eq!(
        received.payload["data"].as_str().unwrap().len(),
        payload_length
    );
    assert_eq!(
        connection
            .poll(Duration::from_secs(2))
            .unwrap()
            .unwrap()
            .kind,
        "next"
    );
    finish.send(()).unwrap();
    peer.join().unwrap();
}
