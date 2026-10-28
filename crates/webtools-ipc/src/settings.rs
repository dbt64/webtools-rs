//! Native settings apply phases. Prepare reserves resources; commit makes them
//! active only after persistence. Rollback releases the prepared resources.
use serde_json::Value;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPhase {
    Prepare,
    Commit,
    Rollback,
}
#[derive(Clone, Debug)]
pub struct SettingsChange {
    pub previous: Value,
    pub next: Value,
}
#[derive(Clone, Debug)]
pub struct SettingsError {
    pub code: &'static str,
    pub message: &'static str,
}
impl SettingsError {
    pub fn unavailable() -> Self {
        Self {
            code: "INVALID_PAYLOAD",
            message: "Launcher settings could not be applied or persisted.",
        }
    }
    pub fn hotkey_unavailable() -> Self {
        Self {
            code: "HOTKEY_UNAVAILABLE",
            message: "无法注册新快捷键，原快捷键仍保持有效。",
        }
    }
}
pub type SettingsHandler =
    Box<dyn FnMut(SettingsPhase, &SettingsChange) -> Result<(), SettingsError> + Send>;
