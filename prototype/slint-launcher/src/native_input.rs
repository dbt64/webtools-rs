//! One owned Win32 input thread; only the configured panel gesture consumes input.
use crate::app::AppEvent;
use crate::mouse_gesture::{Disposition, Event as PointerEvent, MouseGesture, Outcome};
use std::sync::{
    atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
    mpsc::Sender,
    Mutex, OnceLock,
};
use webtools_ipc::hotkey::{Binding, DoubleTap};
use webtools_ipc::super_panel::{MouseButton, PanelSettings};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
std::thread_local! { static DOUBLE_TAP:std::cell::RefCell<Option<DoubleTap>>=const{std::cell::RefCell::new(None)}; }
std::thread_local! { static POINTER:std::cell::RefCell<PointerState>=std::cell::RefCell::new(PointerState::default()); }

static EVENTS: OnceLock<Mutex<Option<Sender<AppEvent>>>> = OnceLock::new();
static PANEL: AtomicIsize = AtomicIsize::new(0);
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURE_VERSION: AtomicU64 = AtomicU64::new(0);
static PANEL_VERSION: AtomicU64 = AtomicU64::new(1);

pub fn observe_panel(hwnd: isize) {
    PANEL.store(hwnd, Ordering::SeqCst);
}
pub fn observe_capture(pending: bool) {
    CAPTURE_VERSION.fetch_add(1, Ordering::SeqCst);
    CAPTURING.store(pending, Ordering::SeqCst);
}
pub fn capture_token() -> u64 {
    CAPTURE_VERSION.load(Ordering::SeqCst)
}
pub fn capture_current(token: u64) -> bool {
    CAPTURING.load(Ordering::SeqCst) && CAPTURE_VERSION.load(Ordering::SeqCst) == token
}
pub fn panel_version() -> u64 {
    PANEL_VERSION.load(Ordering::SeqCst)
}
pub fn notify_theme_change() {
    send(AppEvent::SystemThemeChanged);
}
fn send(event: AppEvent) {
    if let Some(slot) = EVENTS.get() {
        if let Ok(events) = slot.lock() {
            if let Some(events) = events.as_ref() {
                let _ = events.send(event);
            }
        }
    }
}
unsafe extern "system" fn keyboard(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0
        && matches!(
            wparam as u32,
            WM_KEYDOWN | WM_SYSKEYDOWN | WM_KEYUP | WM_SYSKEYUP
        )
    {
        let key = &*(lparam as *const KBDLLHOOKSTRUCT);
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        if DOUBLE_TAP.with(|tap| {
            tap.borrow_mut()
                .as_mut()
                .is_some_and(|tap| tap.key(key.vkCode, down, key.time as u64))
        }) {
            send(AppEvent::ToggleLauncher);
        }
    }
    if code >= 0 && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN) {
        let key = &*(lparam as *const KBDLLHOOKSTRUCT);
        let panel = PANEL.load(Ordering::SeqCst);
        if key.vkCode == 0x1b
            && (CAPTURING.load(Ordering::SeqCst)
                || panel != 0 && GetForegroundWindow() as isize != panel)
        {
            send(AppEvent::Escape);
        }
    }
    CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
}
unsafe extern "system" fn mouse(code: i32, wparam: usize, lparam: isize) -> isize {
    if code < 0 {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let pointer = &*(lparam as *const MSLLHOOKSTRUCT);
    if pointer.flags & 3 != 0 {
        return CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam);
    }
    let message = wparam as u32;
    let button = match message {
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some(MouseButton::Middle),
        WM_XBUTTONDOWN | WM_XBUTTONUP => match pointer.mouseData >> 16 {
            1 => Some(MouseButton::Xbutton1),
            2 => Some(MouseButton::Xbutton2),
            _ => None,
        },
        _ => None,
    };
    let down = matches!(
        message,
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
    );
    let mut disposition = Disposition::Pass;
    if down {
        let selected = POINTER.with(|p| p.borrow().gesture.settings.mouse_button);
        if CAPTURING.load(Ordering::SeqCst) {
            observe_capture(false);
            send(AppEvent::Dismiss);
            if button == Some(selected) {
                disposition = Disposition::Cancel;
            }
        }
        let panel = PANEL.load(Ordering::SeqCst);
        let mut rect = std::mem::zeroed();
        if panel != 0
            && GetWindowRect(panel as _, &mut rect) != 0
            && (pointer.pt.x < rect.left
                || pointer.pt.x >= rect.right
                || pointer.pt.y < rect.top
                || pointer.pt.y >= rect.bottom)
        {
            send(AppEvent::Dismiss);
            if disposition != Disposition::Cancel {
                disposition = Disposition::DismissForward;
            }
        }
    }
    let event = if message == WM_MOUSEMOVE {
        Some(PointerEvent::Move(pointer.pt.x, pointer.pt.y))
    } else {
        button.map(|b| {
            if down {
                PointerEvent::Down(b, pointer.pt.x, pointer.pt.y)
            } else {
                PointerEvent::Up(b)
            }
        })
    };
    let out = event
        .map(|event| {
            POINTER.with(|p| {
                p.borrow_mut()
                    .gesture
                    .process(event, pointer_now(), disposition)
            })
        })
        .unwrap_or_default();
    pointer_outcome(out);
    if out.consume {
        1
    } else {
        CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam)
    }
}
struct PointerState {
    gesture: MouseGesture,
    timer: usize,
    armed: Option<u64>,
}
impl Default for PointerState {
    fn default() -> Self {
        Self {
            gesture: MouseGesture::new(PanelSettings {
                enabled: false,
                ..Default::default()
            }),
            timer: 0,
            armed: None,
        }
    }
}
fn pointer_now() -> u64 {
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_millis() as u64
}
fn pointer_outcome(out: Outcome) {
    POINTER.with(|p| {
        let mut p = p.borrow_mut();
        if p.armed != p.gesture.deadline {
            if p.timer != 0 {
                unsafe {
                    KillTimer(std::ptr::null_mut(), p.timer);
                }
            }
            p.timer = 0;
            p.armed = p.gesture.deadline;
            if let Some(deadline) = p.armed {
                p.timer = unsafe {
                    SetTimer(
                        std::ptr::null_mut(),
                        0,
                        deadline
                            .saturating_sub(pointer_now())
                            .clamp(1, u32::MAX as u64) as u32,
                        None,
                    )
                };
            }
        }
    });
    if let Some((button, include_up)) = out.replay {
        replay_mouse(button, include_up);
    }
    if let Some(version) = out.activate {
        send(AppEvent::MousePanel(version));
    }
}
fn replay_mouse(button: MouseButton, include_up: bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    let (down, up, data) = match button {
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, 0),
        MouseButton::Xbutton1 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 1),
        MouseButton::Xbutton2 => (MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, 2),
    };
    let input = |flag| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: data,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut inputs = vec![input(down)];
    if include_up {
        inputs.push(input(up));
    }
    if unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        )
    } != inputs.len() as u32
    {
        eprintln!(
            "panel: mouse-click-restore failed: {}",
            std::io::Error::last_os_error()
        );
    }
}

