//! Everything's documented Unicode QUERY2 protocol. No command-line helper or DLL.
//! Layout source: voidtools Everything SDK ipc/everything_ipc.h (MIT).

use std::sync::{
    atomic::{AtomicU32, Ordering},
    OnceLock,
};
use std::time::{Duration, Instant};
use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};

const QUERY2W: usize = 18;
const FULL_PATH: u32 = 4;
const MAX_RESULTS: u32 = 20;
static REPLY_ID: AtomicU32 = AtomicU32::new(1);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

pub fn find(instance: Option<&str>) -> Option<usize> {
    let class = match instance {
        Some(name) => format!("EVERYTHING_TASKBAR_NOTIFICATION_({name})"),
        None => "EVERYTHING_TASKBAR_NOTIFICATION".to_string(),
    };
    let hwnd = unsafe { FindWindowW(wide(&class).as_ptr(), std::ptr::null()) };
    (!hwnd.is_null()).then_some(hwnd as usize)
}

pub fn external() -> Option<usize> {
    find(None).or_else(|| find(Some("1.5a")))
}

pub fn process_path_for_window(hwnd: usize) -> Option<std::path::PathBuf> {
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

    let mut process_id = 0;
    if unsafe { GetWindowThreadProcessId(hwnd as HWND, &mut process_id) } == 0 {
        return None;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id) };
    if process.is_null() {
        return None;
    }
    let mut path = vec![0u16; 32_768];
    let mut length = path.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length) };
    unsafe { windows_sys::Win32::Foundation::CloseHandle(process) };
    if ok == 0 {
        return None;
    }
    path.truncate(length as usize);
    Some(std::path::PathBuf::from(String::from_utf16_lossy(&path)))
}

fn control(hwnd: usize, code: usize) -> Result<usize, String> {
    let mut value = 0;
    let ok = unsafe {
        SendMessageTimeoutW(
            hwnd as HWND,
            WM_USER,
            code,
            0,
            SMTO_ABORTIFHUNG,
            1500,
            &mut value,
        )
    };
    if ok == 0 {
        Err("无法连接 Everything，请确认程序正在运行。".into())
    } else {
        Ok(value)
    }
}

pub fn ready(hwnd: usize) -> Result<bool, String> {
    control(hwnd, 401).map(|v| v != 0)
}
pub fn exit(hwnd: usize) {
    let _ = control(hwnd, 4);
}

fn query_packet(hwnd: usize, reply: u32, query: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(30 + query.len() * 2);
    // HWND is explicitly a DWORD on the wire, including 64-bit Windows.
    for value in [hwnd as u32, reply, 0, 0, MAX_RESULTS, FULL_PATH, 1] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    for unit in query.encode_utf16().chain(Some(0)) {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

fn dword(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn parse_reply(bytes: &[u8]) -> Result<Vec<String>, String> {
    let invalid = || "Everything 返回的文件结果格式无效。".to_string();
    let count = dword(bytes, 4).ok_or_else(invalid)? as usize;
    if dword(bytes, 12) != Some(FULL_PATH)
        || count > MAX_RESULTS as usize
        || bytes.len() < 20 + count * 8
    {
        return Err(invalid());
    }
    let mut paths = Vec::with_capacity(count);
    for index in 0..count {
        let offset = dword(bytes, 24 + index * 8).ok_or_else(invalid)? as usize;
        if offset < 20 + count * 8 {
            return Err(invalid());
        }
        let length = dword(bytes, offset).ok_or_else(invalid)? as usize;
        let end = offset
            .checked_add(4)
            .and_then(|v| v.checked_add(length.checked_mul(2)?))
            .ok_or_else(invalid)?;
        if dword(bytes, offset).is_none() || bytes.get(end..end.saturating_add(2)) != Some(&[0, 0])
        {
            return Err(invalid());
        }
        let text = bytes.get(offset + 4..end).ok_or_else(invalid)?;
        let units: Vec<u16> = text
            .chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]))
            .collect();
        paths.push(String::from_utf16(&units).map_err(|_| invalid())?);
    }
    Ok(paths)
}

