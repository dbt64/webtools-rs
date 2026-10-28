#[cfg(test)]
mod tests {
    #[test]
    fn startup_command_quotes_executable_paths_with_spaces() {
        assert_eq!(super::startup_command(std::path::Path::new(r"C:\Program Files\WebTools\WebTools.NativeHost.Rust.exe")), r#""C:\Program Files\WebTools\WebTools.NativeHost.Rust.exe""#);
    }
}
// Windows login startup for an installed production host.
use std::{io, path::Path, ptr};
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegGetValueW, RegSetValueExW,
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn startup_command(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// A portable preview does not change the installed product's startup entry.
pub fn available() -> bool {
    if !crate::config::production_mode() { return false; }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join(".phase4f-test-install").is_file() && dir.join("Uninstall.exe").is_file() { return true; }
        }
    }
    let key = wide(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\WebToolsNative");
    let name = wide("InstallLocation");
    let mut size = 0u32;
    let first = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), ptr::null_mut(), &mut size) };
    if first != ERROR_SUCCESS || size < 2 || size > 65536 { return false; }
    let mut buffer = vec![0u16; (size as usize + 1) / 2];
    let result = unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size) };
    if result != ERROR_SUCCESS { return false; }
    let length = buffer.iter().position(|ch| *ch == 0).unwrap_or(buffer.len());
    let installed = std::fs::canonicalize(String::from_utf16_lossy(&buffer[..length]));
    let current = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).and_then(|p| std::fs::canonicalize(p).ok());
    matches!((installed, current), (Ok(a), Some(b)) if a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()))
}

pub fn enabled() -> io::Result<bool> {
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("WebTools");
    let mut size = 0u32;
    let result = unsafe { RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), ptr::null_mut(), &mut size) };
    if result == ERROR_FILE_NOT_FOUND { return Ok(false); }
    if result != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(result as i32)); }
    let mut buffer = vec![0u16; (size as usize + 1) / 2];
    let result = unsafe { RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), buffer.as_mut_ptr().cast(), &mut size) };
    if result != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(result as i32)); }
    let len = buffer.iter().position(|ch| *ch == 0).unwrap_or(buffer.len());
    Ok(String::from_utf16_lossy(&buffer[..len]).eq_ignore_ascii_case(&startup_command(&std::env::current_exe()?)))
}

pub fn apply(enabled: bool) -> io::Result<()> {
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let name = wide("WebTools");
    let mut key = ptr::null_mut();
    let result = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, ptr::null(), &mut key, ptr::null_mut()) };
    if result != ERROR_SUCCESS { return Err(io::Error::from_raw_os_error(result as i32)); }
    let result = if enabled {
        let executable = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => { unsafe { RegCloseKey(key); } return Err(error); }
        };
        let command = wide(&startup_command(&executable));
        unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, command.as_ptr().cast(), (command.len() * 2) as u32) }
    } else {
        unsafe { RegDeleteValueW(key, name.as_ptr()) }
    };
    unsafe { RegCloseKey(key); }
    if result == ERROR_SUCCESS || (!enabled && result == ERROR_FILE_NOT_FOUND) { Ok(()) }
    else { Err(io::Error::from_raw_os_error(result as i32)) }
}

