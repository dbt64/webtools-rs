//! Windows notifications invalidate the live catalog even when the launcher is hidden.
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*, System::{Registry::*, Threading::*}};

pub struct CatalogWatch {
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for CatalogWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() { let _ = worker.join(); }
    }
}
enum Watch { Directory(HANDLE), Registry(HANDLE, HKEY) }
impl Watch {
    fn handle(&self) -> HANDLE { match self { Self::Directory(h) | Self::Registry(h, _) => *h } }
    unsafe fn rearm(&self) {
        match self {
            Self::Directory(h) => { FindNextChangeNotification(*h); },
            Self::Registry(h, key) => { RegNotifyChangeKeyValue(*key, 1, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET, *h, 1); },
        }
    }
}
impl Drop for Watch {
    fn drop(&mut self) { unsafe { match self {
        Self::Directory(h) => { FindCloseChangeNotification(*h); },
        Self::Registry(h, key) => { CloseHandle(*h); RegCloseKey(*key); },
    } } }
}
pub fn start(mut changed: impl FnMut() + Send + 'static) -> CatalogWatch {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    let worker = std::thread::spawn(move || unsafe {
        let mut watches = Vec::new();
        for (i, root) in crate::index::shortcut_roots().into_iter().enumerate() {
            let path: Vec<u16> = root.to_string_lossy().encode_utf16().chain(Some(0)).collect();
            let handle = FindFirstChangeNotificationW(path.as_ptr(), if i >= 2 { 1 } else { 0 }, FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME | FILE_NOTIFY_CHANGE_LAST_WRITE);
            if handle != INVALID_HANDLE_VALUE && !handle.is_null() { watches.push(Watch::Directory(handle)); }
        }
        for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            for path in [r"Software\Microsoft\Windows\CurrentVersion\App Paths", r"Software\Microsoft\Windows\CurrentVersion\Uninstall"] {
                let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
                let mut key = std::ptr::null_mut();
                if RegOpenKeyExW(hive, path.as_ptr(), 0, KEY_NOTIFY, &mut key) != 0 { continue; }
                let event = CreateEventW(std::ptr::null(), 0, 0, std::ptr::null());
                if event.is_null() { RegCloseKey(key); continue; }
                let watch = Watch::Registry(event, key);
                watch.rearm(); watches.push(watch);
            }
        }
        let handles: Vec<_> = watches.iter().map(Watch::handle).collect();
        // Register before scanning so changes during the initial inventory remain signaled.
        changed();
        while !flag.load(Ordering::Relaxed) {
            let result = WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, 250);
            if result >= WAIT_OBJECT_0 && result < WAIT_OBJECT_0 + watches.len() as u32 {
                watches[(result - WAIT_OBJECT_0) as usize].rearm();
                changed();
            } else if result == WAIT_FAILED { break; }
        }
    });
    CatalogWatch { stop, worker: Some(worker) }
}
