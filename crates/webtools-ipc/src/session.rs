//! Native-side session: answers the manager's handshake exactly as the WPF host
//! does, so a Rust shell can drive the real Electron manager unmodified.

use crate::{Ack, Connection, Envelope, Result};
use std::time::{Duration, Instant};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

/// The capability strings the manager accepts in its whitelist.
pub const SUPPORTED_FEATURES: [&str; 6] = [
    crate::quick_result::CAPABILITY,
    "feature-projection-v2",
    "electron-host-lifecycle-v1",
    "launcher-feature-surfaces-v1",
    "super-panel-context-v1",
    "super-panel-settings-v2",
];

#[derive(Debug, Clone, Default)]
pub struct HelloInfo {
    pub process_id: u32,
    pub manager_version: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceRequest {
    ShowEmptyLauncher,
    FocusLauncher,
    WindowPresented {
        request_id: String,
        window_handle: u64,
        process_id: u32,
    },
}

pub type SurfaceRequestHandler = Box<dyn FnMut(SurfaceRequest) -> bool + Send>;

pub struct HostSession {
    connection: Connection,
    pub hello: Option<HelloInfo>,
    /// Electron sends this only after initializing its business services and
    /// synchronizing native ownership. A connected pipe alone is not ready.
    pub host_ready: bool,
    pub renderer_ready: bool,
    pub closing: bool,
    ownership: HostOwnership,
    session_id: String,
    results: Vec<crate::quick_result::QuickResult>,
    confirmations: Vec<crate::quick_result::AIConfirmation>,
    pending_confirmations: BTreeMap<String, crate::quick_result::AIConfirmation>,
    pending_quick_invokes: BTreeMap<String, String>,
    completed_quick_invokes: Vec<(String, Ack)>,
    projection: Option<crate::projection::Projection>,
    quick_result_negotiated: bool,
    state: std::sync::Arc<std::sync::Mutex<crate::state::LauncherStateStore>>,
    settings_handler: Option<crate::settings::SettingsHandler>,
    panel_handler: Option<crate::super_panel::PanelHandler>,
    surface_request_handler: Option<SurfaceRequestHandler>,
}

#[derive(Default)]
struct OwnershipState {
    revision: u64,
    sessions: BTreeSet<String>,
    requests: BTreeMap<String, String>,
}

/// The native shell's live ownership snapshot, shared with the pipe session.
/// Session and request changes are reflected in the next Manager handshake or sync.
#[derive(Clone, Default)]
pub struct HostOwnership(Arc<Mutex<OwnershipState>>);

impl HostOwnership {
    pub fn begin_session(&self, id: &str) {
        let mut state = self.0.lock().unwrap();
        if state.sessions.insert(id.to_owned()) {
            state.revision += 1;
        }
    }

    pub fn end_session(&self, id: &str) {
        let mut state = self.0.lock().unwrap();
        if state.sessions.remove(id) {
            state.revision += 1;
        }
    }

    pub fn has_owners(&self) -> bool {
        let state = self.0.lock().unwrap();
        !state.sessions.is_empty() || !state.requests.is_empty()
    }

    fn acquire(&self, id: &str) {
        let mut state = self.0.lock().unwrap();
        if !state.requests.contains_key(id) {
            state.requests.insert(id.to_owned(), "queued".into());
            state.revision += 1;
        }
    }

    fn delivered(&self, id: &str) {
        let mut state = self.0.lock().unwrap();
        if let Some(current) = state.requests.get_mut(id) {
            if current != "delivered" {
                *current = "delivered".into();
                state.revision += 1;
            }
        }
    }

    fn complete(&self, id: &str) {
        let mut state = self.0.lock().unwrap();
        if state.requests.remove(id).is_some() {
            state.revision += 1;
        }
    }

    fn snapshot(&self) -> serde_json::Value {
        let state = self.0.lock().unwrap();
        let sessions: Vec<_> = state.sessions.iter().cloned().collect();
        let requests: Vec<_> = state.requests.iter().map(|(request_id, status)| {
            serde_json::json!({ "requestId": request_id, "state": status })
        }).collect();
        serde_json::json!({ "revision": state.revision, "sessionIds": sessions, "requests": requests })
    }

    fn revision(&self) -> u64 {
        self.0.lock().unwrap().revision
    }
}

/// WPF launcher defaults, retained for callers that use an in-memory session.
pub use crate::state::default_launcher_state;

impl HostSession {
    /// The latest projection the manager published. Every row already carries the
    /// contexts its plugin allows, so the panel filters from this, never from a
    /// local idea of what a feature does.
    pub fn projection(&self) -> Option<&crate::projection::Projection> {
        self.projection.as_ref()
    }

    /// Replaces the projection; used by tests and by the host when it restores a
    /// cached snapshot before the manager republishes.
    pub fn set_projection(&mut self, projection: crate::projection::Projection) {
        self.projection = Some(projection);
    }

