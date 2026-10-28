//! Blocking named-pipe client with request/response correlation.
//!
//! Windows synchronous pipe handles serialise I/O on the underlying file object,
//! so a background reader thread would deadlock writes (and vice versa). We keep
//! one thread and poll with PeekNamedPipe instead: no concurrent I/O, no
//! blocking read holding the handle while a write wants to go out.

use crate::{read_envelope, write_envelope, Ack, Envelope, ProtocolError, Result};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

pub struct Connection {
    file: File,
    backlog: VecDeque<Envelope>,
}

impl Connection {
    /// Connects to the pipe, retrying until the deadline because the manager
    /// may still be starting when we call.
    pub fn connect(pipe_name: &str, deadline: Duration) -> io::Result<Self> {
        let path = format!(r"\\.\pipe\{}", pipe_name);
        let start = Instant::now();
        let mut last_error;
        loop {
            match OpenOptions::new().read(true).write(true).open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        file,
                        backlog: VecDeque::new(),
                    })
                }
                Err(error) => last_error = error,
            }
            if start.elapsed() >= deadline {
                return Err(last_error);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn from_file(file: File) -> Self {
        Self {
            file,
            backlog: VecDeque::new(),
        }
    }

    /// The operating system's identity for the process connected to a server pipe.
    pub fn client_process_id(&self) -> io::Result<u32> {
        let mut process_id = 0;
        let ok = unsafe {
            windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId(
                self.file.as_raw_handle() as _,
                &mut process_id,
            )
        };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(process_id) }
    }

    pub fn send(&mut self, envelope: &Envelope) -> Result<()> {
        if std::env::var_os("WT_DUMP").is_some() {
            eprintln!(
                "[wire>] {}",
                serde_json::to_string(envelope).unwrap_or_default()
            );
        }
        write_envelope(&mut self.file, envelope)
    }

    /// Non-blocking-ish poll: returns the next envelope, or None when nothing
    /// arrived within the timeout.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Envelope>> {
        if let Some(envelope) = self.backlog.pop_front() {
            return Ok(Some(envelope));
        }
        let deadline = Instant::now() + timeout;
        loop {
            if self.available()? > 0 {
                if let Some(envelope) = read_envelope(&mut self.file)? {
                    if std::env::var_os("WT_DUMP").is_some() {
                        eprintln!(
                            "[wire<] {}",
                            serde_json::to_string(&envelope).unwrap_or_default()
                        );
                    }
                    return Ok(Some(envelope));
                }
                return Ok(None);
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Sends a request and waits for its matching ack envelope.
    pub fn request(&mut self, envelope: &Envelope, timeout: Duration) -> Result<Ack> {
        self.send(envelope)?;
        let deadline = Instant::now() + timeout;
        let mut deferred = VecDeque::new();
        let result = (|| {
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(ProtocolError::Malformed("ack timeout"));
                }
                let Some(message) = self.poll(remaining)? else {
                    return Err(ProtocolError::Malformed("ack timeout"));
                };
                if message.kind == "ack" {
                    let ack: Ack = serde_json::from_value(message.payload.clone())?;
                    if ack.reply_to_id == envelope.request_id && ack.reply_to_type == envelope.kind
                    {
                        return Ok(ack);
                    }
                }
                // Do not reinsert until this wait ends: poll would otherwise read
                // this same push forever and never reach the ack still on the wire.
                deferred.push_back(message);
            }
        })();
        deferred.append(&mut self.backlog);
        self.backlog = deferred;
        result
    }

    /// Next unsolicited message (server push such as plugins-update).
    pub fn next_event(&mut self, timeout: Duration) -> Result<Option<Envelope>> {
        match self.poll(timeout)? {
            Some(message) if message.kind == "ack" => {
                self.backlog.push_back(message);
                Ok(None)
            }
            other => Ok(other),
        }
    }

    pub fn reply(
        &mut self,
        request: &Envelope,
        ok: bool,
        result: Option<serde_json::Value>,
    ) -> Result<()> {
        let payload = serde_json::json!({
            "replyToId": request.request_id,
            "replyToType": request.kind,
            "ok": ok,
            "result": result,
        });
        self.send(&Envelope::new("ack", payload))
    }

    fn available(&self) -> io::Result<u32> {
        let mut bytes = 0u32;
        let ok = unsafe {
            PeekNamedPipe(
                self.file.as_raw_handle() as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut bytes,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            // 109 = ERROR_BROKEN_PIPE: the peer closed, which is different from
            // "no data yet" and must not be reported as an ack timeout.
            if error.raw_os_error() == Some(109) {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "manager closed the pipe",
                ));
            }
            return Err(error);
        }
        Ok(bytes)
    }
}
