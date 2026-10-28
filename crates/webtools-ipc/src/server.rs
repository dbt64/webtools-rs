//! Named-pipe server side (the role the native host plays in production).

use crate::pipe::Connection;
use std::fs::File;
use std::io;
use std::os::windows::io::FromRawHandle;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};

pub struct PipeServer {
    handle: isize,
    name: String,
}

impl PipeServer {
    /// Bound the wait for a Manager connection and allow application shutdown.
    pub fn accept_until(
        mut self,
        cancelled: &std::sync::atomic::AtomicBool,
        timeout: std::time::Duration,
    ) -> io::Result<Connection> {
        use windows_sys::Win32::System::Pipes::{SetNamedPipeHandleState, PIPE_NOWAIT};
        let mode = PIPE_READMODE_BYTE | PIPE_NOWAIT;
        if unsafe {
            SetNamedPipeHandleState(self.handle as _, &mode, std::ptr::null(), std::ptr::null())
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "host stopping"));
            }
            let connected = unsafe { ConnectNamedPipe(self.handle as _, std::ptr::null_mut()) };
            let error = io::Error::last_os_error();
            if connected != 0 || error.raw_os_error() == Some(535) {
                let mode = PIPE_READMODE_BYTE | PIPE_WAIT;
                if unsafe {
                    SetNamedPipeHandleState(
                        self.handle as _,
                        &mode,
                        std::ptr::null(),
                        std::ptr::null(),
                    )
                } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let handle = self.handle;
                self.handle = 0;
                return Ok(Connection::from_file(unsafe {
                    File::from_raw_handle(handle as _)
                }));
            }
            if !matches!(error.raw_os_error(), Some(232 | 233 | 536)) {
                return Err(error);
            }
            if std::time::Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Manager did not connect",
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    /// Creates the first instance of the pipe. Fails if the name is taken, which
    /// is exactly the guard we want: never hijack the installed pipe name.
    pub fn bind(name: &str) -> io::Result<Self> {
        let wide: Vec<u16> = format!(r"\\.\pipe\{}", name)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateNamedPipeW(
                wide.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                1,
                64 * 1024,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            handle: handle as isize,
            name: name.to_string(),
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Blocks until a client connects, then hands back a Connection.
    pub fn accept(mut self) -> io::Result<Connection> {
        let connected = unsafe { ConnectNamedPipe(self.handle as _, std::ptr::null_mut()) };
        if connected == 0 {
            let error = io::Error::last_os_error();
            // ERROR_PIPE_CONNECTED means the client won the race; that is success.
            if error.raw_os_error() != Some(535) {
                return Err(error);
            }
        }
        let handle = self.handle;
        self.handle = 0;
        let file = unsafe { File::from_raw_handle(handle as _) };
        Ok(Connection::from_file(file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::atomic::AtomicBool,
        time::{Duration, Instant},
    };
    #[test]
    fn cancellable_accept_preserves_a_real_duplex_connection() {
        let name = format!("WebTools.R1.duplex.{}", std::process::id());
        let server = PipeServer::bind(&name).unwrap();
        let client = std::thread::spawn(move || {
            let mut connection = Connection::connect(&name, Duration::from_secs(2)).unwrap();
            connection
                .send(&crate::Envelope::new(
                    "probe",
                    serde_json::json!({"value":42}),
                ))
                .unwrap();
            connection
                .poll(Duration::from_secs(2))
                .unwrap()
                .expect("reply")
        });
        let mut connection = server
            .accept_until(&AtomicBool::new(false), Duration::from_secs(2))
            .unwrap();
        assert_eq!(connection.client_process_id().unwrap(), std::process::id());
        let message = connection
            .poll(Duration::from_secs(2))
            .unwrap()
            .expect("request");
        assert_eq!(message.kind, "probe");
        connection
            .reply(&message, true, Some(message.payload.clone()))
            .unwrap();
        assert_eq!(client.join().unwrap().kind, "ack");
    }
    #[test]
    fn shutdown_cancels_a_server_with_no_manager_client() {
        let server =
            PipeServer::bind(&format!("WebTools.R1.cancel.{}", std::process::id())).unwrap();
        let started = Instant::now();
        let result = server.accept_until(&AtomicBool::new(true), Duration::from_secs(20));
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() < Duration::from_secs(1));
    }
    #[test]
    fn disconnected_manager_does_not_wait_forever() {
        let server =
            PipeServer::bind(&format!("WebTools.R1.timeout.{}", std::process::id())).unwrap();
        let started = Instant::now();
        let result = server.accept_until(&AtomicBool::new(false), Duration::from_millis(25));
        assert_eq!(result.err().unwrap().kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() >= Duration::from_millis(20));
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

impl Drop for PipeServer {
    fn drop(&mut self) {
        if self.handle != 0 {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle as _) };
            self.handle = 0;
        }
    }
}