    /// Drains the quick results the manager published since the last call.
    pub fn take_results(&mut self) -> Vec<crate::quick_result::QuickResult> {
        std::mem::take(&mut self.results)
    }

    pub fn take_confirmations(&mut self) -> Vec<crate::quick_result::AIConfirmation> {
        std::mem::take(&mut self.confirmations)
    }

    /// Sends the user's one-shot decision for a Manager confirmation.
    pub fn decide_ai_confirmation(
        &mut self,
        confirmation: &crate::quick_result::AIConfirmation,
        approved: bool,
    ) -> Result<Ack> {
        let Some(pending) = self.pending_confirmations.remove(&confirmation.consent_id) else {
            return Ok(Ack {
                reply_to_id: String::new(),
                reply_to_type: "feature-ai-decision-v1".into(),
                ok: false,
                result: None,
                error: Some(crate::AckError {
                    code: "STALE_REQUEST".into(),
                    message: "Confirmation is no longer pending.".into(),
                }),
            });
        };
        if pending != *confirmation {
            return Ok(Ack {
                reply_to_id: String::new(),
                reply_to_type: "feature-ai-decision-v1".into(),
                ok: false,
                result: None,
                error: Some(crate::AckError {
                    code: "STALE_REQUEST".into(),
                    message: "Confirmation binding no longer matches.".into(),
                }),
            });
        }
        let payload = serde_json::json!({
            "requestId": confirmation.request_id,
            "ref": {
                "pluginId": confirmation.plugin_id,
                "featureId": confirmation.feature_id,
            },
            "consentId": confirmation.consent_id,
            "approved": approved,
        });
        self.request(
            &Envelope::new("feature-ai-decision-v1", payload),
            Duration::from_secs(5),
        )
    }

    pub fn take_completed_quick_invokes(&mut self) -> Vec<(String, Ack)> {
        std::mem::take(&mut self.completed_quick_invokes)
    }

    fn accept_quick_invoke_ack(&mut self, ack: Ack) {
        if let Some(request_id) = self.pending_quick_invokes.remove(&ack.reply_to_id) {
            self.ownership.complete(&request_id);
            self.completed_quick_invokes.push((request_id, ack));
        }
    }

    /// Dispatch a network-backed preview without blocking later cancel/window commands.
    pub fn begin_feature_quick_result(
        &mut self,
        request_id: &str,
        plugin_id: &str,
        feature_id: &str,
        text: &str,
    ) -> Result<bool> {
        let ownership = match self.acquire(request_id) {
            Ok(ack) => ack,
            Err(error) => {
                self.ownership.complete(request_id);
                return Err(error);
            }
        };
        if !ownership.ok {
            return Ok(false);
        }
        let captured = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        let payload = serde_json::json!({
            "requestId":request_id,"ref":{"pluginId":plugin_id,"featureId":feature_id},"source":"super-panel",
            "context":{"type":"text","source":"selected-text","text":text,"capturedAtUnixMs":captured},
            "quickResult":{"version":1},
        });
        if !crate::contract::is_valid_feature_invoke(&payload) {
            self.ownership.complete(request_id);
            return Err(crate::ProtocolError::Malformed("feature-invoke payload"));
        }
        let envelope = Envelope::new("feature-invoke", payload);
        self.pending_quick_invokes
            .insert(envelope.request_id.clone(), request_id.to_string());
        if let Err(error) = self.connection.send(&envelope) {
            self.pending_quick_invokes.remove(&envelope.request_id);
            self.ownership.complete(request_id);
            return Err(error);
        }
        Ok(true)
    }

    /// Declares an intent before dispatch. The manager only executes request
    /// ids it has been told about through this snapshot.
    pub fn acquire(&mut self, request_id: &str) -> Result<Ack> {
        self.ownership.acquire(request_id);
        let queued = self.sync_ownership()?;
        if !queued.ok {
            self.ownership.complete(request_id);
            let _ = self.sync_ownership();
            return Ok(queued);
        }
        self.ownership.delivered(request_id);
        let delivered = self.sync_ownership()?;
        if !delivered.ok {
            self.ownership.complete(request_id);
            let _ = self.sync_ownership();
        }
        Ok(delivered)
    }

    /// Opens a launcher page in the manager window.
    pub fn open_page(&mut self, request_id: &str, section: &str) -> Result<Ack> {
        let ownership = self.acquire(request_id)?;
        if !ownership.ok {
            return Ok(ownership);
        }
        let payload = serde_json::json!({ "requestId": request_id, "section": section });
        let response = self.request(
            &Envelope::new("open-page", payload),
            Duration::from_secs(45),
        );
        if response.is_ok() {
            self.finish_request(request_id);
        }
        response
    }