struct ReplyState {
    id: u32,
    result: Option<Result<Vec<String>, String>>,
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: usize, lp: isize) -> isize {
    if message == WM_COPYDATA && lp != 0 {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ReplyState;
        let data = &*(lp as *const windows_sys::Win32::System::DataExchange::COPYDATASTRUCT);
        if !state.is_null() && data.dwData == (*state).id as usize {
            (*state).result = Some(if data.lpData.is_null() || data.cbData > 2_000_000 {
                Err("Everything 返回的文件结果格式无效。".into())
            } else {
                parse_reply(std::slice::from_raw_parts(
                    data.lpData as *const u8,
                    data.cbData as usize,
                ))
            });
            return 1;
        }
    }
    DefWindowProcW(hwnd, message, wp, lp)
}

struct ReplyWindow {
    hwnd: HWND,
    state: Box<ReplyState>,
}
impl Drop for ReplyWindow {
    fn drop(&mut self) {
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            DestroyWindow(self.hwnd);
        }
    }
}

pub fn query(hwnd: usize, text: &str, current: &impl Fn() -> bool) -> Result<Vec<String>, String> {
    static CLASS: OnceLock<bool> = OnceLock::new();
    let class = wide("WebTools.Everything.QueryReply");
    if !CLASS.get_or_init(|| unsafe {
        let mut wc: WNDCLASSW = std::mem::zeroed();
        wc.lpfnWndProc = Some(window_proc);
        wc.hInstance =
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
        wc.lpszClassName = class.as_ptr();
        RegisterClassW(&wc) != 0
    }) {
        return Err("无法创建文件搜索通信窗口。".into());
    }
    let receiver = unsafe {
        CreateWindowExW(
            0,
            class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        )
    };
    if receiver.is_null() {
        return Err("无法创建文件搜索通信窗口。".into());
    }
    let mut window = ReplyWindow {
        hwnd: receiver,
        state: Box::new(ReplyState {
            id: REPLY_ID.fetch_add(1, Ordering::Relaxed),
            result: None,
        }),
    };
    unsafe {
        SetWindowLongPtrW(
            receiver,
            GWLP_USERDATA,
            (&mut *window.state as *mut ReplyState) as isize,
        );
        ChangeWindowMessageFilterEx(receiver, WM_COPYDATA, MSGFLT_ALLOW, std::ptr::null_mut());
    }
    let packet = query_packet(receiver as usize, window.state.id, text);
    let data = windows_sys::Win32::System::DataExchange::COPYDATASTRUCT {
        dwData: QUERY2W,
        cbData: packet.len() as u32,
        lpData: packet.as_ptr() as *mut _,
    };
    let mut accepted = 0;
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd as HWND,
            WM_COPYDATA,
            receiver as usize,
            (&data as *const _) as isize,
            SMTO_ABORTIFHUNG,
            1500,
            &mut accepted,
        )
    };
    if sent == 0 || accepted == 0 {
        return Err("Everything 未接受文件查询，请稍后重试。".into());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if !current() {
            return Ok(Vec::new());
        }
        if let Some(result) = window.state.result.take() {
            return result;
        }
        if Instant::now() >= deadline {
            return Err("Everything 查询未及时返回，请稍后重试。".into());
        }
        unsafe {
            let mut message = std::mem::zeroed();
            while PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            MsgWaitForMultipleObjectsEx(0, std::ptr::null(), 10, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_query_uses_query2_dword_hwnd_and_full_path_only() {
        let packet = query_packet(0x12345678, 9, "中文.txt");
        assert_eq!(dword(&packet, 0), Some(0x12345678));
        assert_eq!(dword(&packet, 16), Some(20));
        assert_eq!(dword(&packet, 20), Some(FULL_PATH));
        assert_eq!(
            packet[28..],
            "中文.txt"
                .encode_utf16()
                .chain(Some(0))
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }
    #[test]
    fn reads_full_paths_from_length_prefixed_unicode_reply() {
        let path = r"D:\资料\test file.txt";
        let units: Vec<_> = path.encode_utf16().collect();
        let mut bytes = Vec::new();
        for value in [1u32, 1, 0, FULL_PATH, 1, 0, 28, units.len() as u32] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        for unit in units.into_iter().chain(Some(0)) {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(parse_reply(&bytes).unwrap(), [path]);
        bytes[24..28].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_reply(&bytes).is_err());
    }
    #[test]
    fn rejects_truncated_reply_and_wrong_requested_fields() {
        assert!(parse_reply(&[0; 19]).is_err());
        let mut bytes = vec![0; 20];
        bytes[12..16].copy_from_slice(&3u32.to_le_bytes());
        assert!(parse_reply(&bytes).is_err());
    }
}
