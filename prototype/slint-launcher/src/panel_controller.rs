//! The existing Slint surface bound to a single captured context.
use crate::panel_settings::PanelSettingsController;
use crate::{
    app::AppEvent, panel_model, pins, shell_context::ShellContext, PanelAction, SuperPanel,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::sync::mpsc::Sender;
use std::{cell::RefCell, rc::Rc};
use webtools_ipc::projection::{FeatureRow, Selection};

pub struct PanelController {
    pub ui: SuperPanel,
    pub text: String,
    context: Option<ShellContext>,
    page: usize,
    pins: Vec<pins::Pin>,
    settings: Rc<RefCell<PanelSettingsController>>,
    actions: Vec<(String, String)>,
    board: Vec<(String, String)>,
    choices: Vec<FeatureRow>,
    confirmation: Option<webtools_ipc::quick_result::AIConfirmation>,
}
impl PanelController {
    pub fn new(
        text: String,
        context: Option<ShellContext>,
        rows: &[FeatureRow],
        events: Sender<AppEvent>,
        settings: Rc<RefCell<PanelSettingsController>>,
    ) -> Result<Self, slint::PlatformError> {
        let ui = SuperPanel::new()?;
        let pins = settings.borrow().pins();
        let mut panel = Self {
            ui,
            text: String::new(),
            context: None,
            page: 0,
            pins,
            settings,
            actions: Vec::new(),
            board: Vec::new(),
            choices: Vec::new(),
            confirmation: None,
        };
        let tx = events.clone();
        panel.ui.on_open_search(move || {
            let _ = tx.send(AppEvent::ShowLauncher);
        });
        let tx = events.clone();
        panel.ui.on_action_chosen(move |i| {
            let _ = tx.send(AppEvent::Action(i));
        });
        let tx = events.clone();
        panel.ui.on_board_chosen(move |i| {
            let _ = tx.send(AppEvent::Board(i));
        });
        let tx = events.clone();
        panel.ui.on_board_page(move |i| {
            let _ = tx.send(AppEvent::Page(i));
        });
        let tx = events.clone();
        panel.ui.on_board_context_action(move |i, action| {
            let _ = tx.send(AppEvent::BoardContextAction(i, action));
        });
        let tx = events.clone();
        panel.ui.on_board_reorder(move |from, to| {
            let _ = tx.send(AppEvent::BoardReorder(from, to));
        });
        let tx = events.clone();
        panel.ui.on_board_add(move || {
            let _ = tx.send(AppEvent::Add);
        });
        let tx = events.clone();
        panel.ui.on_chooser_chosen(move |i| {
            let _ = tx.send(AppEvent::Pin(i));
        });
        let tx = events.clone();
        panel.ui.on_close_result(move || {
            let _ = tx.send(AppEvent::CloseResult);
        });
        let tx = events.clone();
        panel.ui.on_consent_approved(move || {
            let _ = tx.send(AppEvent::AIConsentDecision(true));
        });
        let tx = events.clone();
        panel.ui.on_consent_cancelled(move || {
            let _ = tx.send(AppEvent::AIConsentDecision(false));
        });
        panel.set_context(text, context, rows);
        Ok(panel)
    }
    pub fn set_context(
        &mut self,
        text: String,
        context: Option<ShellContext>,
        rows: &[FeatureRow],
    ) {
        self.text = text;
        self.context = context;
        self.confirmation = None;
        self.page = 0;
        self.actions.clear();
        self.board.clear();
        self.choices.clear();
        self.ui.set_mode("context".into());
        self.ui.set_context_kind("none".into());
        self.ui.set_context_title("已选文本".into());
        self.ui.set_context_text("".into());
        self.ui.set_context_meta("".into());
        self.ui.set_context_name("".into());
        self.ui.set_context_path("".into());
        self.ui.set_context_items(ModelRc::new(VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
        self.ui.set_context_overflow("".into());
        self.ui.set_notice("".into());
        self.ui.set_status_text("".into());
        self.ui.set_result_visible(false);
        self.ui.set_result_source("".into());
        self.ui.set_result_body("".into());
        self.ui.set_result_cancel_visible(false);
        self.ui.set_consent_visible(false);
        self.ui.set_consent_source("".into());
        self.ui.set_consent_preview("".into());
        self.ui.set_chooser_visible(false);
        self.ui
            .set_chooser_items(ModelRc::new(VecModel::from(Vec::<PanelAction>::new())));
        self.ui.set_selected_action(-1);
        self.ui.set_selected_board(-1);
        if let Some(context) = self.context.as_ref() {
            use crate::shell_context::ShellKind;
            if matches!(
                context.kind,
                ShellKind::DesktopEmpty | ShellKind::TooManyItems
            ) {
                self.ui.set_mode("board".into());
                self.ui.set_context_kind("none".into());
                if context.kind == ShellKind::TooManyItems {
                    self.ui
                        .set_notice("选择数量超过 64 项，未显示上下文操作。".into());
                }
                self.ui.set_add_shortcut_visible(true);
            } else {
                self.ui.set_context_kind(
                    match context.kind {
                        ShellKind::Folder => "folder",
                        ShellKind::Files => "files",
                        ShellKind::Explorer => "explorer",
                        ShellKind::DesktopEmpty | ShellKind::TooManyItems => "none",
                    }
                    .into(),
                );
                let names: Vec<slint::SharedString> = context
                    .items
                    .iter()
                    .map(|s| s.rsplit(['\\', '/']).next().unwrap_or(s).into())
                    .collect();
                self.ui
                    .set_context_items(ModelRc::new(VecModel::from(names)));
                self.ui
                    .set_context_meta(format!("选中 {} 项", context.items.len()).into());
                self.ui
                    .set_context_name(context.folder_name.as_str().into());
                self.ui
                    .set_context_path(context.folder_path.as_str().into());
            }
        } else if self.text.is_empty() {
            self.ui.set_mode("board".into());
            self.ui.set_add_shortcut_visible(true);
        } else {
            self.ui.set_context_kind("text".into());
            self.ui.set_context_text(self.text.as_str().into());
            self.ui
                .set_context_meta(format!("已捕获 {} 个字符", self.text.chars().count()).into());
        }
        self.ui
            .set_add_shortcut_visible(self.ui.get_mode() == "board");
        self.refresh(rows);
    }
    pub fn refresh(&mut self, rows: &[FeatureRow]) {
        self.pins = self.settings.borrow().pins();
        if self.ui.get_mode() == "board" {
            crate::apply_actions(&self.ui, Vec::new());
            let available = panel_model::pinned_rows(rows, &self.pins);
            let pages = (available.len().div_ceil(9)).max(1);
            self.page = self.page.min(pages - 1);
            let page = available
                .into_iter()
                .skip(self.page * 9)
                .take(9)
                .collect::<Vec<_>>();
            self.board = page
                .iter()
                .map(|r| (r.plugin_id.clone(), r.feature_id.clone()))
                .collect();
            crate::apply_board(
                &self.ui,
                page.iter()
                    .map(|r| PanelAction {
                        title: r.title.as_str().into(),
                        hint: "".into(),
                    })
                    .collect(),
                self.page,
                pages,
            );
            self.ui.set_previous_page_enabled(self.page > 0);
            self.ui.set_next_page_enabled(self.page + 1 < pages);
        } else {
            crate::apply_board(&self.ui, Vec::new(), 0, 1);
            let selection = panel_model::selection_for(&self.context, &self.text);
            let explorer_folder_without_selection = self
                .context
                .as_ref()
                .is_some_and(|context| context.kind == crate::shell_context::ShellKind::Explorer);
            let visible = if explorer_folder_without_selection {
                Vec::new()
            } else {
                panel_model::visible(rows, selection.as_ref())
            };
            let mut ordered = Vec::new();
            let mut seen = Vec::<(String, String)>::new();
            for row in panel_model::pinned_rows(rows, &self.pins) {
                if visible.iter().any(|candidate| {
                    candidate.plugin_id == row.plugin_id && candidate.feature_id == row.feature_id
                }) {
                    seen.push((row.plugin_id.clone(), row.feature_id.clone()));
                    ordered.push(row);
                }
            }
            ordered.extend(visible.into_iter().filter(|row| {
                !seen
                    .iter()
                    .any(|(plugin, feature)| plugin == &row.plugin_id && feature == &row.feature_id)
            }));

            self.actions.clear();
            let mut rendered = Vec::new();
            if self.context.is_none()
                && !self.text.trim().is_empty()
                && self.text.chars().count() <= 3000
            {
                self.actions
                    .push(("webtools.host".into(), "search-selected-text".into()));
                rendered.push(PanelAction {
                    title: "搜索选中文字".into(),
                    hint: "".into(),
                });
                if webtools_ipc::projection::is_http_url(&self.text) {
                    self.actions
                        .push(("webtools.host".into(), "open-selected-link".into()));
                    rendered.push(PanelAction {
                        title: "打开选中链接".into(),
                        hint: "".into(),
                    });
                }
            }
            for row in ordered {
                self.actions
                    .push((row.plugin_id.clone(), row.feature_id.clone()));
                rendered.push(PanelAction {
                    title: row.title.as_str().into(),
                    hint: "".into(),
                });
            }
            crate::apply_actions(&self.ui, rendered);
        }
    }
    pub fn action(&self, index: i32, board: bool) -> Option<(String, String, Option<Selection>)> {
        let (plugin, feature) = if board {
            self.board.get(index as usize)
        } else {
            self.actions.get(index as usize)
        }?
        .clone();
        Some((
            plugin,
            feature,
            if board {
                None
            } else {
                panel_model::selection_for(&self.context, &self.text)
            },
        ))
    }
    pub fn page(&mut self, delta: i32, rows: &[FeatureRow]) {
        self.page = (self.page as i32 + delta).max(0) as usize;
        self.refresh(rows);
    }
    pub fn add(&mut self, rows: &[FeatureRow]) {
        self.choices = panel_model::chooser_rows(rows, &self.pins)
            .into_iter()
            .cloned()
            .collect();
        self.ui.set_chooser_items(ModelRc::new(VecModel::from(
            self.choices
                .iter()
                .map(|r| PanelAction {
                    title: r.title.as_str().into(),
                    hint: r.plugin_name.as_str().into(),
                })
                .collect::<Vec<_>>(),
        )));
        self.ui.set_chooser_visible(true);
    }
    pub fn pin(&mut self, index: i32, rows: &[FeatureRow]) {
        let Some(row) = self.choices.get(index as usize) else {
            return;
        };
        if self.pins.len() >= 100 {
            return;
        }
        let mut pins = self.pins.clone();
        pins.push((
            row.plugin_id.clone(),
            row.feature_id.clone(),
            row.title.clone(),
        ));
        let saved = self.settings.borrow_mut().save_pins(&pins);
        if saved {
            self.pins = pins;
            self.refresh(rows);
        } else {
            self.ui
                .set_notice("固定功能保存失败，原设置保持不变。".into());
        }
    }
    pub fn board_context_action(&mut self, index: i32, action: i32, rows: &[FeatureRow]) {
        let Some((plugin, feature)) = self.board.get(index as usize).cloned() else {
            return;
        };
        let Some(position) = self
            .pins
            .iter()
            .position(|pin| pin.0 == plugin && pin.1 == feature)
        else {
            return;
        };
        let mut pins = self.pins.clone();
        match action {
            0 => {
                pins.remove(position);
            }
            1 | 2 => {
                let next = if action == 1 {
                    position.checked_sub(1)
                } else {
                    Some(position + 1)
                };
                let Some(next) = next.filter(|next| *next < pins.len()) else {
                    return;
                };
                pins.swap(position, next);
            }
            _ => return,
        }
        self.save_pins(pins, rows);
    }
    pub fn reorder_board(&mut self, from: i32, to: i32, rows: &[FeatureRow]) {
        let (Some(source), Some(target)) =
            (self.board.get(from as usize), self.board.get(to as usize))
        else {
            return;
        };
        if from == to {
            return;
        }
        let source = source.clone();
        let target = target.clone();
        let (Some(source_index), Some(target_index)) = (
            self.pins
                .iter()
                .position(|pin| pin.0 == source.0 && pin.1 == source.1),
            self.pins
                .iter()
                .position(|pin| pin.0 == target.0 && pin.1 == target.1),
        ) else {
            return;
        };
        let mut pins = self.pins.clone();
        let item = pins.remove(source_index);
        pins.insert(target_index, item);
        self.save_pins(pins, rows);
    }
    fn save_pins(&mut self, pins: Vec<pins::Pin>, rows: &[FeatureRow]) {
        if self.settings.borrow_mut().save_pins(&pins) {
            self.pins = pins;
            self.refresh(rows);
        } else {
            self.ui
                .set_notice("固定功能保存失败，原设置保持不变。".into());
        }
    }
    pub fn close_result(&self) {
        self.ui.set_result_visible(false);
        self.ui.set_result_cancel_visible(false);
        self.ui.set_result_source("".into());
        self.ui.set_result_body("".into());
    }
    pub fn show_confirmation(&mut self, confirmation: webtools_ipc::quick_result::AIConfirmation) {
        self.ui.set_consent_source(
            format!(
                "{} · {}",
                confirmation.plugin_name, confirmation.provider_name
            )
            .into(),
        );
        self.ui
            .set_consent_preview(confirmation.preview.as_str().into());
        self.ui.set_consent_visible(true);
        self.confirmation = Some(confirmation);
    }
    pub fn take_confirmation(&mut self) -> Option<webtools_ipc::quick_result::AIConfirmation> {
        self.ui.set_consent_visible(false);
        self.ui.set_consent_source("".into());
        self.ui.set_consent_preview("".into());
        self.confirmation.take()
    }
    pub fn clear_confirmation(&mut self) {
        self.confirmation = None;
        self.ui.set_consent_visible(false);
        self.ui.set_consent_source("".into());
        self.ui.set_consent_preview("".into());
    }
}
impl Drop for PanelController {
    fn drop(&mut self) {
        let _ = self.ui.hide();
    }
}
