//! The application and input control that owned focus before a quick surface.
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct PasteTarget {
    pub window: isize,
    process: u32,
    thread: u32,
    focus: isize,
}

fn input_ready(window: isize, expected_focus: isize, foreground: isize, focus: isize) -> bool {
    foreground == window && focus != 0 && (expected_focus == 0 || focus == expected_focus)
}

impl PasteTarget {
    pub fn capture(window: isize) -> Option<Self> {
        let mut process = 0;
        let thread = unsafe { GetWindowThreadProcessId(window as _, &mut process) };
        if window == 0 || thread == 0 || process == std::process::id() { return None; }
        let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
        let focus = if unsafe { GetGUIThreadInfo(thread, &mut info) } != 0 { info.hwndFocus as isize } else { 0 };
        Some(Self { window, process, thread, focus })
    }

    pub fn paste_text(self, text: &str) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
        let mut process = 0;
        if self.window == 0 || unsafe { IsWindow(self.window as _) } == 0
            || unsafe { GetWindowThreadProcessId(self.window as _, &mut process) } != self.thread
            || process != self.process { return false; }
        unsafe { SetForegroundWindow(self.window as _); }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let ready = || {
            let mut info: GUITHREADINFO = unsafe { std::mem::zeroed() };
            info.cbSize = std::mem::size_of::<GUITHREADINFO>() as u32;
            (unsafe { GetGUIThreadInfo(self.thread, &mut info) } != 0)
                && input_ready(self.window, self.focus, crate::visual::current_foreground(), info.hwndFocus as isize)
        };
        while !ready() {
            if std::time::Instant::now() >= deadline { return false; }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if !crate::clipboard::set_text(text) || !ready() { return false; }
        let key = |vk, flags| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } } };
        let inputs = [key(VK_CONTROL, 0), key(b'V' as u16, 0), key(b'V' as u16, KEYEVENTF_KEYUP), key(VK_CONTROL, KEYEVENTF_KEYUP)];
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) == inputs.len() as u32 }
    }
}

#[cfg(test)]
mod tests {
    use super::input_ready;
    #[test]
    fn foreground_without_the_original_input_focus_is_not_ready_for_paste() {
        assert!(!input_ready(10, 11, 10, 0));
        assert!(!input_ready(10, 11, 10, 12));
        assert!(!input_ready(10, 11, 20, 11));
        assert!(input_ready(10, 11, 10, 11));
        assert!(!input_ready(10, 0, 10, 0));
        assert!(input_ready(10, 0, 10, 11));
    }
}
