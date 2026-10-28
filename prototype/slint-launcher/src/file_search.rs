//! Native Everything IPC, with an owned portable engine when none is running.
mod helper;
mod ipc;

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, Instant};

#[cfg(test)]
#[path = "file_search_tests.rs"]
mod tests;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
static OWNED: Mutex<Option<OwnedEngine>> = Mutex::new(None);
static STARTING: AtomicBool = AtomicBool::new(false);
static STOPPING: AtomicBool = AtomicBool::new(false);
struct OwnedEngine {
    process: EngineProcess,
    instance: String,
    executable: PathBuf,
}

enum EngineProcess {
    Normal(Child),
    Elevated(OwnedHandle),
}
impl EngineProcess {
    fn running(&mut self) -> bool {
        match self {
            Self::Normal(child) => child.try_wait().ok().flatten().is_none(),
            Self::Elevated(handle) => unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(
                    handle.as_raw_handle() as _,
                    0,
                ) == windows_sys::Win32::Foundation::WAIT_TIMEOUT
            },
        }
    }
    fn terminate(&mut self) {
        match self {
            Self::Normal(child) => {
                let _ = child.kill();
                let _ = child.wait();
            }
            Self::Elevated(handle) => unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(
                    handle.as_raw_handle() as _,
                    0,
                );
                windows_sys::Win32::System::Threading::WaitForSingleObject(
                    handle.as_raw_handle() as _,
                    1000,
                );
            },
        }
    }
}

impl OwnedEngine {
    fn start(
        exe: &Path,
        instance: String,
        ini: &Path,
        database: &Path,
        elevated: bool,
    ) -> Result<Self, String> {
        let process = if elevated {
            // ShellExecute returns the actual elevated process handle. Letting
            // Everything relaunch itself would leave us owning only its launcher.
            use windows_sys::Win32::{System::Com::*, UI::Shell::*};
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let host = std::env::current_exe().map_err(|e| e.to_string())?;
            let file = wide(&host.to_string_lossy());
            let args = wide(&format!(
                "--file-index-helper {} \"{}\" {instance} \"{}\" \"{}\"",
                std::process::id(),
                exe.display(),
                ini.display(),
                database.display()
            ));
            let verb = wide("runas");
            unsafe {
                let initialized =
                    CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
                let mut info: SHELLEXECUTEINFOW = std::mem::zeroed();
                info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
                info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI;
                info.lpVerb = verb.as_ptr();
                info.lpFile = file.as_ptr();
                info.lpParameters = args.as_ptr();
                info.nShow = 0;
                let ok = ShellExecuteExW(&mut info);
                let error = std::io::Error::last_os_error();
                if initialized {
                    CoUninitialize();
                }
                if ok == 0 || info.hProcess.is_null() {
                    return Err(format!(
                        "未能启动文件索引引擎，请完成系统授权后重试：{error}"
                    ));
                }
                EngineProcess::Elevated(OwnedHandle::from_raw_handle(info.hProcess as _))
            }
        } else {
            EngineProcess::Normal(
                Command::new(exe)
                    .args(["-startup", "-instance", &instance, "-config"])
                    .arg(ini)
                    .arg("-db")
                    .arg(database)
                    .creation_flags(CREATE_NO_WINDOW)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| format!("无法启动 Everything：{e}"))?,
            )
        };
        Ok(Self {
            process,
            instance,
            executable: exe.to_path_buf(),
        })
    }
}

impl Drop for OwnedEngine {
    fn drop(&mut self) {
        if let Some(hwnd) = ipc::find(Some(&self.instance)) {
            ipc::exit(hwnd);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if !self.process.running() && ipc::find(Some(&self.instance)).is_none() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        // This is the Child handle we created, not an executable-name process kill.
        self.process.terminate();
    }
}

#[derive(Clone, Debug)]
pub struct FileHit {
    pub path: String,
    pub name: String,
    pub location: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileCategory {
    All,
    Folder,
    Application,
    Document,
    Image,
    Video,
    Audio,
    Archive,
    Other,
}

impl FileCategory {
    pub const ALL: [Self; 9] = [
        Self::All,
        Self::Folder,
        Self::Application,
        Self::Document,
        Self::Image,
        Self::Video,
        Self::Audio,
        Self::Archive,
        Self::Other,
    ];

    pub fn from_index(index: i32) -> Self {
        Self::ALL
            .get(index.max(0) as usize)
            .copied()
            .unwrap_or(Self::All)
    }

    pub fn index(self) -> i32 {
        Self::ALL
            .iter()
            .position(|category| *category == self)
            .unwrap_or(0) as i32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Folder => "文件夹",
            Self::Application => "应用程序",
            Self::Document => "文档",
            Self::Image => "图片",
            Self::Video => "视频",
            Self::Audio => "音频",
            Self::Archive => "压缩文件",
            Self::Other => "其他",
        }
    }