    /// Invokes one plugin feature with text selected in the Super Panel.
    /// `quick_result` is set only for command-style features that return inline
    /// content; window-style features receive the same context without a card.
    pub fn feature_invoke_with_context(
        &mut self,
        request_id: &str,
        plugin_id: &str,
        feature_id: &str,
        text: &str,
        quick_result: bool,
    ) -> Result<Ack> {
        let ownership = self.acquire(request_id)?;
        if !ownership.ok {
            return Ok(ownership);
        }
        let captured = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        // Panel invocations ask for a quick result; without this option the manager
        // runs the feature normally and no card ever comes back.
        let mut payload = serde_json::json!({
            "requestId": request_id,
            "ref": { "pluginId": plugin_id, "featureId": feature_id },
            "source": "super-panel",
            "context": { "type": "text", "source": "selected-text", "text": text, "capturedAtUnixMs": captured },
        });
        if quick_result {
            payload["quickResult"] = serde_json::json!({ "version": 1 });
        }
        let response = self.send_invoke(payload);
        if response.is_ok() {
            self.finish_request(request_id);
        }
        response
    }

    /// Invokes a feature with a file or folder selection attached.
    pub fn feature_invoke_with_selection(
        &mut self,
        request_id: &str,
        plugin_id: &str,
        feature_id: &str,
        selection: &crate::projection::Selection,
    ) -> Result<Ack> {
        let ownership = self.acquire(request_id)?;
        if !ownership.ok {
            return Ok(ownership);
        }
        let captured = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        let items: Vec<serde_json::Value> = selection
            .items
            .iter()
            .map(|(kind, path)| {
                let kind = if *kind == crate::projection::ContextKind::Files {
                    "file"
                } else {
                    "directory"
                };
                serde_json::json!({ "kind": kind, "path": path })
            })
            .collect();
        let kind = if selection.kind == crate::projection::ContextKind::Files {
            "files"
        } else {
            "directories"
        };
        let payload = serde_json::json!({
            "requestId": request_id,
            "ref": { "pluginId": plugin_id, "featureId": feature_id },
            "source": "super-panel",
            "context": {
                "type": kind,
                "source": "explorer-selection",
                "capturedAtUnixMs": captured,
                "items": items,
            },
        });
        let response = self.send_invoke(payload);
        if response.is_ok() {
            self.finish_request(request_id);
        }
        response
    }

    /// Invokes one plugin feature and returns the manager's acceptance.
    pub fn feature_invoke(
        &mut self,
        request_id: &str,
        plugin_id: &str,
        feature_id: &str,
        source: &str,
    ) -> Result<Ack> {
        let ownership = self.acquire(request_id)?;
        if !ownership.ok {
            return Ok(ownership);
        }
        let payload = serde_json::json!({
            "requestId": request_id,
            "ref": { "pluginId": plugin_id, "featureId": feature_id },
            "source": source,
        });
        let response = self.send_invoke(payload);
        if response.is_ok() {
            self.finish_request(request_id);
        }
        response
    }

    /// Invokes a Launcher feature with its query only when the projection
    /// advertised the launcher-input context.
    pub fn feature_invoke_with_launcher_text(
        &mut self,
        request_id: &str,
        plugin_id: &str,
        feature_id: &str,
        text: &str,
    ) -> Result<Ack> {
        let ownership = self.acquire(request_id)?;
        if !ownership.ok {
            return Ok(ownership);
        }
        let captured = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        let payload = serde_json::json!({
            "requestId": request_id,
            "ref": { "pluginId": plugin_id, "featureId": feature_id },
            "source": "launcher",
            "context": {
                "type": "text",
                "source": "launcher-input",
                "text": text,
                "capturedAtUnixMs": captured,
            },
        });
        let response = self.send_invoke(payload);
        if response.is_ok() {
            self.finish_request(request_id);
        }
        response
    }

    /// Opens Translation with the exact Launcher query as its initial text.
    pub fn translation_prefill(
        &mut self,
        request_id: &str,
        text: &str,
        source: &str,
    ) -> Result<Ack> {
        let ownership = self.acquire(request_id)?;
        if !ownership.ok {
            return Ok(ownership);
        }
        let payload = serde_json::json!({
            "requestId": request_id,
            "text": text,
            "source": source,
        });
        let response = self.request(
            &Envelope::new("translation-prefill", payload),
            Duration::from_secs(45),
        );
        if response.is_ok() {
            self.finish_request(request_id);
        }
        response
    }

    /// Cancels a pending Super Panel quick-result feature request in the Manager.
    pub fn cancel_quick_result(
        &mut self,
        request_id: &str,
        plugin_id: &str,
        feature_id: &str,
    ) -> Result<Ack> {
        let payload = serde_json::json!({
            "requestId": request_id,
            "ref": { "pluginId": plugin_id, "featureId": feature_id },
        });
        let ack = self.request(
            &Envelope::new("feature-result-cancel-v1", payload),
            Duration::from_secs(5),
        )?;
        if ack.ok {
            self.pending_quick_invokes
                .retain(|_, pending| pending != request_id);
            self.pending_confirmations
                .retain(|_, confirmation| confirmation.request_id != request_id);
            self.confirmations
                .retain(|confirmation| confirmation.request_id != request_id);
            self.ownership.complete(request_id);
        }
        Ok(ack)
    }