pub struct NativeInput {
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
    handle: NativeInputHandle,
}
const WM_INPUT_COMMAND: u32 = WM_APP + 50;
enum InputOperation {
    Prepare(String),
    Commit,
    Rollback,
    Panel(PanelSettings),
}
#[derive(Clone)]
pub struct NativeInputHandle {
    thread_id: u32,
    commands: std::sync::mpsc::Sender<(InputOperation, std::sync::mpsc::SyncSender<bool>)>,
}
impl NativeInputHandle {
    fn request(&self, operation: InputOperation) -> bool {
        let (reply, received) = std::sync::mpsc::sync_channel(1);
        if self.commands.send((operation, reply)).is_err() {
            return false;
        }
        if unsafe { PostThreadMessageW(self.thread_id, WM_INPUT_COMMAND, 0, 0) } == 0 {
            return false;
        }
        received.recv().unwrap_or(false)
    }
    pub fn prepare(&self, shortcut: &str) -> bool {
        self.request(InputOperation::Prepare(shortcut.into()))
    }
    pub fn commit(&self) -> bool {
        self.request(InputOperation::Commit)
    }
    pub fn rollback(&self) -> bool {
        self.request(InputOperation::Rollback)
    }
    pub fn configure_panel(&self, settings: &PanelSettings) -> bool {
        self.request(InputOperation::Panel(settings.clone()))
    }
}
trait HotkeyApi {
    fn register(&mut self, id: i32, binding: &Binding) -> bool;
    fn unregister(&mut self, id: i32);
}
struct Win32Hotkeys;
impl HotkeyApi for Win32Hotkeys {
    fn register(&mut self, id: i32, binding: &Binding) -> bool {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::MOD_NOREPEAT;
        match binding {
            Binding::Chord { modifiers, key } => unsafe {
                RegisterHotKey(std::ptr::null_mut(), id, modifiers | MOD_NOREPEAT, *key) != 0
            },
            Binding::DoubleModifier { .. } => true,
        }
    }
    fn unregister(&mut self, id: i32) {
        unsafe {
            UnregisterHotKey(std::ptr::null_mut(), id);
        }
    }
}
struct HotkeySlots<A: HotkeyApi> {
    api: A,
    active: Option<(Binding, i32)>,
    pending: Option<(Binding, i32)>,
    observer: bool,
}
impl<A: HotkeyApi> HotkeySlots<A> {
    fn new(api: A, observer: bool) -> Self {
        Self {
            api,
            active: None,
            pending: None,
            observer,
        }
    }
    fn prepare(&mut self, binding: Binding) -> bool {
        self.rollback();
        if self
            .active
            .as_ref()
            .is_some_and(|(current, _)| *current == binding)
        {
            return true;
        }
        if matches!(binding, Binding::DoubleModifier { .. }) && !self.observer {
            return false;
        }
        let id = if self.active.as_ref().is_some_and(|(_, id)| *id == 1) {
            3
        } else {
            1
        };
        if !self.api.register(id, &binding) {
            return false;
        }
        self.pending = Some((binding, id));
        true
    }
    fn commit(&mut self) {
        if let Some(next) = self.pending.take() {
            if let Some((Binding::Chord { .. }, id)) = self.active.take() {
                self.api.unregister(id);
            }
            self.active = Some(next);
        }
    }
    fn rollback(&mut self) {
        if let Some((Binding::Chord { .. }, id)) = self.pending.take() {
            self.api.unregister(id);
        }
    }
    fn active_id(&self) -> Option<i32> {
        self.active
            .as_ref()
            .and_then(|(binding, id)| matches!(binding, Binding::Chord { .. }).then_some(*id))
    }
    fn observer_key(&self) -> Option<u32> {
        match self.active.as_ref() {
            Some((Binding::DoubleModifier { key }, _)) => Some(*key),
            _ => None,
        }
    }
}
impl NativeInput {
    pub fn handle(&self) -> NativeInputHandle {
        self.handle.clone()
    }
    pub fn start(events: Sender<AppEvent>, shortcut: String) -> Self {
        *EVENTS.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(events.clone());
        let (ready, received) = std::sync::mpsc::sync_channel(1);
        let (commands, command_rx) =
            std::sync::mpsc::channel::<(InputOperation, std::sync::mpsc::SyncSender<bool>)>();
        let thread = std::thread::spawn(move || unsafe {
            use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
            let id = windows_sys::Win32::System::Threading::GetCurrentThreadId();
            let mut message: MSG = std::mem::zeroed();
            PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
            let panel_key = if RegisterHotKey(
                std::ptr::null_mut(),
                2,
                MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
                VK_F11 as u32,
            ) != 0
            {
                "Ctrl+Alt+Shift+F11"
            } else if RegisterHotKey(
                std::ptr::null_mut(),
                2,
                MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT,
                VK_F10 as u32,
            ) != 0
            {
                "Ctrl+Alt+Shift+F10"
            } else {
                "unavailable (use tray)"
            };
            let module =
                windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null());
            let keys = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard), module, 0);
            let pointer = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse), module, 0);
            let mut slots = HotkeySlots::new(Win32Hotkeys, !keys.is_null());
            let launcher = Binding::parse(&shortcut).is_some_and(|binding| slots.prepare(binding));
            slots.commit();
            DOUBLE_TAP.with(|tap| *tap.borrow_mut() = slots.observer_key().map(DoubleTap::new));
            eprintln!("app: input launcher={shortcut} registered={launcher} panel={panel_key} escape={} outside-click={}", !keys.is_null(), !pointer.is_null());
            let _ = ready.send((id, !pointer.is_null()));
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                if message.message == WM_HOTKEY {
                    if Some(message.wParam as i32) == slots.active_id() {
                        let _ = events.send(AppEvent::ToggleLauncher);
                    } else if message.wParam == 2 {
                        let _ = events.send(AppEvent::TogglePanel);
                    }
                }
                if message.message == WM_TIMER
                    && POINTER.with(|p| p.borrow().timer == message.wParam)
                {
                    let out = POINTER.with(|p| {
                        p.borrow_mut().gesture.process(
                            PointerEvent::Tick,
                            pointer_now(),
                            Disposition::Pass,
                        )
                    });
                    pointer_outcome(out);
                }
                if message.message == WM_INPUT_COMMAND {
                    while let Ok((operation, reply)) = command_rx.try_recv() {
                        let ok = match operation {
                            InputOperation::Prepare(shortcut) => Binding::parse(&shortcut)
                                .is_some_and(|binding| slots.prepare(binding)),
                            InputOperation::Commit => {
                                slots.commit();
                                DOUBLE_TAP.with(|tap| {
                                    *tap.borrow_mut() = slots.observer_key().map(DoubleTap::new)
                                });
                                true
                            }
                            InputOperation::Rollback => {
                                slots.rollback();
                                true
                            }
                            InputOperation::Panel(settings) => {
                                if settings.enabled && pointer.is_null() {
                                    false
                                } else {
                                    observe_capture(false);
                                    let out = POINTER.with(|p| {
                                        let mut p = p.borrow_mut();
                                        let out = p.gesture.configure(settings);
                                        PANEL_VERSION.store(p.gesture.version, Ordering::SeqCst);
                                        out
                                    });
                                    pointer_outcome(out);
                                    true
                                }
                            }
                        };
                        let _ = reply.send(ok);
                    }
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            POINTER.with(|p| {
                let mut p = p.borrow_mut();
                if p.timer != 0 {
                    KillTimer(std::ptr::null_mut(), p.timer);
                }
                p.timer = 0;
            });
            UnregisterHotKey(std::ptr::null_mut(), 1);
            UnregisterHotKey(std::ptr::null_mut(), 2);
            UnregisterHotKey(std::ptr::null_mut(), 3);
            DOUBLE_TAP.with(|tap| *tap.borrow_mut() = None);
            if !keys.is_null() {
                UnhookWindowsHookEx(keys);
            }
            if !pointer.is_null() {
                UnhookWindowsHookEx(pointer);
            }
        });
        let (thread_id, _mouse_available) = received.recv().unwrap_or((0, false));
        Self {
            thread_id,
            thread: Some(thread),
            handle: NativeInputHandle {
                thread_id,
                commands,
            },
        }
    }
}
impl Drop for NativeInput {
    fn drop(&mut self) {
        observe_panel(0);
        observe_capture(false);
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(slot) = EVENTS.get() {
            *slot.lock().unwrap() = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Api {
        slots: std::collections::HashMap<i32, Binding>,
        blocked: Option<Binding>,
    }
    impl HotkeyApi for Api {
        fn register(&mut self, id: i32, binding: &Binding) -> bool {
            if self.blocked.as_ref() == Some(binding) || self.slots.values().any(|v| v == binding) {
                return false;
            }
            self.slots.insert(id, binding.clone());
            true
        }
        fn unregister(&mut self, id: i32) {
            self.slots.remove(&id);
        }
    }
    #[test]
    fn preparation_keeps_old_shortcut_reserved_until_disk_commit() {
        let old = Binding::parse("Ctrl+Alt+A").unwrap();
        let new = Binding::parse("Ctrl+Alt+B").unwrap();
        let mut slots = HotkeySlots::new(Api::default(), true);
        assert!(slots.prepare(old.clone()));
        slots.commit();
        assert!(slots.prepare(new.clone()));
        assert_eq!(slots.api.slots.len(), 2);
        slots.rollback();
        assert_eq!(slots.api.slots.len(), 1);
        assert_eq!(slots.api.slots.get(&1), Some(&old));
        assert!(slots.prepare(new.clone()));
        slots.commit();
        assert_eq!(slots.api.slots.len(), 1);
        assert_eq!(slots.api.slots.get(&3), Some(&new));
    }
    #[test]
    fn a_conflict_keeps_the_active_registration() {
        let old = Binding::parse("Ctrl+Alt+A").unwrap();
        let new = Binding::parse("Ctrl+Alt+B").unwrap();
        let mut slots = HotkeySlots::new(Api::default(), true);
        assert!(slots.prepare(old.clone()));
        slots.commit();
        slots.api.blocked = Some(new.clone());
        assert!(!slots.prepare(new));
        assert_eq!(slots.active, Some((old, 1)));
        assert!(slots.pending.is_none());
    }
    #[test]
    fn actual_windows_registration_preserves_the_old_key_when_new_key_is_occupied() {
        let mut slots = HotkeySlots::new(Win32Hotkeys, true);
        let mut keys = Vec::new();
        for key in 0x70..=0x7b {
            let binding = Binding::Chord { modifiers: 14, key };
            if slots.api.register(900 + key as i32, &binding) {
                keys.push((binding, 900 + key as i32));
                if keys.len() == 2 {
                    break;
                }
            }
        }
        assert_eq!(keys.len(), 2, "need two unused test chords");
        slots.api.unregister(keys[0].1);
        assert!(slots.prepare(keys[0].0.clone()));
        slots.commit();
        assert!(!slots.prepare(keys[1].0.clone()));
        assert!(
            !slots.api.register(990, &keys[0].0),
            "old chord must still be registered"
        );
        slots.api.unregister(1);
        slots.api.unregister(keys[1].1);
    }
}
