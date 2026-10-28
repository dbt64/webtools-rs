//! WPF MouseTriggerObserver decisions, independent of hooks and SendInput.
use webtools_ipc::super_panel::{Gesture, MouseButton, PanelSettings};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    Pass,
    DismissForward,
    Cancel,
}
#[derive(Clone, Copy, Debug)]
pub enum Event {
    Down(MouseButton, i32, i32),
    Up(MouseButton),
    Move(i32, i32),
    Tick,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub consume: bool,
    pub activate: Option<u64>,
    pub replay: Option<(MouseButton, bool)>,
}
struct Press {
    button: MouseButton,
    x: i32,
    y: i32,
    fired: bool,
    forwarded: bool,
}
pub struct MouseGesture {
    pub settings: PanelSettings,
    pub version: u64,
    pressed: Option<Press>,
    pub deadline: Option<u64>,
}
impl MouseGesture {
    pub fn new(settings: PanelSettings) -> Self {
        Self {
            settings,
            version: 1,
            pressed: None,
            deadline: None,
        }
    }
    pub fn configure(&mut self, settings: PanelSettings) -> Outcome {
        self.version = self.version.wrapping_add(1);
        self.settings = settings;
        self.deadline = None;
        let mut out = Outcome::default();
        if let Some(p) = &mut self.pressed {
            if !p.fired && !p.forwarded {
                p.forwarded = true;
                out.replay = Some((p.button, false));
            }
        }
        out
    }
    pub fn process(&mut self, event: Event, at: u64, disposition: Disposition) -> Outcome {
        let mut out = Outcome::default();
        match event {
            Event::Down(button, x, y)
                if self.settings.enabled
                    && button == self.settings.mouse_button
                    && self.pressed.is_none() =>
            {
                let mut p = Press {
                    button,
                    x,
                    y,
                    fired: false,
                    forwarded: false,
                };
                out.consume = true;
                match disposition {
                    Disposition::Cancel => p.fired = true,
                    Disposition::DismissForward => {
                        p.fired = true;
                        p.forwarded = true;
                        out.consume = false;
                    }
                    Disposition::Pass => {
                        if self.settings.gesture == Gesture::Press {
                            p.fired = true;
                            out.activate = Some(self.version);
                        } else {
                            self.deadline = Some(at + self.settings.hold_milliseconds as u64);
                        }
                    }
                }
                self.pressed = Some(p);
            }
            Event::Up(button) if self.pressed.as_ref().is_some_and(|p| p.button == button) => {
                let p = self.pressed.take().unwrap();
                out.consume = !p.forwarded;
                if !p.fired && !p.forwarded {
                    out.replay = Some((p.button, true));
                }
                self.deadline = None;
            }
            Event::Move(x, y) => {
                if let Some(p) = &mut self.pressed {
                    if !p.fired
                        && !p.forwarded
                        && ((x as i64 - p.x as i64).abs() > 12
                            || (y as i64 - p.y as i64).abs() > 12)
                    {
                        p.forwarded = true;
                        self.deadline = None;
                        out.replay = Some((p.button, false));
                    }
                }
            }
            Event::Tick => {
                if self.deadline.is_some_and(|deadline| at >= deadline) {
                    self.deadline = None;
                    if let Some(p) = &mut self.pressed {
                        if !p.fired && !p.forwarded {
                            p.fired = true;
                            out.activate = Some(self.version);
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> PanelSettings {
        PanelSettings::default()
    }
    #[test]
    fn hold_consumes_one_complete_gesture_and_fires_only_at_threshold() {
        let mut m = MouseGesture::new(settings());
        assert!(
            m.process(Event::Down(MouseButton::Middle, 0, 0), 0, Disposition::Pass)
                .consume
        );
        assert_eq!(
            m.process(Event::Tick, 599, Disposition::Pass).activate,
            None
        );
        assert_eq!(
            m.process(Event::Tick, 600, Disposition::Pass).activate,
            Some(1)
        );
        assert_eq!(
            m.process(Event::Tick, 900, Disposition::Pass).activate,
            None
        );
        assert!(
            m.process(Event::Up(MouseButton::Middle), 950, Disposition::Pass)
                .consume
        );
    }
    #[test]
    fn a_short_click_is_restored_and_drag_restores_down_before_forwarding_up() {
        let mut m = MouseGesture::new(settings());
        m.process(
            Event::Down(MouseButton::Middle, 10, 10),
            0,
            Disposition::Pass,
        );
        assert_eq!(
            m.process(Event::Up(MouseButton::Middle), 100, Disposition::Pass)
                .replay,
            Some((MouseButton::Middle, true))
        );
        m.process(
            Event::Down(MouseButton::Middle, 10, 10),
            200,
            Disposition::Pass,
        );
        assert_eq!(
            m.process(Event::Move(23, 10), 230, Disposition::Pass)
                .replay,
            Some((MouseButton::Middle, false))
        );
        assert!(
            !m.process(Event::Up(MouseButton::Middle), 250, Disposition::Pass)
                .consume
        );
        assert_eq!(
            m.process(Event::Tick, 900, Disposition::Pass).activate,
            None
        );
    }
    #[test]
    fn rebind_revokes_old_timer_and_retains_consumed_up_after_a_fired_gesture() {
        let mut m = MouseGesture::new(settings());
        m.process(Event::Down(MouseButton::Middle, 0, 0), 0, Disposition::Pass);
        assert_eq!(
            m.configure(PanelSettings {
                enabled: false,
                ..settings()
            })
            .replay,
            Some((MouseButton::Middle, false))
        );
        assert_eq!(
            m.process(Event::Tick, 1000, Disposition::Pass).activate,
            None
        );
        assert!(
            !m.process(Event::Up(MouseButton::Middle), 1100, Disposition::Pass)
                .consume
        );
        m.configure(PanelSettings {
            gesture: Gesture::Press,
            ..settings()
        });
        m.process(
            Event::Down(MouseButton::Middle, 0, 0),
            1200,
            Disposition::Pass,
        );
        m.configure(PanelSettings {
            enabled: false,
            ..settings()
        });
        assert!(
            m.process(Event::Up(MouseButton::Middle), 1250, Disposition::Pass)
                .consume
        );
    }
    #[test]
    fn outside_trigger_dismisses_and_forwards_and_pending_capture_toggle_consumes_without_reopen() {
        let mut m = MouseGesture::new(PanelSettings {
            gesture: Gesture::Press,
            ..settings()
        });
        let out = m.process(
            Event::Down(MouseButton::Middle, 0, 0),
            0,
            Disposition::DismissForward,
        );
        assert!(!out.consume);
        assert!(out.activate.is_none());
        assert!(
            !m.process(Event::Up(MouseButton::Middle), 10, Disposition::Pass)
                .consume
        );
        let out = m.process(
            Event::Down(MouseButton::Middle, 0, 0),
            20,
            Disposition::Cancel,
        );
        assert!(out.consume);
        assert!(out.activate.is_none());
    }
    #[test]
    fn selected_side_button_and_press_mode_ignore_other_buttons() {
        let mut m = MouseGesture::new(PanelSettings {
            mouse_button: MouseButton::Xbutton2,
            gesture: Gesture::Press,
            ..settings()
        });
        assert_eq!(
            m.process(
                Event::Down(MouseButton::Xbutton1, 0, 0),
                0,
                Disposition::Pass
            ),
            Outcome::default()
        );
        assert_eq!(
            m.process(
                Event::Down(MouseButton::Xbutton2, 0, 0),
                10,
                Disposition::Pass
            )
            .activate,
            Some(1)
        );
    }
}
