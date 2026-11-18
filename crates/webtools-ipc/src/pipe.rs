//! Single-owner named-pipe connection with request/response correlation.
//!
//! Windows synchronous pipe handles serialise I/O on the underlying file object,
//! so a background reader thread would deadlock writes (and vice versa). We keep
//! one thread and use PeekNamedPipe to read only bytes already available.

use crate::{write_envelope, Ack, Envelope, FrameDecoder, ProtocolError, Result, MAX_MESSAGE_BYTES};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::windows::io::AsRawHandle;
use std::time::{Duration, Instant};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;

const FRAME_TIMEOUT: Duration = Duration::from_secs(5);
const READ_CHUNK_BYTES: usize = 64 * 1024;

pub struct Connection {
    file: File,
    backlog: VecDeque<Envelope>,
    decoder: FrameDecoder,
    partial_since: Option<Instant>,
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
                Ok(file) => return Ok(Self::from_file(file)),
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
            decoder: FrameDecoder::default(),
            partial_since: None,
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

    /// Waits for an envelope, preserving fragments across individual polls.
    /// A frame that remains incomplete past FRAME_TIMEOUT ends the connection.
    pub fn poll(&mut self, timeout: Duration) -> Result<Option<Envelope>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(envelope) = self.backlog.pop_front() {
                return Ok(Some(envelope));
            }
            let now = Instant::now();
            if self
                .partial_since
                .is_some_and(|since| now - since >= FRAME_TIMEOUT)
            {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "incomplete pipe frame").into());
            }
            let available = self.available()? as usize;
            if available > 0 {
                self.read_available(available, now)?;
                if !self.backlog.is_empty() {
                    continue;
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            if available == 0 {
                std::thread::sleep(remaining.min(Duration::from_millis(2)));
            }
        }
    }

    fn read_available(&mut self, available: usize, now: Instant) -> Result<()> {
        // Leave room for a full-sized frame even if a second frame is already
        // queued behind it. Only this thread consumes bytes from the handle.
        let room = MAX_MESSAGE_BYTES + 4 - self.decoder.buffered_len();
        let mut chunk = [0; READ_CHUNK_BYTES];
        let to_read = available.min(room).min(READ_CHUNK_BYTES);
        let count = self.file.read(&mut chunk[..to_read])?;
        if count == 0 {
            return Err(
                io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed while reading").into(),
            );
        }
        let messages = self.decoder.push(&chunk[..count])?;
        if self.decoder.buffered_len() == 0 {
            self.partial_since = None;
            self.decoder.trim_buffer(READ_CHUNK_BYTES);
        } else if self.partial_since.is_none() || !messages.is_empty() {
            self.partial_since = Some(now);
        }
        if std::env::var_os("WT_DUMP").is_some() {
            for envelope in &messages {
                eprintln!(
                    "[wire<] {}",
                    serde_json::to_string(envelope).unwrap_or_default()
                );
            }
        }
        self.backlog.extend(messages);
        Ok(())
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