    pub fn classify(path: &str, is_directory: bool) -> Self {
        if is_directory {
            return Self::Folder;
        }
        let extension = Path::new(path)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let contains = |extensions: &[&str]| extensions.contains(&extension.as_str());
        if contains(&["exe", "msi", "msix", "appx", "lnk"]) {
            Self::Application
        } else if contains(&[
            "txt", "md", "markdown", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "rtf",
            "odt", "ods", "odp", "csv", "log", "epub",
        ]) {
            Self::Document
        } else if contains(&[
            "png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "svg", "ico", "heic", "avif",
        ]) {
            Self::Image
        } else if contains(&[
            "mp4", "m4v", "mkv", "mov", "avi", "wmv", "webm", "mpeg", "mpg", "flv", "ts",
        ]) {
            Self::Video
        } else if contains(&[
            "mp3", "wav", "flac", "aac", "m4a", "ogg", "opus", "wma", "aiff", "mid", "midi",
        ]) {
            Self::Audio
        } else if contains(&[
            "zip", "rar", "7z", "tar", "gz", "bz2", "xz", "cab", "iso", "tgz", "zst",
        ]) {
            Self::Archive
        } else {
            Self::Other
        }
    }
}

pub struct SearchOutput {
    pub hits: Vec<FileHit>,
    pub engine_source: String,
}

pub fn perform_file_operation(path: &str, operation: i32) -> Result<(), &'static str> {
    let item = Path::new(path);
    let is_directory = item.is_dir();
    if !item.is_absolute() || (!is_directory && !item.is_file()) {
        return Err("此项目已不存在或无法访问，请重新搜索。");
    }
    match operation {
        0 if crate::shell::open_file(path) => Ok(()),
        1 if crate::shell::show_file_in_folder(path, is_directory) => Ok(()),
        2 if crate::clipboard::set_file(path) => Ok(()),
        3 if crate::clipboard::set_text(path) => Ok(()),
        4 => {
            let parent = item.parent().unwrap_or(item).to_string_lossy();
            if crate::clipboard::set_text(&parent) {
                Ok(())
            } else {
                Err("无法写入剪贴板，请重试。")
            }
        }
        0 | 1 => Err("无法完成此操作，请检查项目是否仍可访问。"),
        2 | 3 => Err("无法写入剪贴板，请重试。"),
        _ => Err("不支持此文件操作。"),
    }
}

#[cfg(test)]
pub fn search(configured_path: &str, query: &str) -> Result<Vec<FileHit>, String> {
    search_with_progress(configured_path, query, || true, |_| {}).map(|output| output.hits)
}

pub fn search_with_progress(
    configured_path: &str,
    query: &str,
    current: impl Fn() -> bool,
    progress: impl Fn(&str),
) -> Result<SearchOutput, String> {
    let query = query.trim();
    if query.is_empty() || !current() {
        return Ok(SearchOutput {
            hits: Vec::new(),
            engine_source: String::new(),
        });
    }
    if query.encode_utf16().count() > 300 || query.chars().any(char::is_control) {
        return Err("文件搜索关键词太长或包含无效字符。".into());
    }
    let Some(endpoint) = prepare(configured_path, &current, &progress)? else {
        return Ok(SearchOutput {
            hits: Vec::new(),
            engine_source: String::new(),
        });
    };
    if !ipc::ready(endpoint.hwnd)? {
        progress("Everything 正在建立文件索引，完成后自动搜索…");
        while !ipc::ready(endpoint.hwnd)? {
            if !current() {
                return Ok(SearchOutput {
                    hits: Vec::new(),
                    engine_source: String::new(),
                });
            }
            std::thread::sleep(Duration::from_millis(120));
        }
    }
    if !current() {
        return Ok(SearchOutput {
            hits: Vec::new(),
            engine_source: String::new(),
        });
    }
    progress("正在搜索文件…");
    let paths = ipc::query(endpoint.hwnd, query, &current)?;
    let hits = paths
        .into_iter()
        .filter_map(|path| {
            let full = PathBuf::from(&path);
            if !full.is_absolute() || (!full.is_file() && !full.is_dir()) {
                return None;
            }
            let name = full
                .file_name()
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let location = full
                .parent()
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .into_owned();
            Some(FileHit {
                path,
                name,
                location,
            })
        })
        .collect();
    Ok(SearchOutput {
        hits,
        engine_source: endpoint.description,
    })
}

struct SearchEndpoint {
    hwnd: usize,
    description: String,
}

