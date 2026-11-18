use crate::mcp::{invalid, McpToolsProjection};
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

pub fn mcp_profile_path(native_root: &Path) -> PathBuf {
    native_root.join("runtime").join("wt-manager-profile")
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    pub enabled: bool,
    pub port: u16,
    pub token: String,
    pub disabled_plugins: Vec<String>,
}
impl McpSettings {
    pub fn validate(&self) -> io::Result<()> {
        if self.port < 1024
            || !(32..=128).contains(&self.token.len())
            || !self
                .token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(invalid("Invalid MCP settings"));
        }
        Ok(())
    }
    fn fresh() -> io::Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| io::Error::other("Secure random unavailable"))?;
        Ok(Self {
            enabled: false,
            port: 24459,
            token: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            disabled_plugins: vec![],
        })
    }
}
pub struct McpSettingsStore {
    path: PathBuf,
    pub value: McpSettings,
}
impl McpSettingsStore {
    pub fn load_or_migrate(root: &Path) -> io::Result<Self> {
        Self::load_at_profile(root, &mcp_profile_path(root))
    }
    pub fn load_at_profile(root: &Path, profile: &Path) -> io::Result<Self> {
        let path = profile.join("plugin-mcp-settings.json");
        if path.exists() {
            let value: McpSettings = serde_json::from_slice(&fs::read(&path)?)?;
            value.validate()?;
            return Ok(Self { path, value });
        }
        let legacy = root.join("plugin-mcp-settings.json");
        let value = match fs::read(&legacy) {
            Ok(bytes) => match serde_json::from_slice::<McpSettings>(&bytes) {
                Ok(value) => {
                    value.validate()?;
                    value
                }
                Err(_) => McpSettings::fresh()?,
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => McpSettings::fresh()?,
            Err(e) => return Err(e),
        };
        let store = Self { path, value };
        store.save()?;
        Ok(store)
    }
    pub fn save(&self) -> io::Result<()> {
        self.value.validate()?;
        atomic_save(&self.path, &self.value)
    }
    pub fn set_enabled(&mut self, enabled: bool) -> io::Result<()> {
        let mut next = self.value.clone();
        next.enabled = enabled;
        self.commit(next)
    }
    pub fn set_plugin_enabled(&mut self, id: &str, enabled: bool) -> io::Result<()> {
        if id.is_empty() || id.len() > 128 {
            return Err(invalid("Invalid plugin ID"));
        }
        let mut next = self.value.clone();
        next.disabled_plugins.retain(|item| item != id);
        if !enabled {
            next.disabled_plugins.push(id.into());
        }
        self.commit(next)
    }
    fn commit(&mut self, next: McpSettings) -> io::Result<()> {
        next.validate()?;
        atomic_save(&self.path, &next)?;
        self.value = next;
        Ok(())
    }
}
fn atomic_save(path: &Path, value: &impl Serialize) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("tmp-{}", crate::new_request_id()));
    let result = (|| {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp)?;
        serde_json::to_writer_pretty(&file, value)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
pub struct McpProjectionStore {
    path: PathBuf,
    value: Option<McpToolsProjection>,
    generation: u64,
    source_revision: Option<u64>,
}
impl McpProjectionStore {
    pub fn load(path: PathBuf) -> io::Result<Self> {
        let value = match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<McpToolsProjection>(&bytes)
                .ok()
                .filter(|v| v.validate().is_ok()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        Ok(Self {
            path,
            value,
            generation: 0,
            source_revision: None,
        })
    }
    pub fn begin_generation(&mut self, generation: u64) {
        self.generation = generation;
        self.source_revision = None;
    }
    pub fn snapshot(&self) -> Option<McpToolsProjection> {
        self.value.clone()
    }
    pub fn replace_from(&mut self, generation: u64, next: McpToolsProjection) -> io::Result<()> {
        if generation != self.generation || self.source_revision.is_some_and(|r| next.revision <= r)
        {
            return Err(invalid("Stale MCP catalog update"));
        }
        next.validate()?;
        atomic_save(&self.path, &next)?;
        self.source_revision = Some(next.revision);
        self.value = Some(next);
        Ok(())
    }
}