    /// Current Launcher data after Manager updates have been applied.
    pub fn launcher_state_snapshot(&self) -> serde_json::Value {
        self.state.lock().unwrap().snapshot()
    }

    /// Rejects payloads the manager would refuse anyway, before they hit the wire.
    fn send_invoke(&mut self, payload: serde_json::Value) -> Result<Ack> {
        if !crate::contract::is_valid_feature_invoke(&payload) {
            return Err(crate::ProtocolError::Malformed("feature-invoke payload"));
        }
        self.request(
            &Envelope::new("feature-invoke", payload),
            Duration::from_secs(30),
        )
    }

    fn finish_request(&mut self, request_id: &str) {
        self.ownership.complete(request_id);
        if let Err(error) = self.sync_ownership() {
            eprintln!("ownership: request completion sync failed: {error}");
        }
    }

    /// Sends the authoritative native ownership state and records the Manager's
    /// idle-exit decision, matching the WPF host's host-ownership-sync flow.
    pub fn sync_ownership(&mut self) -> Result<Ack> {
        let snapshot = self.ownership.snapshot();
        let ack = self.request(
            &Envelope::new("host-ownership-sync", snapshot),
            Duration::from_secs(10),
        )?;
        if ack
            .result
            .as_ref()
            .and_then(|value| value.get("exiting"))
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            self.closing = true;
        }
        Ok(ack)
    }

    pub fn new(connection: Connection) -> Self {
        Self::with_state_store(connection, crate::state::LauncherStateStore::memory())
    }

    pub fn with_state_store(
        connection: Connection,
        state: crate::state::LauncherStateStore,
    ) -> Self {
        let ownership = HostOwnership::default();
        ownership.begin_session(&crate::new_request_id());
        Self::with_ownership(connection, state, ownership)
    }

    pub fn with_ownership(
        connection: Connection,
        state: crate::state::LauncherStateStore,
        ownership: HostOwnership,
    ) -> Self {
        Self::with_shared_state(
            connection,
            std::sync::Arc::new(std::sync::Mutex::new(state)),
            ownership,
        )
    }

    pub fn with_shared_state(
        connection: Connection,
        state: std::sync::Arc<std::sync::Mutex<crate::state::LauncherStateStore>>,
        ownership: HostOwnership,
    ) -> Self {
        Self {
            connection,
            hello: None,
            host_ready: false,
            renderer_ready: false,
            closing: false,
            ownership,
            session_id: crate::new_request_id(),
            results: Vec::new(),
            confirmations: Vec::new(),
            pending_confirmations: BTreeMap::new(),
            pending_quick_invokes: BTreeMap::new(),
            completed_quick_invokes: Vec::new(),
            projection: None,
            quick_result_negotiated: false,
            state,
            settings_handler: None,
            panel_handler: None,
            surface_request_handler: None,
        }
    }

    pub fn connection(&mut self) -> &mut Connection {
        &mut self.connection
    }

    pub fn set_settings_handler(&mut self, handler: crate::settings::SettingsHandler) {
        self.settings_handler = Some(handler);
    }
    pub fn set_panel_handler(&mut self, handler: crate::super_panel::PanelHandler) {
        self.panel_handler = Some(handler);
    }

    pub fn set_surface_request_handler(&mut self, handler: SurfaceRequestHandler) {
        self.surface_request_handler = Some(handler);
    }

    /// Toggles the current temporary Manager/plugin window without starting Electron.
    pub fn toggle_current_temporary_window(&mut self) -> Result<bool> {
        let negotiated = self.hello.as_ref().is_some_and(|hello| {
            hello
                .capabilities
                .iter()
                .any(|capability| capability == "launcher-feature-surfaces-v1")
        });
        if !negotiated {
            return Ok(false);
        }
        let ack = self.request(
            &Envelope::new("toggle-current-temporary-window", serde_json::json!({})),
            Duration::from_secs(2),
        )?;
        Ok(ack.ok
            && ack
                .result
                .as_ref()
                .and_then(|value| value.get("handled"))
                .and_then(serde_json::Value::as_bool)
                == Some(true))
    }

    pub fn notify_panel_settings(
        &mut self,
        status: &crate::super_panel::PanelStatus,
    ) -> Result<()> {
        if self.hello.as_ref().is_some_and(|h| {
            h.capabilities
                .iter()
                .any(|c| c == crate::super_panel::CAPABILITY)
        }) {
            self.connection.send(&Envelope::new(
                "super-panel-settings-changed",
                serde_json::to_value(status)?,
            ))?;
        }
        Ok(())
    }

