//! Exclusive quick surfaces and cancellation of late selection captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Surface {
    #[default]
    Hidden,
    Launcher,
    Panel,
}

#[derive(Default)]
pub struct SurfaceState {
    pub active: Surface,
    generation: u64,
    pending: bool,
}
impl SurfaceState {
    pub fn is_pending(&self) -> bool {
        self.pending
    }
    pub fn launcher(&mut self) {
        self.hide();
        self.active = Surface::Launcher;
    }
    pub fn capture(&mut self) -> u64 {
        self.hide();
        self.pending = true;
        self.generation
    }
    pub fn show_pending_panel(&mut self, generation: u64) -> bool {
        if !self.pending || generation != self.generation {
            return false;
        }
        self.active = Surface::Panel;
        true
    }
    pub fn present(&mut self, generation: u64) -> bool {
        if !self.pending || generation != self.generation {
            return false;
        }
        self.pending = false;
        self.active = Surface::Panel;
        true
    }
    pub fn cancel(&mut self, generation: u64) -> bool {
        if !self.pending || self.generation != generation {
            return false;
        }
        self.hide();
        true
    }
    pub fn hide(&mut self) {
        self.cancel_pending();
        self.active = Surface::Hidden;
    }
    pub fn cancel_pending(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_worker_abort_cannot_close_a_new_surface() {
        let mut app = SurfaceState::default();
        let old = app.capture();
        app.launcher();
        assert!(!app.cancel(old));
        assert_eq!(app.active, Surface::Launcher);
        let current = app.capture();
        assert!(app.cancel(current));
        assert_eq!(app.active, Surface::Hidden);
        assert!(!app.present(current));
    }
    #[test]
    fn opening_launcher_revokes_a_pending_panel_capture() {
        let mut app = SurfaceState::default();
        let old = app.capture();
        app.launcher();
        assert!(!app.present(old));
        assert_eq!(app.active, Surface::Launcher);
    }
    #[test]
    fn escape_before_capture_completes_does_not_reopen_panel() {
        let mut app = SurfaceState::default();
        let old = app.capture();
        app.hide();
        assert!(!app.present(old));
        assert_eq!(app.active, Surface::Hidden);
    }
    #[test]
    fn newest_capture_wins_and_cannot_be_presented_twice() {
        let mut app = SurfaceState::default();
        let old = app.capture();
        let current = app.capture();
        assert!(!app.present(old));
        assert!(app.present(current));
        assert!(!app.present(current));
        assert_eq!(app.active, Surface::Panel);
    }
    #[test]
    fn quick_surfaces_are_mutually_exclusive() {
        let mut app = SurfaceState::default();
        app.launcher();
        let capture = app.capture();
        assert_eq!(app.active, Surface::Hidden);
        assert!(app.present(capture));
        app.launcher();
        assert_eq!(app.active, Surface::Launcher);
        app.hide();
        assert_eq!(app.active, Surface::Hidden);
    }
}
