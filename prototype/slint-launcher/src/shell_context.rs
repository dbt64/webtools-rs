//! Reads the Explorer selection of the foreground window through the shell.

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView, IFolderView2, IShellBrowser, IShellFolder, IShellItemArray, IShellView,
    IShellWindows, SID_STopLevelBrowser, ShellWindows, StrRetToBufW, SHGDN_FORPARSING,
    SIGDN_FILESYSPATH, SWC_DESKTOP, SWFO_NEEDDISPATCH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowTextW,
    GetWindowThreadProcessId, IsChild, GA_ROOT,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShellKind {
    Folder,
    Files,
    Explorer,
    DesktopEmpty,
    TooManyItems,
}

const MAX_SELECTION_ITEMS: u32 = 64;

enum SelectedPaths {
    Items(Vec<String>),
    TooMany,
}

#[derive(Debug, Clone)]
pub struct ShellContext {
    pub kind: ShellKind,
    pub folder_path: String,
    pub folder_name: String,
    pub items: Vec<String>,
}

fn window_class(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn file_name(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or(path).to_string()
}

fn is_dir(path: &str) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

fn item_paths(view: &IShellView) -> SelectedPaths {
    let Ok(view2) = view.cast::<IFolderView2>() else {
        return SelectedPaths::Items(Vec::new());
    };
    // An empty selection is the current Explorer folder, not a selected folder.
    // Match WPF's SelectedItems() behavior and let folder_path() represent that view.
    let Ok(array) = (unsafe { view2.GetSelection(false) }) else {
        return SelectedPaths::Items(Vec::new());
    };
    paths_from_array(&array)
}

fn paths_from_array(array: &IShellItemArray) -> SelectedPaths {
    let Ok(count) = (unsafe { array.GetCount() }) else {
        return SelectedPaths::Items(Vec::new());
    };
    if count > MAX_SELECTION_ITEMS {
        return SelectedPaths::TooMany;
    }
    let mut paths = Vec::new();
    for index in 0..count {
        let Ok(item) = (unsafe { array.GetItemAt(index) }) else {
            continue;
        };
        let Ok(name) = (unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }) else {
            continue;
        };
        if name.is_null() {
            continue;
        }
        let text = unsafe { name.to_string() }.unwrap_or_default();
        unsafe { CoTaskMemFree(Some(name.0 as *const core::ffi::c_void)) };
        if !text.is_empty() {
            paths.push(text);
        }
    }
    SelectedPaths::Items(paths)
}

fn folder_path(view: &IShellView) -> String {
    let Ok(view) = view.cast::<IFolderView>() else {
        return String::new();
    };
    let Ok(folder) = (unsafe { view.GetFolder::<IShellFolder>() }) else {
        return String::new();
    };
    let mut strret = windows::Win32::UI::Shell::Common::STRRET::default();
    if unsafe { folder.GetDisplayNameOf(std::ptr::null(), SHGDN_FORPARSING, &mut strret) }.is_err()
    {
        return String::new();
    }
    let mut buf = [0u16; 512];
    unsafe { StrRetToBufW(&mut strret, None, &mut buf) };
    let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn init_com() {
    use windows::Win32::System::Com::{CoIncrementMTAUsage, COINIT_MULTITHREADED};
    unsafe {
        let single = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if single.is_err() {
            let multi = CoInitializeEx(None, COINIT_MULTITHREADED);
            if multi.is_err() {
                let _ = CoIncrementMTAUsage();
            }
            if std::env::var("WT_SHELL_PROBE").is_ok() {
                eprintln!("shell: com sta={single:?} mta={multi:?}");
            }
        }
    }
}

/// Captures the selection of the foreground Explorer window, if any.
pub fn capture() -> Option<ShellContext> {
    init_com();
    let foreground = unsafe { GetForegroundWindow() };
    capture_from(foreground)
}

/// Captures the selection of a specific Explorer window.
pub fn capture_from(foreground: HWND) -> Option<ShellContext> {
    init_com();
    if foreground.0.is_null() {
        return None;
    }
    let class = window_class(foreground);
    if std::env::var("WT_SHELL_PROBE").is_ok() {
        eprintln!("shell: probe hwnd={foreground:?} class={class:?}");
    }
    if matches!(
        class.as_str(),
        "Progman" | "WorkerW" | "SHELLDLL_DefView" | "SysListView32"
    ) {
        return capture_desktop(foreground);
    }
    if class != "CabinetWClass" && class != "ExploreWClass" {
        return None;
    }
    let title = window_title(foreground);
    let collection: IShellWindows =
        match unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) } {
            Ok(collection) => collection,
            Err(error) => {
                eprintln!("shell: CoCreateInstance(ShellWindows) failed: {error}");
                return None;
            }
        };
    let count = match unsafe { collection.Count() } {
        Ok(count) => count,
        Err(error) => {
            eprintln!("shell: Count failed: {error}");
            return None;
        }
    };
    if std::env::var("WT_SHELL_PROBE").is_ok() {
        eprintln!("shell: {count} shell windows, target={foreground:?} class={class}");
    }
    for index in 0..count {
        let variant = VARIANT::from(index);
        let Ok(dispatch) = (unsafe { collection.Item(&variant) }) else {
            eprintln!("shell: item {index} dispatch failed");
            continue;
        };
        let Ok(provider) = dispatch.cast::<windows::Win32::System::Com::IServiceProvider>() else {
            eprintln!("shell: item {index} no IServiceProvider");
            continue;
        };
        let Ok(browser) =
            (unsafe { provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) })
        else {
            eprintln!("shell: item {index} no IShellBrowser");
            continue;
        };
        let Ok(hwnd) = (unsafe { browser.GetWindow() }) else {
            eprintln!("shell: item {index} no HWND");
            continue;
        };
        if std::env::var("WT_SHELL_PROBE").is_ok() {
            eprintln!("shell: item {index} hwnd={hwnd:?}");
        }
        // Win11 tabs report the tab host, so compare against the top-level ancestor too.
        let same_window = hwnd == foreground
            || unsafe { GetAncestor(hwnd, GA_ROOT) } == foreground
            || unsafe { IsChild(foreground, hwnd) }.as_bool();
        if !same_window {
            continue;
        }
        let Ok(view) = (unsafe { browser.QueryActiveShellView() }) else {
            continue;
        };
        let paths = match item_paths(&view) {
            SelectedPaths::Items(paths) => paths,
            SelectedPaths::TooMany => return Some(too_many_items_context()),
        };
        let directory = folder_path(&view);
        let name = if directory.is_empty() {
            title
        } else {
            file_name(&directory)
        };
        let kind = if paths.is_empty() {
            ShellKind::Explorer
        } else if paths.iter().all(|path| is_dir(path)) {
            ShellKind::Folder
        } else {
            ShellKind::Files
        };
        // Keep absolute paths for invocation and classification. The UI derives
        // display names from these paths in PanelController.
        let items = paths;
        if std::env::var("WT_SHELL_PROBE").is_ok() {
            eprintln!("shell: captured kind={kind:?} items={}", items.len());
        }
        return Some(ShellContext {
            kind,
            folder_path: directory,
            folder_name: name,
            items,
        });
    }
    None
}

