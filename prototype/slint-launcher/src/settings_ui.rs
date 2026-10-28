//! OS theme observation and WPF preference resolution. Registry access is read-only.
pub fn effective_light(preference: &str, system_light: bool) -> bool {
    match preference {
        "light" => true,
        "dark" => false,
        _ => system_light,
    }
}
pub fn system_light() -> bool {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD};
    let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme".encode_utf16().chain(Some(0)).collect();
    let mut value = 0u32;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        ) == 0
            && value == 1
    }
}
pub struct ThemeObserver(windows_sys::Win32::UI::WindowsAndMessaging::HHOOK);
impl ThemeObserver {
    pub fn install() -> Self {
        use windows_sys::Win32::{
            System::Threading::GetCurrentThreadId,
            UI::WindowsAndMessaging::{SetWindowsHookExW, WH_CALLWNDPROC},
        };
        Self(unsafe {
            SetWindowsHookExW(
                WH_CALLWNDPROC,
                Some(theme_message),
                std::ptr::null_mut(),
                GetCurrentThreadId(),
            )
        })
    }
}
impl Drop for ThemeObserver {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::UnhookWindowsHookEx(self.0);
            }
        }
    }
}
unsafe extern "system" fn theme_message(code: i32, wparam: usize, lparam: isize) -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, CWPSTRUCT, WM_SETTINGCHANGE, WM_THEMECHANGED,
    };
    if code >= 0 && lparam != 0 {
        let message = &*(lparam as *const CWPSTRUCT);
        if matches!(message.message, WM_SETTINGCHANGE | WM_THEMECHANGED) {
            crate::native_input::notify_theme_change();
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}
#[cfg(test)]
mod tests {
    #[test]
    fn explicit_preference_wins_and_system_follows_windows() {
        assert!(super::effective_light("light", false));
        assert!(!super::effective_light("dark", true));
        assert!(super::effective_light("system", true));
        assert!(!super::effective_light("system", false));
    }
}
