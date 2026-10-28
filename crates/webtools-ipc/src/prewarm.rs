//! When should the manager host be started?
//!
//! Starting it when the launcher appears costs memory for every user, including
//! those who only search local apps. Starting it on the first keystroke pays the
//! same cost only when the user is actually searching, and typing time usually
//! covers the host start.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Query text changed; carries the new length in characters.
    QueryEdited(usize),
    /// Query starts with a search prefix such as '?', so the web path is certain.
    SearchPrefix,
    /// A plugin or website row is now visible in the result list.
    ManagedRowVisible,
    /// The launcher has been open for the dwell budget without any input.
    Dwell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Wait,
    Start,
}

#[derive(Debug, Clone)]
pub struct PrewarmPolicy {
    armed: bool,
    started: bool,
    min_chars: usize,
}

impl Default for PrewarmPolicy {
    fn default() -> Self {
        Self {
            armed: false,
            started: false,
            min_chars: 2,
        }
    }
}

impl PrewarmPolicy {
    pub fn new(min_chars: usize) -> Self {
        Self {
            armed: false,
            started: false,
            min_chars,
        }
    }

    /// Called when the launcher becomes visible. Does not start anything.
    pub fn arm(&mut self) {
        self.armed = true;
        self.started = false;
    }

    /// Called when the launcher hides; the next session starts from scratch.
    pub fn disarm(&mut self) {
        self.armed = false;
        self.started = false;
    }

    pub fn mark_started(&mut self) {
        self.started = true;
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    /// Returns Start at most once per armed session.
    pub fn note(&mut self, trigger: Trigger) -> Decision {
        if !self.armed || self.started {
            return Decision::Wait;
        }
        let fire = match trigger {
            Trigger::QueryEdited(chars) => chars >= self.min_chars,
            Trigger::SearchPrefix | Trigger::ManagedRowVisible | Trigger::Dwell => true,
        };
        if fire {
            self.started = true;
            Decision::Start
        } else {
            Decision::Wait
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_nothing_before_arm() {
        let mut policy = PrewarmPolicy::default();
        assert_eq!(policy.note(Trigger::QueryEdited(10)), Decision::Wait);
    }

    #[test]
    fn arms_without_starting() {
        let mut policy = PrewarmPolicy::default();
        policy.arm();
        assert!(!policy.is_started());
    }

    #[test]
    fn starts_on_second_character() {
        let mut policy = PrewarmPolicy::default();
        policy.arm();
        assert_eq!(policy.note(Trigger::QueryEdited(1)), Decision::Wait);
        assert_eq!(policy.note(Trigger::QueryEdited(2)), Decision::Start);
    }

    #[test]
    fn starts_once_per_session() {
        let mut policy = PrewarmPolicy::default();
        policy.arm();
        assert_eq!(policy.note(Trigger::Dwell), Decision::Start);
        assert_eq!(policy.note(Trigger::QueryEdited(5)), Decision::Wait);
    }

    #[test]
    fn disarm_resets() {
        let mut policy = PrewarmPolicy::default();
        policy.arm();
        policy.note(Trigger::SearchPrefix);
        policy.disarm();
        policy.arm();
        assert!(!policy.is_started());
        assert_eq!(policy.note(Trigger::SearchPrefix), Decision::Start);
    }
}
