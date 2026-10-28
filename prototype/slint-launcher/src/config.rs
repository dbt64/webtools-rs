//! Launcher configuration.
//!
//! The packaged build must work without environment variables: everything the
//! user can tune lives in `webtools-launcher.json` next to the executable.
//! Environment variables still win so the bench harness can override a field.

use serde::Deserialize;
use std::path::PathBuf;

/// Production shares the existing Nook profile; isolated runs can override it.
pub fn profile_root() -> PathBuf {
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .map(|p| p.join("AppData").join("Roaming"))
    });
    resolve_profile_root(
        std::env::var_os("WT_PROFILE_ROOT").map(PathBuf::from),
        cfg!(feature = "production"),
        appdata,
        std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())),
    )
}

fn resolve_profile_root(explicit: Option<PathBuf>, production: bool, appdata: Option<PathBuf>, executable_dir: Option<PathBuf>) -> PathBuf {
    if let Some(root) = explicit { return root; }
    if production {
        if let Some(root) = appdata { return root.join("Nook"); }
    }
    executable_dir.map(|p| p.join("data")).unwrap_or_else(|| PathBuf::from("data"))
}

pub fn default_manager_args(production: bool) -> Vec<String> {
    if production {
        vec!["--manager-only".into()]
    } else {
        vec!["--phase4g-manager-test".into(), "--manager-only".into()]
    }
}

/// An explicit profile is an isolated run even when the binary was built for installation.
pub fn production_mode() -> bool {
    cfg!(feature = "production") && std::env::var_os("WT_PROFILE_ROOT").is_none()
}

#[derive(Deserialize, Default, Clone)]
pub struct Config {
    /// Absolute path to the WebTools Manager executable.
    #[serde(rename = "managerExe", default)]
    pub manager_exe: Option<String>,
    /// "on-input" (default) warms after the first keystroke, "on-show" as soon as the window shows.
    #[serde(rename = "prewarmMode", default)]
    pub prewarm_mode: Option<String>,
    /// Characters required before prewarming; defaults to 2.
    #[serde(rename = "minChars", default)]
    pub min_chars: Option<usize>,
    /// Idle seconds before the manager working set is trimmed; defaults to 30.
    #[serde(rename = "idleTrimSeconds", default)]
    pub idle_trim_seconds: Option<u64>,
    /// Extra arguments passed to the manager (defaults keep it isolated).
    #[serde(rename = "managerArgs", default)]
    pub manager_args: Option<Vec<String>>,
}

impl Config {
    pub fn manager_exe(&self) -> Option<String> {
        let configured = std::env::var("WT_PREWARM_BENCH")
            .ok()
            .or_else(|| self.manager_exe.clone())
            .or_else(|| cfg!(feature = "production").then(|| "Manager\\WebTools.exe".into()))?;
        let path = PathBuf::from(&configured);
        if path.is_absolute() {
            return Some(path.to_string_lossy().into_owned());
        }
        let executable_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
        Some(executable_dir.join(path).to_string_lossy().into_owned())
    }

    pub fn prewarm_mode(&self) -> String {
        std::env::var("WT_PREWARM_MODE")
            .ok()
            .or_else(|| self.prewarm_mode.clone())
            .unwrap_or_else(|| "on-input".into())
    }
}

/// Loads the first config found: `WT_CONFIG`, then the exe directory, then cwd.
pub fn load() -> Config {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("WT_CONFIG") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("webtools-launcher.json"));
        }
    }
    candidates.push(PathBuf::from("webtools-launcher.json"));

    for path in candidates {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("config: {} unreadable ({error})", path.display());
                continue;
            }
        };
        // Windows editors love to prepend a BOM; serde_json rejects it.
        let text = text.trim_start_matches('\u{feff}');
        match serde_json::from_str::<Config>(text) {
            Ok(config) => {
                eprintln!("config: loaded {}", path.display());
                return config;
            }
            Err(error) => eprintln!("config: {} is invalid ({error}), ignoring", path.display()),
        }
    }
    eprintln!("config: none found, using defaults");
    Config::default()
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn production_profile_uses_existing_roaming_nook_directory() {
        assert_eq!(
            resolve_profile_root(None, true, Some(PathBuf::from(r"C:\Users\Example\AppData\Roaming")), Some(PathBuf::from(r"C:\Program Files\WebTools"))),
            PathBuf::from(r"C:\Users\Example\AppData\Roaming\Nook")
        );
    }

    #[test]
    fn isolated_profile_override_wins_even_for_a_production_build() {
        let root = PathBuf::from(r"D:\isolated\profile");
        assert_eq!(resolve_profile_root(Some(root.clone()), true, None, None), root);
    }

    #[test]
    fn manager_arguments_distinguish_installed_and_isolated_modes() {
        assert_eq!(default_manager_args(true), vec!["--manager-only"]);
        assert_eq!(default_manager_args(false), vec!["--phase4g-manager-test", "--manager-only"]);
    }
}