    fn update_settings(
        &mut self,
        update: &serde_json::Value,
    ) -> std::result::Result<serde_json::Value, crate::settings::SettingsError> {
        use crate::settings::{SettingsChange, SettingsError, SettingsPhase};
        if let Some(shortcut) = update
            .get("quickSearchShortcut")
            .and_then(serde_json::Value::as_str)
        {
            if !crate::state::is_valid_hotkey(shortcut) {
                return Err(SettingsError {
                    code: "INVALID_HOTKEY",
                    message: "快捷键格式无效。",
                });
            }
        }
        let change = {
            let store = self.state.lock().unwrap();
            SettingsChange {
                previous: store.snapshot(),
                next: store
                    .preview_settings(update)
                    .map_err(|_| SettingsError::unavailable())?,
            }
        };
        let handler = self
            .settings_handler
            .as_mut()
            .ok_or_else(SettingsError::unavailable)?;
        handler(SettingsPhase::Prepare, &change)?;
        let persisted = match self.state.lock().unwrap().apply_settings(update) {
            Ok(state) => state,
            Err(_) => {
                if handler(SettingsPhase::Rollback, &change).is_err() {
                    eprintln!("settings: native rollback failed after persistence error");
                }
                return Err(SettingsError::unavailable());
            }
        };
        if let Err(error) = handler(SettingsPhase::Commit, &change) {
            if self
                .state
                .lock()
                .unwrap()
                .apply_settings(&change.previous)
                .is_err()
            {
                eprintln!("settings: persistence rollback failed after native commit error");
            }
            if handler(SettingsPhase::Rollback, &change).is_err() {
                eprintln!("settings: native rollback failed after commit error");
            }
            return Err(error);
        }
        Ok(persisted)
    }

    /// Reads one message and answers it if it is a request.
    /// Returns false when nothing arrived inside the timeout.
    pub fn pump(&mut self, timeout: Duration) -> Result<bool> {
        let Some(message) = self.connection.poll(timeout)? else {
            return Ok(false);
        };
        self.dispatch(message)?;
        Ok(true)
    }