fn prepare(
    configured_path: &str,
    current: &impl Fn() -> bool,
    progress: &impl Fn(&str),
) -> Result<Option<SearchEndpoint>, String> {
    let mut deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if !current() || STOPPING.load(Ordering::SeqCst) {
            return Ok(None);
        }
        // Sharing an existing index does not confer ownership of its process.
        if let Some(hwnd) = ipc::external() {
            let path = ipc::process_path_for_window(hwnd);
            let description = path
                .map(|path| format!("已连接运行中的 Everything · {}", path.display()))
                .unwrap_or_else(|| "已连接运行中的 Everything".into());
            return Ok(Some(SearchEndpoint { hwnd, description }));
        }
        let mut owned = OWNED
            .lock()
            .map_err(|_| "文件搜索服务状态不可用。".to_string())?;
        if let Some(engine) = owned.as_mut() {
            if let Some(hwnd) = ipc::find(Some(&engine.instance)) {
                return Ok(Some(SearchEndpoint {
                    hwnd,
                    description: format!("WebTools 独立引擎 · {}", engine.executable.display()),
                }));
            }
            if !engine.process.running() {
                let dead = owned.take();
                drop(owned);
                drop(dead);
                continue;
            }
        }
        if owned.is_none()
            && STARTING
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            drop(owned);
            struct Starting;
            impl Drop for Starting {
                fn drop(&mut self) {
                    STARTING.store(false, Ordering::SeqCst);
                }
            }
            let _starting = Starting;
            let exe = find_executable(configured_path).ok_or_else(|| {
                "没有找到 Everything 文件索引引擎，请使用包含 tools/everything 的完整验收包。"
                    .to_string()
            })?;
            let data = crate::config::profile_root().join("file-search");
            std::fs::create_dir_all(&data).map_err(|e| format!("无法创建文件索引目录：{e}"))?;
            let ini = data.join("Everything.ini");
            let existing = std::fs::read_to_string(&ini).ok();
            std::fs::write(&ini, engine_config(existing.as_deref()))
                .map_err(|e| format!("无法保存文件索引配置：{e}"))?;
            let instance = format!("WebToolsFileSearch_{}", std::process::id());
            progress("正在启动文件索引引擎；首次读取全盘索引可能需要系统授权…");
            // Do not hold the shutdown mutex while consent waits for a human.
            let engine = OwnedEngine::start(
                &exe,
                instance,
                &ini,
                &data.join("Everything.db"),
                !service_running(),
            )?;
            if !current() || STOPPING.load(Ordering::SeqCst) {
                drop(engine);
                return Ok(None);
            }
            let mut owned = OWNED
                .lock()
                .map_err(|_| "文件搜索服务状态不可用。".to_string())?;
            if STOPPING.load(Ordering::SeqCst) || !current() {
                drop(owned);
                drop(engine);
                return Ok(None);
            }
            *owned = Some(engine);
            deadline = Instant::now() + Duration::from_secs(30);
            continue;
        }
        drop(owned);
        if Instant::now() >= deadline {
            return Err("Everything 尚未启动，请完成首次系统授权后重试。".into());
        }
        std::thread::sleep(Duration::from_millis(80));
    }
}

fn find_executable(configured: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        // Older profiles stored es.exe. It is only a location hint, never executed.
        candidates.push(
            if path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("es.exe"))
            {
                path.with_file_name("Everything.exe")
            } else {
                path
            },
        );
    }
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(variable) {
            candidates.push(
                PathBuf::from(root)
                    .join("Everything")
                    .join("Everything.exe"),
            );
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("tools/everything/Everything.exe"));
        }
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/everything/Everything.exe"),
    );
    candidates.into_iter().find(|path| {
        path.is_absolute()
            && path.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case("Everything.exe")
            })
            && path.is_file()
    })
}

fn engine_config(existing: Option<&str>) -> String {
    // A named instance isolates settings/index files from the user's Everything.
    // Elevation is decided on each launch. Disable engine self-relaunch so we
    // keep the final process handle, while preserving saved volume/index settings.
    let default =
        "[Everything]\r\napp_data=0\r\nshow_tray_icon=0\r\ncheck_for_updates_on_startup=0\r\n";
    let mut lines: Vec<String> = existing
        .unwrap_or(default)
        .lines()
        .filter(|line| {
            !line
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("run_as_admin=")
        })
        .map(str::to_string)
        .collect();
    let section = lines
        .iter()
        .position(|line| line.trim().eq_ignore_ascii_case("[Everything]"))
        .unwrap_or_else(|| {
            lines.insert(0, "[Everything]".into());
            0
        });
    lines.insert(section + 1, "run_as_admin=0".into());
    lines.join("\r\n") + "\r\n"
}

fn service_running() -> bool {
    use windows_sys::Win32::System::Services::*;
    unsafe {
        let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
        if manager.is_null() {
            return false;
        }
        let name: Vec<_> = "Everything".encode_utf16().chain(Some(0)).collect();
        let service = OpenServiceW(manager, name.as_ptr(), SERVICE_QUERY_STATUS);
        let mut status: SERVICE_STATUS = std::mem::zeroed();
        let running = !service.is_null()
            && QueryServiceStatus(service, &mut status) != 0
            && status.dwCurrentState == SERVICE_RUNNING;
        if !service.is_null() {
            CloseServiceHandle(service);
        }
        CloseServiceHandle(manager);
        running
    }
}

/// Only the named engine started by this host is stopped. Shared installations stay open.
pub fn shutdown() {
    STOPPING.store(true, Ordering::SeqCst);
    let engine = OWNED.lock().ok().and_then(|mut engine| engine.take());
    drop(engine);
}

pub fn startup_pending() -> bool {
    STARTING.load(Ordering::SeqCst)
}
pub fn run_helper() -> Result<(), String> {
    helper::run()
}
