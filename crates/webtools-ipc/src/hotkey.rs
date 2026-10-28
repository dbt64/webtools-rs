//! Formats and double-modifier recognition from WPF HotkeyBindingCodec.
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Binding {
    Chord { modifiers: u32, key: u32 },
    DoubleModifier { key: u32 },
}
impl Binding {
    pub fn parse(text: &str) -> Option<Self> {
        if text.encode_utf16().count() > 80 {
            return None;
        }
        let text = text.trim().to_ascii_lowercase();
        if let Some(modifier) = text.strip_prefix("doublemodifier:") {
            return Some(Self::DoubleModifier {
                key: match modifier.trim() {
                    "control" | "ctrl" => 0x11,
                    "alt" => 0x12,
                    _ => return None,
                },
            });
        }
        let function = |s: &str, max: u32| {
            s.strip_prefix('f')
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| (1..=max).contains(n))
                .map(|n| 0x6f + n)
        };
        if let Some(key) = text.strip_prefix("functionkey:") {
            return Some(Self::Chord {
                modifiers: 0,
                key: function(key.trim(), 10)?,
            });
        }
        if let Some(key) = function(&text, 10) {
            return Some(Self::Chord { modifiers: 0, key });
        }
        let mut modifiers = 0;
        let mut key = None;
        for part in text.split('+').map(str::trim).filter(|s| !s.is_empty()) {
            let modifier = match part {
                "control" | "ctrl" => 2,
                "alt" => 1,
                "shift" => 4,
                "super" | "win" | "meta" => 8,
                _ => 0,
            };
            if modifier != 0 {
                if modifiers & modifier != 0 {
                    return None;
                }
                modifiers |= modifier;
            } else {
                if key.is_some() {
                    return None;
                }
                key = Some(
                    if part.len() == 1 && part.as_bytes()[0].is_ascii_alphanumeric() {
                        part.as_bytes()[0].to_ascii_uppercase() as u32
                    } else if let Some(key) = function(part, 12) {
                        key
                    } else {
                        match part {
                            "space" => 0x20,
                            "up" | "arrowup" => 0x26,
                            "down" | "arrowdown" => 0x28,
                            "left" | "arrowleft" => 0x25,
                            "right" | "arrowright" => 0x27,
                            "pageup" => 0x21,
                            "pagedown" => 0x22,
                            "home" => 0x24,
                            "end" => 0x23,
                            "insert" => 0x2d,
                            "delete" => 0x2e,
                            "backspace" => 8,
                            "tab" => 9,
                            "enter" => 0x0d,
                            _ => return None,
                        }
                    },
                );
            }
        }
        if modifiers == 0 {
            return None;
        }
        Some(Self::Chord {
            modifiers,
            key: key?,
        })
    }
}

pub struct DoubleTap {
    target: u32,
    modifiers: HashSet<u32>,
    other: HashSet<u32>,
    candidate: Option<u64>,
    started: u64,
    active: bool,
    invalid: bool,
    second: bool,
}
impl DoubleTap {
    pub fn new(target: u32) -> Self {
        Self {
            target,
            modifiers: HashSet::new(),
            other: HashSet::new(),
            candidate: None,
            started: 0,
            active: false,
            invalid: false,
            second: false,
        }
    }
    pub fn key(&mut self, key: u32, down: bool, time: u64) -> bool {
        if self
            .candidate
            .is_some_and(|at| time < at || time - at > 350)
        {
            self.candidate = None;
        }
        let modifier = match key {
            0x11 | 0xa2 | 0xa3 => Some(0x11),
            0x12 | 0xa4 | 0xa5 => Some(0x12),
            0x10 | 0xa0 | 0xa1 => Some(0x10),
            0x5b | 0x5c => Some(0x5b),
            _ => None,
        };
        let Some(modifier) = modifier else {
            self.candidate = None;
            if self.active {
                self.invalid = true;
            }
            if down {
                self.other.insert(key);
            } else {
                self.other.remove(&key);
            }
            return false;
        };
        if down {
            if !self.modifiers.insert(modifier) {
                if modifier == self.target && self.active {
                    self.invalid = true;
                }
                return false;
            }
            if modifier != self.target {
                self.candidate = None;
                if self.active {
                    self.invalid = true;
                }
                return false;
            }
            self.active = true;
            self.started = time;
            self.invalid =
                !self.other.is_empty() || self.modifiers.iter().any(|m| *m != self.target);
            self.second = !self.invalid
                && self
                    .candidate
                    .is_some_and(|at| time >= at && time - at <= 350);
            if self.invalid || !self.second {
                self.candidate = None;
            }
            return false;
        }
        if !self.modifiers.remove(&modifier) || modifier != self.target || !self.active {
            return false;
        }
        let valid = !self.invalid
            && time >= self.started
            && time - self.started <= 300
            && self.other.is_empty()
            && self.modifiers.is_empty();
        let triggered = valid
            && self.second
            && self
                .candidate
                .is_some_and(|at| time >= at && time - at <= 350);
        self.candidate = if valid && !triggered {
            Some(time)
        } else {
            None
        };
        self.active = false;
        self.invalid = false;
        self.second = false;
        triggered
    }
}
