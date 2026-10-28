//! Tray icon: a hidden top-level menu owner on its own message thread.

use crate::app::AppEvent;
use std::sync::mpsc::Sender;

const WM_TRAY: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;
const MENU_SHOW: usize = 1;
const MENU_EXIT: usize = 2;
const MENU_PANEL: usize = 3;
const MENU_SETTINGS: usize = 4;

struct TrayContext {
    events: Sender<AppEvent>,
}

pub struct Tray {
    hwnd: usize,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Tray {
    fn drop(&mut self) {
        if self.hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                    self.hwnd as _,
                    windows_sys::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    0,
                    0,
                );
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(test)]
impl Tray {
    pub(crate) fn open_test_menu(&self) {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                self.hwnd as _,
                WM_TRAY,
                1,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_RBUTTONUP as isize,
            );
        }
    }
    pub(crate) fn request_test_exit(&self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe {
            PostMessageW(self.hwnd as _, WM_CANCELMODE, 0, 0);
            PostMessageW(self.hwnd as _, WM_COMMAND, MENU_EXIT, 0);
        }
    }
}
pub fn install(events: std::sync::mpsc::Sender<crate::app::AppEvent>) -> Tray {
    let (ready, received) = std::sync::mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || unsafe {
        use windows_sys::Win32::UI::Shell::{
            Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::*;

        let class_name = wide("WebToolsSlintTray");
        let mut class: WNDCLASSW = std::mem::zeroed();
        class.lpfnWndProc = Some(window_proc);
        class.hInstance =
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        class.lpszClassName = class_name.as_ptr();
        RegisterClassW(&class);
        let context = Box::new(TrayContext { events });

        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            wide("WebTools tray").as_ptr(),
            WS_POPUP,
            -32000,
            -32000,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            class.hInstance,
            (&*context as *const TrayContext).cast(),
        );
        if hwnd.is_null() {
            let _ = ready.send(0);
            eprintln!("tray: window creation failed");
            return;
        }

        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        let icon_path = std::env::current_exe().ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("app.ico")));
        let icon: HICON = icon_path.as_ref().map(|path| {
            LoadImageW(std::ptr::null_mut(), wide(&path.to_string_lossy()).as_ptr(), IMAGE_ICON, 0, 0, LR_LOADFROMFILE | LR_DEFAULTSIZE) as _
        }).unwrap_or(std::ptr::null_mut());
        let owns_icon = !icon.is_null();
        data.hIcon = if owns_icon { icon } else { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) };
        for (slot, character) in data.szTip.iter_mut().zip("WebTools".encode_utf16()) {
            *slot = character;
        }
        let added = Shell_NotifyIconW(NIM_ADD, &data);
        let _ = ready.send(hwnd as usize);
        if std::env::var("WT_TRAY_DEBUG").is_ok() {
            eprintln!("tray: add={} hwnd={hwnd:?}", added != 0);
        }
        if added == 0 {
            if owns_icon { DestroyIcon(icon); }
            DestroyWindow(hwnd);
            return;
        }

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        Shell_NotifyIconW(NIM_DELETE, &data);
        if owns_icon { DestroyIcon(icon); }
        DestroyWindow(hwnd);
    });
    Tray {
        hwnd: received.recv().unwrap_or(0),
        thread: Some(thread),
    }
}

unsafe extern "system" fn window_proc(
    hwnd: windows_sys::Win32::Foundation::HWND,
    message: u32,
    wparam: windows_sys::Win32::Foundation::WPARAM,
    lparam: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::Win32::Foundation::LRESULT {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    match message {
        WM_NCCREATE if lparam != 0 => {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            1
        }
        WM_TRAY | WM_COMMAND => {
            let context = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TrayContext;
            if !context.is_null() {
                if message == WM_COMMAND {
                    dispatch_menu(&(*context).events, wparam & 0xffff);
                } else {
                    let event = lparam as u32;
                    if event == WM_LBUTTONUP {
                        let _ = (*context).events.send(AppEvent::ToggleLauncher);
                    } else if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                        show_menu(hwnd, &(*context).events);
                    }
                }
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn dispatch_menu(events: &Sender<AppEvent>, command: usize) {
    let event = match command {
        MENU_SHOW => AppEvent::ToggleLauncher,
        MENU_PANEL => AppEvent::TrayPanel,
        MENU_SETTINGS => AppEvent::TraySettings,
        MENU_EXIT => AppEvent::Quit,
        _ => return,
    };
    let _ = events.send(event);
}

unsafe fn show_menu(hwnd: windows_sys::Win32::Foundation::HWND, events: &Sender<AppEvent>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    AppendMenuW(menu, MF_STRING, MENU_SETTINGS, wide("设置").as_ptr());
    AppendMenuW(menu, MF_STRING, MENU_SHOW, wide("显示 / 隐藏").as_ptr());
    AppendMenuW(menu, MF_STRING, MENU_PANEL, wide("快捷面板").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
    AppendMenuW(menu, MF_STRING, MENU_EXIT, wide("退出").as_ptr());
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    // The owner must be activatable for menu keyboard input. Keep its 1px
    // tool window off-screen; it is never a taskbar surface.
    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    SetForegroundWindow(hwnd);
    let command = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY,
        point.x,
        point.y,
        0,
        hwnd,
        std::ptr::null(),
    ) as usize;
    DestroyMenu(menu);
    ShowWindow(hwnd, SW_HIDE);
    PostMessageW(hwnd, WM_NULL, 0, 0);
    dispatch_menu(events, command);
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "briefly owns a test tray icon; no existing application's icon is modified"]
    fn sent_tray_callback_reaches_the_application() {
        let (events, receiver) = std::sync::mpsc::channel();
        let tray = install(events);
        assert_ne!(tray.hwnd, 0);
        assert!(
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetParent(tray.hwnd as _) }
                .is_null(),
            "A tray popup needs an ordinary hidden top-level owner, not a message-only window"
        );
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                tray.hwnd as _,
                WM_TRAY,
                1,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_LBUTTONUP as isize,
            );
        }
        assert!(
            matches!(
                receiver.recv_timeout(std::time::Duration::from_millis(100)),
                Ok(crate::app::AppEvent::ToggleLauncher)
            ),
            "Shell-delivered callback must reach the application through WndProc"
        );
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW(
                tray.hwnd as _,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_COMMAND,
                MENU_EXIT,
                0,
            );
        }
        assert!(matches!(
            receiver.recv_timeout(std::time::Duration::from_millis(100)),
            Ok(crate::app::AppEvent::Quit)
        ));
    }
}