/// Reads selected filesystem icons from the actual Desktop ShellView. The
/// desktop is a selection source; an empty desktop selection returns a board
/// context so it is not mistaken for an ordinary application's text selection.
fn capture_desktop(foreground: HWND) -> Option<ShellContext> {
    let root = unsafe { GetAncestor(foreground, GA_ROOT) };
    let root_class = window_class(root);
    if !matches!(root_class.as_str(), "Progman" | "WorkerW") {
        return None;
    }

    let shell = unsafe { GetShellWindow() };
    let mut shell_process = 0;
    let mut foreground_process = 0;
    unsafe {
        GetWindowThreadProcessId(shell, Some(&mut shell_process));
        GetWindowThreadProcessId(foreground, Some(&mut foreground_process));
    }
    if shell_process == 0 || shell_process != foreground_process {
        return None;
    }

    let collection: IShellWindows =
        match unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) } {
            Ok(collection) => collection,
            Err(_) => return Some(desktop_empty_context()),
        };
    let location = VARIANT::from(0);
    let location_root = VARIANT::default();
    let mut desktop_hwnd = 0i32;
    let dispatch = match unsafe {
        collection.FindWindowSW(
            &location,
            &location_root,
            SWC_DESKTOP,
            &mut desktop_hwnd,
            SWFO_NEEDDISPATCH,
        )
    } {
        Ok(dispatch) => dispatch,
        Err(_) => return Some(desktop_empty_context()),
    };
    let Ok(provider) = dispatch.cast::<windows::Win32::System::Com::IServiceProvider>() else {
        return Some(desktop_empty_context());
    };
    let Ok(browser) = (unsafe { provider.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) })
    else {
        return Some(desktop_empty_context());
    };
    let Ok(view) = (unsafe { browser.QueryActiveShellView() }) else {
        return Some(desktop_empty_context());
    };
    let Ok(view_hwnd) = (unsafe { view.GetWindow() }) else {
        return Some(desktop_empty_context());
    };
    let view_root = unsafe { GetAncestor(view_hwnd, GA_ROOT) };
    if window_class(view_hwnd) != "SHELLDLL_DefView"
        || !matches!(window_class(view_root).as_str(), "Progman" | "WorkerW")
        || unsafe { GetWindowThreadProcessId(view_hwnd, Some(&mut foreground_process)) } == 0
        || foreground_process != shell_process
    {
        return Some(desktop_empty_context());
    }

    let paths = match view
        .cast::<IFolderView2>()
        .and_then(|folder| unsafe { folder.GetSelection(false) })
    {
        Ok(array) => match paths_from_array(&array) {
            SelectedPaths::Items(paths) => paths,
            SelectedPaths::TooMany => return Some(too_many_items_context()),
        },
        Err(_) => Vec::new(),
    };
    if paths.is_empty() {
        return Some(desktop_empty_context());
    }
    let kind = if paths.iter().all(|path| is_dir(path)) {
        ShellKind::Folder
    } else {
        ShellKind::Files
    };
    Some(ShellContext {
        kind,
        folder_path: String::new(),
        folder_name: String::new(),
        items: paths,
    })
}

fn desktop_empty_context() -> ShellContext {
    ShellContext {
        kind: ShellKind::DesktopEmpty,
        folder_path: String::new(),
        folder_name: String::new(),
        items: Vec::new(),
    }
}

fn too_many_items_context() -> ShellContext {
    ShellContext {
        kind: ShellKind::TooManyItems,
        folder_path: String::new(),
        folder_name: String::new(),
        items: Vec::new(),
    }
}
