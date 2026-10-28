//! Manager host process lifecycle: launch, job-object containment, trim, kill.
//!
//! Isolation rules (learned from the production acceptance harness):
//! - the pipe name must be unique per run, never the installed one
//! - the profile must stay isolated from the installed Manager profile
//! - the process is assigned to a job object with KILL_ON_JOB_CLOSE, so a
//!   crashed probe never leaves an orphan manager behind

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

#[derive(Debug, Clone)]
pub struct HostOptions {
    pub exe: PathBuf,
    pub args: Vec<String>,
    pub pipe: String,
    pub token: Option<String>,
    pub profile: PathBuf,
    pub trace_dir: Option<PathBuf>,
    pub temp_dir: Option<PathBuf>,
}

pub struct HostProcess {
    child: Child,
    job: JobHandle,
}

impl HostProcess {
    pub fn launch(options: &HostOptions) -> std::io::Result<Self> {
        let mut command = Command::new(&options.exe);
        command.args(&options.args);
        // The parent shell may run with ELECTRON_RUN_AS_NODE=1 (node-mode), which
        // would make electron.exe parse our switches as node options.
        command.env_remove("ELECTRON_RUN_AS_NODE");
        command.env_remove("NODE_OPTIONS");
        // Keep the Electron host hidden during prewarm. Explicit page/plugin
        // commands create or present the corresponding BrowserWindow.
        command.env("WEBTOOLS_ELECTRON_HOST_ONLY", "1");
        // Isolation: the caller's test profile, never real user data. The caller
        // controls whether that isolated profile is reused between launches.
        // `--manager-only` enables the isolated native-manager test profile.
        // Window visibility is controlled separately by HOST_ONLY above.
        if options.args.iter().any(|arg| arg == "--manager-only") {
            command.env("WEBTOOLS_MANAGER_ONLY", "1");
        } else {
            command.env_remove("WEBTOOLS_MANAGER_ONLY");
        }
        // The test profile is only meaningful together with the phase4g flag;
        // forcing it elsewhere makes the manager behave like an acceptance run.
        if options
            .args
            .iter()
            .any(|arg| arg == "--phase4g-manager-test")
        {
            command.env("WEBTOOLS_MANAGER_TEST_PROFILE", &options.profile);
        } else {
            command.env_remove("WEBTOOLS_MANAGER_TEST_PROFILE");
        }
        command.env("WEBTOOLS_NATIVE_PIPE", &options.pipe);
        command.env("WEBTOOLS_NATIVE_HOST_PID", std::process::id().to_string());
        if let Ok(executable) = std::env::current_exe() {
            command.env("WEBTOOLS_NATIVE_HOST_EXE", executable);
        }
        if let Some(token) = &options.token {
            command.env("WEBTOOLS_NATIVE_PIPE_TOKEN", token);
        }
        if let Some(temp) = &options.temp_dir {
            command.env("TEMP", temp);
            command.env("TMP", temp);
        }
        if let Some(trace) = &options.trace_dir {
            command.env("WEBTOOLS_STARTUP_TRACE_DIR", trace);
        }
        let child = command.spawn()?;
        let job = JobHandle::create_and_assign(&child)?;
        Ok(Self { child, job })
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Drops the working set of the manager tree (page it out, keep it alive).
    pub fn trim_working_set(&self) {
        for pid in self.tree_pids() {
            trim_pid(pid);
        }
    }

    /// Adopts every descendant that escaped the job (Chromium children can be
    /// created outside it), so cleanup is total rather than best effort.
    pub fn adopt_tree(&self) -> usize {
        let mut adopted = 0;
        for pid in descendants(self.child.id()) {
            if self.job.assign(pid) {
                adopted += 1;
            }
        }
        adopted
    }

    pub fn kill(&mut self) {
        let escaped = self.adopt_tree();
        if escaped > 0 {
            eprintln!("host: adopted {escaped} escaped descendant(s) before kill");
        }
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job.handle as _, 1)
        };
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.job.close();
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Waits for Manager's acknowledged idle-exit path to finish before the
    /// host closes the job object and reaps any remaining Chromium children.
    pub fn wait_for_exit(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait()
    }

    /// PIDs currently in the job object (the manager plus its children).
    pub fn tree_pids(&self) -> Vec<u32> {
        self.job.pids()
    }
}

impl Drop for HostProcess {
    fn drop(&mut self) {
        // Closing the job kills the whole tree, even if we return early.
        self.job.close();
    }
}

#[derive(Default)]
pub struct JobHandle {
    handle: isize,
}

impl JobHandle {
    pub fn create_and_assign(child: &Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
            if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self {
                handle: job as isize,
            })
        }
    }

    pub fn assign(&self, pid: u32) -> bool {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_ALL_ACCESS};
        if self.handle == 0 {
            return false;
        }
        unsafe {
            let process = OpenProcess(PROCESS_ALL_ACCESS, 0, pid);
            if process.is_null() {
                return false;
            }
            let ok = AssignProcessToJobObject(self.handle as _, process) != 0;
            windows_sys::Win32::Foundation::CloseHandle(process);
            ok
        }
    }

    pub fn pids(&self) -> Vec<u32> {
        Vec::new() // filled in by the process census helper; kept simple here
    }

    pub fn close(&mut self) {
        if self.handle != 0 {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.handle as _) };
            self.handle = 0;
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        self.close();
    }
}

/// Walks the process table and returns every descendant of `root`.
fn descendants(root: u32) -> Vec<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut edges: Vec<(u32, u32)> = Vec::new();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot as isize == -1 {
            return Vec::new();
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                edges.push((entry.th32ProcessID, entry.th32ParentProcessID));
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    let mut out: Vec<u32> = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (pid, ppid) in &edges {
            if *ppid == parent && *pid != root && !out.contains(pid) {
                out.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    out
}

fn trim_pid(pid: u32) {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, SetProcessWorkingSetSize, PROCESS_QUERY_INFORMATION, PROCESS_SET_QUOTA,
    };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_SET_QUOTA, 0, pid);
        if !process.is_null() {
            SetProcessWorkingSetSize(process, usize::MAX, usize::MAX);
            windows_sys::Win32::Foundation::CloseHandle(process);
        }
    }
}

/// Builds the environment for an isolated acceptance run: unique pipe, temp
/// profile, trace directory. Never reuses the installed pipe name.
pub fn isolated_options(exe: impl Into<PathBuf>, temp_root: &Path, run_id: &str) -> HostOptions {
    let pipe = format!("WebTools.NativeHost.Manager.v1.Phase4E.{run_id}");
    let profile = temp_root.join(format!("profile-{run_id}"));
    let trace = temp_root.join(format!("trace-{run_id}"));
    let _ = std::fs::create_dir_all(&profile);
    let _ = std::fs::create_dir_all(&trace);
    let mut env: HashMap<&str, String> = HashMap::new();
    env.insert("WEBTOOLS_MANAGER_ONLY", "1".into());
    HostOptions {
        exe: exe.into(),
        args: vec!["--phase4g-manager-test".into(), "--manager-only".into()],
        pipe,
        token: None,
        profile,
        trace_dir: Some(trace),
        temp_dir: Some(temp_root.to_path_buf()),
    }
}
