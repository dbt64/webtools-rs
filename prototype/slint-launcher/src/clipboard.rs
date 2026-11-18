//! Minimal clipboard writer (CF_UNICODETEXT) for the result card.

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;

#[repr(C)]
struct DropFiles {
    offset: u32,
    x: i32,
    y: i32,
    non_client_area: i32,
    wide: i32,
}

pub fn set_text(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * std::mem::size_of::<u16>();
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            CloseClipboard();
            return false;
        }
        let target = GlobalLock(handle);
        if target.is_null() {
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), target as *mut u16, wide.len());
        GlobalUnlock(handle);
        let stored = SetClipboardData(CF_UNICODETEXT, handle);
        CloseClipboard();
        !stored.is_null()
    }
}

pub fn set_file(path: &str) -> bool {
    set_files(&[path.to_owned()])
}

pub fn set_files(paths: &[String]) -> bool {
    if paths.is_empty() { return false; }
    let files: Vec<u16> = paths.iter().flat_map(|path| path.encode_utf16().chain(Some(0))).chain(Some(0)).collect();
    let offset = std::mem::size_of::<DropFiles>();
    let bytes = offset + files.len() * std::mem::size_of::<u16>();
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();
        let handle = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if handle.is_null() {
            CloseClipboard();
            return false;
        }
        let target = GlobalLock(handle);
        if target.is_null() {
            CloseClipboard();
            return false;
        }
        let drop = DropFiles {
            offset: offset as u32,
            x: 0,
            y: 0,
            non_client_area: 0,
            wide: 1,
        };
        std::ptr::write(target as *mut DropFiles, drop);
        std::ptr::copy_nonoverlapping(
            files.as_ptr(),
            (target as *mut u8).add(offset) as *mut u16,
            files.len(),
        );
        GlobalUnlock(handle);
        let stored = SetClipboardData(CF_HDROP, handle);
        CloseClipboard();
        !stored.is_null()
    }
}

pub fn read_files() -> Option<Vec<String>> {
    use windows_sys::Win32::System::DataExchange::GetClipboardData;
    use windows_sys::Win32::UI::Shell::DragQueryFileW;
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 { return None; }
        let handle = GetClipboardData(CF_HDROP);
        if handle.is_null() { CloseClipboard(); return Some(Vec::new()); }
        let count = DragQueryFileW(handle as _, u32::MAX, std::ptr::null_mut(), 0);
        let mut paths = Vec::new();
        for i in 0..count {
            let len = DragQueryFileW(handle as _, i, std::ptr::null_mut(), 0);
            let mut buffer = vec![0u16; len as usize + 1];
            if DragQueryFileW(handle as _, i, buffer.as_mut_ptr(), len + 1) == len {
                paths.push(String::from_utf16_lossy(&buffer[..len as usize]));
            }
        }
        CloseClipboard(); Some(paths)
    }
}
