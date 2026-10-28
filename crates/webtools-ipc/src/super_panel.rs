//! SuperPanelSettings v2 and the WPF super-panel-state.json file.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};
pub const CAPABILITY: &str = "super-panel-settings-v2";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Middle,
    Xbutton1,
    Xbutton2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gesture {
    Press,
    Hold,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelSettings {
    pub enabled: bool,
    pub mouse_button: MouseButton,
    pub gesture: Gesture,
    pub hold_milliseconds: u32,
    #[serde(default)]
    pub auto_translation_preview: bool,
}
impl Default for PanelSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            mouse_button: MouseButton::Middle,
            gesture: Gesture::Hold,
            hold_milliseconds: 600,
            auto_translation_preview: false,
        }
    }
}
impl PanelSettings {
    pub fn valid(&self) -> bool {
        (300..=1500).contains(&self.hold_milliseconds)
    }
    pub fn parse(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if ![4, 5].contains(&object.len())
            || !object.keys().all(|k| {
                matches!(
                    k.as_str(),
                    "enabled"
                        | "mouseButton"
                        | "gesture"
                        | "holdMilliseconds"
                        | "autoTranslationPreview"
                )
            })
        {
            return None;
        }
        let settings: Self = serde_json::from_value(value.clone()).ok()?;
        settings.valid().then_some(settings)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureRef {
    pub plugin_id: String,
    pub feature_id: String,
}
impl FeatureRef {
    pub fn valid(&self) -> bool {
        crate::contract::is_valid_plugin_id(&self.plugin_id)
            && crate::contract::is_valid_feature_id(&self.feature_id)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelState {
    pub schema_version: u32,
    #[serde(flatten)]
    pub settings: PanelSettings,
    pub pinned: Vec<FeatureRef>,
}
impl Default for PanelState {
    fn default() -> Self {
        Self {
            schema_version: 2,
            settings: PanelSettings::default(),
            pinned: vec![],
        }
    }
}
impl PanelState {
    pub fn valid(&self) -> bool {
        self.schema_version == 2
            && self.settings.valid()
            && self.pinned.len() <= 100
            && self.pinned.iter().all(FeatureRef::valid)
            && self.pinned.iter().collect::<HashSet<_>>().len() == self.pinned.len()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TriggerStatus {
    Registered,
    Failed,
    Disabled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelStatus {
    pub settings: PanelSettings,
    pub trigger_status: TriggerStatus,
    pub active_trigger: Option<PanelSettings>,
    pub error: Option<String>,
}
impl PanelStatus {
    pub fn new(settings: PanelSettings, available: bool, error: Option<String>) -> Self {
        let trigger_status = if !settings.enabled {
            TriggerStatus::Disabled
        } else if available {
            TriggerStatus::Registered
        } else {
            TriggerStatus::Failed
        };
        let active_trigger =
            (trigger_status == TriggerStatus::Registered).then_some(settings.clone());
        Self {
            settings,
            trigger_status,
            active_trigger,
            error,
        }
    }
}
#[derive(Clone, Debug)]
pub enum PanelOperation {
    Read,
    Update(PanelSettings),
}
pub type PanelHandler =
    Box<dyn FnMut(PanelOperation) -> Result<PanelStatus, crate::settings::SettingsError> + Send>;

pub struct PanelStateStore {
    path: PathBuf,
    state: PanelState,
}
impl PanelStateStore {
    pub fn new(root: &Path) -> Self {
        Self {
            path: root.join("super-panel-state.json"),
            state: PanelState::default(),
        }
    }
    pub fn snapshot(&self) -> PanelState {
        self.state.clone()
    }
    pub fn load(&mut self) -> io::Result<()> {
        check_path(&self.path)?;
        if !self.path.exists() {
            let legacy = self.path.with_file_name("panel-pins.json");
            if legacy.exists() {
                let rows: Vec<(String, String, String)> =
                    serde_json::from_value(read_json(&legacy)?).map_err(|_| invalid())?;
                let mut seen = HashSet::new();
                self.state.pinned = rows
                    .into_iter()
                    .map(|(plugin_id, feature_id, _)| FeatureRef {
                        plugin_id,
                        feature_id,
                    })
                    .filter(|r| r.valid() && seen.insert(r.clone()))
                    .take(100)
                    .collect();
            }
            return Ok(());
        }
        let value = read_json(&self.path)?;
        let state = match value["schemaVersion"].as_u64() {
            Some(2) => serde_json::from_value::<PanelState>(value).map_err(|_| invalid())?,
            Some(1) => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Legacy {
                    #[serde(default = "enabled")]
                    enabled: bool,
                    middle_hold_enabled: bool,
                    hold_milliseconds: u32,
                    pinned: Vec<FeatureRef>,
                }
                fn enabled() -> bool {
                    true
                }
                let old: Legacy = serde_json::from_value(value).map_err(|_| invalid())?;
                PanelState {
                    schema_version: 2,
                    settings: PanelSettings {
                        enabled: old.enabled && old.middle_hold_enabled,
                        hold_milliseconds: old.hold_milliseconds,
                        ..Default::default()
                    },
                    pinned: old.pinned,
                }
            }
            _ => return Err(invalid()),
        };
        if !state.valid() {
            return Err(invalid());
        }
        self.state = state;
        Ok(())
    }
    pub fn save(&mut self, state: PanelState) -> io::Result<()> {
        if !state.valid() {
            return Err(invalid());
        }
        let bytes = serde_json::to_vec_pretty(&state)?;
        if bytes.len() > 32768 {
            return Err(invalid());
        }
        check_path(&self.path)?;
        fs::create_dir_all(self.path.parent().unwrap())?;
        check_path(&self.path)?;
        let temp = self
            .path
            .with_extension(format!("{}.tmp", crate::new_request_id()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            use std::os::windows::ffi::OsStrExt;
            use windows_sys::Win32::Storage::FileSystem::{
                MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
            };
            let from: Vec<_> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<_> = self.path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result?;
        self.state = state;
        Ok(())
    }
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Super Panel settings are invalid; the original file is preserved.",
    )
}
fn read_json(path: &Path) -> io::Result<Value> {
    if fs::metadata(path)?.len() > 32768 {
        return Err(invalid());
    }
    let bytes = fs::read(path)?;
    serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
        .map_err(|_| invalid())
}
fn check_path(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    for p in std::path::absolute(path)?.ancestors() {
        match fs::symlink_metadata(p) {
            Ok(m) if m.file_attributes() & 0x400 != 0 => return Err(invalid()),
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
