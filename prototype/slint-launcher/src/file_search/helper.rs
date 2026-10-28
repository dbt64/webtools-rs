//! Minimal elevated branch: no Slint initialization, only engine lifetime.
use super::*;

pub(super) fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    if args.len() != 5 {
        return Err("文件索引助手参数无效。".into());
    }
    let parent = args[0]
        .parse::<u32>()
        .map_err(|_| "文件索引宿主无效。".to_string())?;
    supervise(
        parent,
        Path::new(&args[1]),
        args[2].clone(),
        Path::new(&args[3]),
        Path::new(&args[4]),
    )
}

pub(super) fn supervise(
    parent_id: u32,
    exe: &Path,
    instance: String,
    ini: &Path,
    database: &Path,
) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::*,
        System::{JobObjects::*, Threading::*},
    };
    unsafe {
        let handle = OpenProcess(PROCESS_SYNCHRONIZE, 0, parent_id);
        if handle.is_null() {
            return Ok(());
        }
        let parent = OwnedHandle::from_raw_handle(handle as _);
        // Consent may have completed after the native host was already closed.
        if WaitForSingleObject(parent.as_raw_handle() as _, 0) != WAIT_TIMEOUT {
            return Ok(());
        }
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let job = OwnedHandle::from_raw_handle(job as _);
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job.as_raw_handle() as _,
            JobObjectExtendedLimitInformation,
            (&limits as *const _) as _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut engine = OwnedEngine::start(exe, instance, ini, database, false)?;
        let EngineProcess::Normal(child) = &engine.process else {
            unreachable!()
        };
        if AssignProcessToJobObject(job.as_raw_handle() as _, child.as_raw_handle() as _) == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        while WaitForSingleObject(parent.as_raw_handle() as _, 100) == WAIT_TIMEOUT
            && engine.process.running()
        {}
        drop(engine);
        drop(job);
        Ok(())
    }
}
