//! UI-thread authority for both trigger settings and pinned feature identities.
use std::path::Path;
use webtools_ipc::super_panel::{FeatureRef, PanelSettings, PanelStateStore, PanelStatus};

pub struct PanelSettingsController {
    store: PanelStateStore,
    registered: bool,
    error: Option<String>,
}
impl PanelSettingsController {
    pub fn load(root: &Path) -> Self {
        let mut store = PanelStateStore::new(root);
        let error = store
            .load()
            .err()
            .map(|e| format!("面板设置读取失败，原文件已保留：{e}"));
        Self {
            store,
            registered: false,
            error,
        }
    }
    pub fn initialize(&mut self, configure: impl FnOnce(&PanelSettings) -> bool) {
        let settings = self.store.snapshot().settings;
        self.registered = configure(&settings) && settings.enabled;
        if settings.enabled && !self.registered {
            self.error = Some("鼠标触发器注册失败。".into());
        }
    }
    pub fn status(&self) -> PanelStatus {
        PanelStatus::new(
            self.store.snapshot().settings,
            self.registered,
            self.error.clone(),
        )
    }
    pub fn enabled(&self) -> bool {
        self.store.snapshot().settings.enabled
    }
    pub fn auto_translation_preview(&self) -> bool {
        self.store.snapshot().settings.auto_translation_preview
    }
    pub fn update(
        &mut self,
        settings: PanelSettings,
        mut configure: impl FnMut(&PanelSettings) -> bool,
    ) -> PanelStatus {
        let previous = self.store.snapshot();
        if !configure(&settings) {
            self.error = Some("鼠标触发器注册失败，原设置保持不变。".into());
            return self.status();
        }
        self.registered = settings.enabled;
        let mut next = previous.clone();
        next.settings = settings;
        if let Err(error) = self.store.save(next) {
            eprintln!("panel: settings save failed: {error}");
            let restored = configure(&previous.settings);
            self.registered = restored && previous.settings.enabled;
            self.error = Some(if restored {
                "设置保存失败，已恢复原触发器。".into()
            } else {
                "设置保存失败；恢复原触发器失败：鼠标触发器不可用。".into()
            });
        } else {
            self.error = None;
        }
        self.status()
    }
    pub fn pins(&self) -> Vec<crate::pins::Pin> {
        self.store
            .snapshot()
            .pinned
            .into_iter()
            .map(|p| (p.plugin_id, p.feature_id, String::new()))
            .collect()
    }
    pub fn save_pins(&mut self, pins: &[crate::pins::Pin]) -> bool {
        let mut next = self.store.snapshot();
        next.pinned = pins
            .iter()
            .map(|(plugin, feature, _)| FeatureRef {
                plugin_id: plugin.clone(),
                feature_id: feature.clone(),
            })
            .collect();
        match self.store.save(next) {
            Ok(()) => {
                self.error = None;
                true
            }
            Err(error) => {
                eprintln!("panel: pin save failed: {error}");
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webtools_ipc::super_panel::{MouseButton, TriggerStatus};
    fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../.tmp/panel-controller")
            .join(webtools_ipc::new_request_id())
    }
    #[test]
    fn settings_and_pins_share_one_snapshot_and_failed_registration_keeps_previous_trigger() {
        let root = root();
        let mut c = PanelSettingsController::load(&root);
        c.initialize(|_| true);
        assert_eq!(c.status().trigger_status, TriggerStatus::Registered);
        let pins = vec![("test.app".into(), "open".into(), "old title".into())];
        assert!(c.save_pins(&pins));
        let next = PanelSettings {
            mouse_button: MouseButton::Xbutton1,
            ..Default::default()
        };
        assert_eq!(c.update(next.clone(), |_| true).settings, next);
        assert_eq!(c.pins()[0].0, "test.app");
        assert!(c.save_pins(&pins));
        assert_eq!(c.status().settings, next);
        let failed = c.update(PanelSettings::default(), |_| false);
        assert_eq!(failed.active_trigger, Some(next.clone()));
        assert!(failed.error.is_some());
        let mut restored = PanelSettingsController::load(&root);
        restored.initialize(|_| true);
        assert_eq!(restored.status().settings, next);
        assert_eq!(restored.pins().len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn save_failure_rolls_back_the_native_trigger_and_reports_actual_restore_failure() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = root();
        let mut c = PanelSettingsController::load(&root);
        c.initialize(|_| true);
        c.update(PanelSettings::default(), |_| true);
        let bytes = std::fs::read(root.join("super-panel-state.json")).unwrap();
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(root.join("super-panel-state.json"))
            .unwrap();
        let mut calls = vec![];
        let disabled = PanelSettings {
            enabled: false,
            ..Default::default()
        };
        let status = c.update(disabled.clone(), |s| {
            calls.push(s.clone());
            true
        });
        assert_eq!(calls, vec![disabled.clone(), PanelSettings::default()]);
        assert_eq!(status.trigger_status, TriggerStatus::Registered);
        assert_eq!(
            status.error.as_deref(),
            Some("设置保存失败，已恢复原触发器。")
        );
        let mut n = 0;
        let status = c.update(disabled, |_| {
            n += 1;
            n == 1
        });
        assert_eq!(status.trigger_status, TriggerStatus::Failed);
        assert!(status.active_trigger.is_none());
        assert_eq!(
            std::fs::read(root.join("super-panel-state.json")).unwrap(),
            bytes
        );
        drop(lock);
        std::fs::remove_dir_all(root).unwrap();
    }
}