    fn dispatch(&mut self, message: Envelope) -> Result<()> {
        if std::env::var("WT_PROTOCOL_TRACE").is_ok() {
            eprintln!("<- {}", message.kind);
        }
        match message.kind.as_str() {
            "ack" => {
                if let Ok(ack) = serde_json::from_value::<Ack>(message.payload) {
                    self.accept_quick_invoke_ack(ack);
                }
                Ok(())
            }
            "hello" => {
                let info = HelloInfo {
                    process_id: message.payload["processId"].as_u64().unwrap_or(0) as u32,
                    manager_version: message.payload["managerVersion"]
                        .as_str()
                        .unwrap_or("unknown")
                        .to_string(),
                    capabilities: message.payload["capabilities"]
                        .as_array()
                        .map(|items| {
                            items
                                .iter()
                                .filter_map(|v| v.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                };
                // Only echo the capabilities the manager actually offered, exactly as
                // the WPF host does, and always report hostProcessId: the manager uses
                // it to decide the host is real before it publishes anything.
                let negotiated: Vec<&str> = SUPPORTED_FEATURES
                    .iter()
                    .filter(|capability| {
                        info.capabilities
                            .iter()
                            .any(|offered| offered == *capability)
                    })
                    .filter(|capability| {
                        **capability != crate::super_panel::CAPABILITY
                            || self.panel_handler.is_some()
                    })
                    .copied()
                    .collect();
                self.hello = Some(info);
                let result = serde_json::json!({
                    "state": self.state.lock().unwrap().snapshot(),
                    "hostProcessId": std::process::id(),
                    "supportedFeatures": negotiated,
                });
                self.connection.reply(&message, true, Some(result))
            }
            "manager-renderer-not-ready" => {
                self.connection
                    .reply(&message, true, Some(serde_json::json!({})))
            }
            "manager-renderer-ready" => {
                self.renderer_ready = true;
                self.connection
                    .reply(&message, true, Some(serde_json::json!({})))
            }
            "electron-host-closing" => {
                self.closing = true;
                self.connection
                    .reply(&message, true, Some(serde_json::json!({})))
            }
            "super-panel-settings-get" | "super-panel-settings-update" => {
                use crate::super_panel::{PanelOperation, PanelSettings, CAPABILITY};
                if self.panel_handler.is_none()
                    || !self
                        .hello
                        .as_ref()
                        .is_some_and(|h| h.capabilities.iter().any(|c| c == CAPABILITY))
                {
                    return self.reply_error(
                        &message,
                        "CAPABILITY_REQUIRED",
                        "Super Panel settings are unavailable for this session.",
                    );
                }
                let update = message.kind == "super-panel-settings-update";
                let operation = if update {
                    let Some(settings) = PanelSettings::parse(&message.payload) else {
                        return self.reply_error(
                            &message,
                            "INVALID_PAYLOAD",
                            "Super Panel 设置字段无效。",
                        );
                    };
                    PanelOperation::Update(settings)
                } else {
                    if !message.payload.as_object().is_some_and(|o| o.is_empty()) {
                        return self.reply_error(
                            &message,
                            "INVALID_PAYLOAD",
                            "Super Panel 读取请求无效。",
                        );
                    }
                    PanelOperation::Read
                };
                match self.panel_handler.as_mut().unwrap()(operation) {
                    Ok(status) => {
                        self.connection.reply(
                            &message,
                            true,
                            Some(serde_json::to_value(&status)?),
                        )?;
                        if update {
                            self.notify_panel_settings(&status)?;
                        }
                        Ok(())
                    }
                    Err(error) => self.reply_error(&message, error.code, error.message),
                }
            }
            "launcher-settings-update" => match self.update_settings(&message.payload) {
                Ok(state) => self.connection.reply(&message, true, Some(state)),
                Err(error) => self.reply_error(&message, error.code, error.message),
            },
            "websites-update" => {
                let update = self
                    .state
                    .lock()
                    .unwrap()
                    .replace_websites(&message.payload["websites"]);
                match update {
                    Ok(count) => self.connection.reply(
                        &message,
                        true,
                        Some(serde_json::json!({"count":count})),
                    ),
                    Err(_) => self.reply_error(
                        &message,
                        "INVALID_PAYLOAD",
                        "Manager request payload was invalid or could not be persisted.",
                    ),
                }
            }
            "plugins-update" => {
                let count = message.payload["plugins"]
                    .as_array()
                    .map(|items| items.len())
                    .unwrap_or(0);
                self.connection
                    .reply(&message, true, Some(serde_json::json!({ "count": count })))
            }
            "feature-text-match-negotiate-v1" => {
                let capability = if message.payload["capability"].as_str()
                    == Some("selected-text-match-http-url-v1")
                {
                    "selected-text-match-http-url-v1"
                } else {
                    ""
                };
                self.connection.reply(
                    &message,
                    true,
                    Some(serde_json::json!({ "capability": capability })),
                )
            }
            "features-update" => {
                // The projection is the plugin's own statement about the panel, so an
                // unreadable snapshot is dropped rather than half-applied.
                match crate::projection::parse_projection(&message.payload) {
                    Some(parsed) => {
                        if std::env::var("WT_PROTOCOL_TRACE").is_ok() {
                            eprintln!(
                                "   projection rows={} revision={}",
                                parsed.features.len(),
                                parsed.revision
                            );
                        }
                        self.projection = Some(parsed);
                    }
                    None => {
                        if std::env::var("WT_PROTOCOL_TRACE").is_ok() {
                            eprintln!("   projection REJECTED by parser");
                        }
                    }
                }
                let count = self
                    .projection
                    .as_ref()
                    .map(|projection| projection.features.len())
                    .unwrap_or(0);
                // The manager validates that the acknowledged session matches the
                // hostSessionId it projected, so echo whatever it sent.
                let host_session_id = message.payload["hostSessionId"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| self.session_id.clone());
                let revision = message.payload["revision"]
                    .as_u64()
                    .unwrap_or(self.ownership.revision());
                let result = serde_json::json!({
                    "count": count,
                    "hostSessionId": host_session_id,
                    "revision": revision,
                });
                self.connection.reply(&message, true, Some(result))
            }
            "app-memory-get" => {
                let Some(query) = message.payload["query"]
                    .as_str()
                    .filter(|s| s.encode_utf16().count() <= 128)
                else {
                    return self.reply_error(&message, "INVALID_QUERY", "搜索内容无效。");
                };
                self.connection.reply(
                    &message,
                    true,
                    Some(serde_json::json!({ "appId": self.state.lock().unwrap().remembered(query) })),
                )
            }
            "app-memory-remember" => {
                let Some((query, app_id)) = message.payload["query"]
                    .as_str()
                    .zip(message.payload["appId"].as_str())
                else {
                    return self.reply_error(&message, "INVALID_APP_MEMORY", "应用搜索记忆无效。");
                };
                let remembered = self.state.lock().unwrap().remember(query, app_id);
                match remembered {
                    Ok(()) => self.connection.reply(&message, true, None),
                    Err(error) if error.kind() == std::io::ErrorKind::InvalidData => {
                        self.reply_error(&message, "INVALID_APP_MEMORY", "应用搜索记忆无效。")
                    }
                    Err(_) => self.reply_error(
                        &message,
                        "INVALID_PAYLOAD",
                        "Manager request payload was invalid or could not be persisted.",
                    ),
                }
            }
            "electron-host-ownership-get" => {
                self.connection
                    .reply(&message, true, Some(self.ownership.snapshot()))
            }
            "electron-host-ready" => {
                let snapshot = self.ownership.snapshot();
                let revision = snapshot["revision"].as_u64().unwrap_or_default();
                let synchronized = message.payload["ownershipRevision"].as_u64() == Some(revision);
                let result = serde_json::json!({
                    "synchronized": synchronized,
                    "snapshot": snapshot,
                });
                self.connection.reply(&message, true, Some(result))?;
                if synchronized {
                    self.host_ready = true;
                }
                Ok(())
            }
            "feature-quick-result-negotiate-v1" => {
                // The manager only starts sending cards after the host accepts the
                // capability, and only when the panel surfaces are negotiated.
                let offered = message
                    .payload
                    .get("capability")
                    .and_then(serde_json::Value::as_str)
                    == Some(crate::quick_result::CAPABILITY);
                let surfaces = self
                    .hello
                    .as_ref()
                    .map(|hello| {
                        ["super-panel-context-v1", "launcher-feature-surfaces-v1"]
                            .iter()
                            .all(|capability| {
                                hello
                                    .capabilities
                                    .iter()
                                    .any(|offered| offered == capability)
                            })
                    })
                    .unwrap_or(false);
                self.quick_result_negotiated = offered && surfaces;
                let result = serde_json::json!({
                    "capability": crate::quick_result::CAPABILITY,
                    "acceptedResult": self.quick_result_negotiated,
                });
                self.connection
                    .reply(&message, self.quick_result_negotiated, Some(result))
            }
            "feature-result-v1" => {
                // A malformed card is dropped: the manager already reported success,
                // so the honest answer is "nothing to show", not a guessed body.
                match crate::quick_result::parse_quick_result(&message.payload) {
                    Some(result) => {
                        if std::env::var("WT_PROTOCOL_TRACE").is_ok() {
                            eprintln!("   quick result {} chars", result.text.chars().count());
                        }
                        self.pending_confirmations.retain(|_, confirmation| {
                            confirmation.request_id != result.request_id
                                || confirmation.plugin_id != result.plugin_id
                                || confirmation.feature_id != result.feature_id
                        });
                        self.confirmations.retain(|confirmation| {
                            confirmation.request_id != result.request_id
                                || confirmation.plugin_id != result.plugin_id
                                || confirmation.feature_id != result.feature_id
                        });
                        self.results.push(result);
                    }
                    None => {
                        if std::env::var("WT_PROTOCOL_TRACE").is_ok() {
                            eprintln!("   quick result REJECTED by parser");
                        }
                    }
                }
                self.connection
                    .reply(&message, true, Some(serde_json::json!({})))
            }
            "feature-result-clear-v1" => {
                let Some((request_id, plugin_id, feature_id)) =
                    crate::quick_result::parse_control(&message.payload)
                else {
                    return self.reply_error(&message, "INVALID_REQUEST", "Invalid result clear.");
                };
                self.results.retain(|result| {
                    result.request_id != request_id
                        || result.plugin_id != plugin_id
                        || result.feature_id != feature_id
                });
                self.pending_confirmations.retain(|_, confirmation| {
                    confirmation.request_id != request_id
                        || confirmation.plugin_id != plugin_id
                        || confirmation.feature_id != feature_id
                });
                self.confirmations.retain(|confirmation| {
                    confirmation.request_id != request_id
                        || confirmation.plugin_id != plugin_id
                        || confirmation.feature_id != feature_id
                });
                self.connection
                    .reply(&message, true, Some(serde_json::json!({})))
            }
            "feature-ai-confirm-v1" => {
                if !self.quick_result_negotiated {
                    return self.reply_error(
                        &message,
                        "CAPABILITY_REQUIRED",
                        "Quick Result was not negotiated.",
                    );
                }
                let Some(confirmation) =
                    crate::quick_result::parse_ai_confirmation(&message.payload)
                else {
                    return self.reply_error(
                        &message,
                        "INVALID_REQUEST",
                        "Invalid AI confirmation.",
                    );
                };
                if self
                    .pending_confirmations
                    .contains_key(&confirmation.consent_id)
                {
                    return self.reply_error(
                        &message,
                        "STALE_REQUEST",
                        "Confirmation is already pending.",
                    );
                }
                self.pending_confirmations
                    .insert(confirmation.consent_id.clone(), confirmation.clone());
                self.confirmations.push(confirmation);
                self.connection
                    .reply(&message, true, Some(serde_json::json!({})))
            }
            "launcher-show-empty" => {
                if !has_exact_fields(&message.payload, &[]) {
                    return self.reply_error(
                        &message,
                        "INVALID_REQUEST",
                        "Launcher return takes no arguments.",
                    );
                }
                let shown = self
                    .surface_request_handler
                    .as_mut()
                    .is_some_and(|handler| handler(SurfaceRequest::ShowEmptyLauncher));
                self.connection
                    .reply(&message, true, Some(serde_json::json!({ "shown": shown })))
            }
            "launcher-refocus" => {
                if !self.hello.as_ref().is_some_and(|hello| {
                    hello
                        .capabilities
                        .iter()
                        .any(|capability| capability == "launcher-feature-surfaces-v1")
                }) {
                    return self.reply_error(
                        &message,
                        "CAPABILITY_REQUIRED",
                        "Launcher Features were not negotiated.",
                    );
                }
                if !has_exact_fields(&message.payload, &[]) {
                    return self.reply_error(
                        &message,
                        "INVALID_REQUEST",
                        "Launcher refocus takes no arguments.",
                    );
                }
                let focused = self
                    .surface_request_handler
                    .as_mut()
                    .is_some_and(|handler| handler(SurfaceRequest::FocusLauncher));
                self.connection.reply(
                    &message,
                    true,
                    Some(serde_json::json!({ "focused": focused })),
                )
            }
            "feature-window-presented" => {
                if !self.hello.as_ref().is_some_and(|hello| {
                    hello
                        .capabilities
                        .iter()
                        .any(|capability| capability == "launcher-feature-surfaces-v1")
                }) {
                    return self.reply_error(
                        &message,
                        "CAPABILITY_REQUIRED",
                        "Launcher Features were not negotiated.",
                    );
                }
                if !has_exact_fields(&message.payload, &["requestId", "windowHandle"]) {
                    return self.reply_error(
                        &message,
                        "INVALID_REQUEST",
                        "Invalid window presentation acknowledgement.",
                    );
                }
                let request_id = message.payload["requestId"].as_str().unwrap_or_default();
                let window_handle = message.payload["windowHandle"]
                    .as_str()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or_default();
                let process_id = self
                    .hello
                    .as_ref()
                    .map(|hello| hello.process_id)
                    .unwrap_or_default();
                if request_id.is_empty()
                    || request_id.len() > crate::MAX_REQUEST_ID
                    || window_handle == 0
                    || process_id == 0
                {
                    return self.reply_error(
                        &message,
                        "INVALID_REQUEST",
                        "Invalid window presentation acknowledgement.",
                    );
                }
                let handed_off = self
                    .surface_request_handler
                    .as_mut()
                    .is_some_and(|handler| {
                        handler(SurfaceRequest::WindowPresented {
                            request_id: request_id.to_string(),
                            window_handle,
                            process_id,
                        })
                    });
                self.connection.reply(
                    &message,
                    true,
                    Some(serde_json::json!({ "handedOff": handed_off })),
                )
            }
            other => {
                let payload = serde_json::json!({
                    "replyToId": message.request_id,
                    "replyToType": message.kind,
                    "ok": false,
                    "error": { "code": "UNKNOWN_MESSAGE", "message": format!("unsupported: {other}") },
                });
                self.connection.send(&Envelope::new("ack", payload))
            }
        }
    }

    fn reply_error(&mut self, message: &Envelope, code: &str, description: &str) -> Result<()> {
        self.connection.send(&Envelope::new(
            "ack",
            serde_json::json!({
                "replyToId":message.request_id,"replyToType":message.kind,"ok":false,
                "error":{"code":code,"message":description}
            }),
        ))
    }

    /// Serves messages until the manager's hello has been answered.
    pub fn wait_for_hello(&mut self, timeout: Duration) -> Result<HelloInfo> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !self.pump(Duration::from_millis(50))? && self.hello.is_some() {
                break;
            }
        }
        self.hello
            .clone()
            .ok_or(crate::ProtocolError::Malformed("no hello"))
    }

    /// Serves messages until the renderer reports ready (or the timeout).
    pub fn wait_for_renderer_ready(&mut self, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.pump(Duration::from_millis(50))?;
            if self.renderer_ready {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Sends one request and waits for its ack while still serving inbound
    /// manager requests. A real host must never stop answering during a wait:
    /// the manager asks for renderer state and feature projections in parallel,
    /// and each unanswered request stalls its startup sequence.
    pub fn request(&mut self, envelope: &Envelope, timeout: Duration) -> Result<Ack> {
        self.connection.send(envelope)?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(crate::ProtocolError::Malformed("ack timeout"));
            }
            let Some(message) = self
                .connection
                .poll(remaining.min(Duration::from_millis(25)))?
            else {
                continue;
            };
            if message.kind == "ack" {
                let Ok(ack) = serde_json::from_value::<Ack>(message.payload.clone()) else {
                    continue;
                };
                if ack.reply_to_id == envelope.request_id && ack.reply_to_type == envelope.kind {
                    return Ok(ack);
                }
                self.accept_quick_invoke_ack(ack);
                continue;
            }
            self.dispatch(message)?;
        }
    }
}

impl Drop for HostSession {
    fn drop(&mut self) {
        for request_id in self.pending_quick_invokes.values() {
            self.ownership.complete(request_id);
        }
    }
}

fn has_exact_fields(value: &serde_json::Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}
