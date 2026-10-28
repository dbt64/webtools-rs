//! Shell launches that the panel performs itself (no manager round trip).

/// Percent-encodes a query the way browsers submit search terms.
fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.trim().bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Opens the default browser with the search results for the selected text.
pub fn open_search(text: &str) -> bool {
    let url = format!("https://www.bing.com/search?q={}", encode(text));
    open_url(&url)
}

pub fn open_search_template(template: &str, text: &str) -> bool {
    open_url(&template.replace("%s", &encode(text)))
}

/// Opens the URL in the user's default browser.
pub fn open_url(url: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let verb = wide("open");
    let target = wide(url);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as usize > 32
}
/// Opens a folder in Explorer.
pub fn open_folder(path: &str) -> bool {
    open_url(path)
}

pub fn open_file(path: &str) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |s: &str| -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let verb = wide("open");
    let target = wide(path);
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    result as usize > 32
}

pub fn show_file_in_folder(path: &str, is_directory: bool) -> bool {
    if is_directory {
        return open_file(path);
    }
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{path}"))
        .spawn()
        .is_ok()
}

/// Opens a terminal rooted at the folder, falling back to PowerShell.
pub fn open_terminal(path: &str) -> bool {
    use std::process::{Command, Stdio};
    let windows_terminal = Command::new("wt.exe")
        .args(["-d", path])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if windows_terminal.is_ok() {
        return true;
    }
    Command::new("powershell.exe")
        .args([
            "-NoExit",
            "-Command",
            &format!("Set-Location -LiteralPath '{path}'"),
        ])
        .stdin(Stdio::null())
        .spawn()
        .is_ok()
}
