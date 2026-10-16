//! Card detail panel. OWNER: scout. echo's stub: `app.rs` calls `show` when a card is selected.
//! Fill in the body freely, but please keep the signature, or tell echo before changing it.
//! The panel is wrapped in a side panel + vertical ScrollArea by `app.rs`.

use eframe::egui;
use rteams_core::State;

/// What the user asked for inside the panel; `app.rs` acts on it.
#[derive(Debug, Default, PartialEq, Eq)]
pub enum DetailAction {
    #[default]
    None,
    /// Dismiss the panel.
    Close,
    /// The user clicked a related task id (e.g. a dependency): select it instead.
    OpenTask(String),
}

/// Render everything about `task` (description, owner/reviewer, dependencies, comments...).
/// The Close button lives here, so the panel owns its own chrome.
pub fn show(ui: &mut egui::Ui, state: &State, task: &str) -> DetailAction {
    let mut action = DetailAction::None;
    ui.horizontal(|ui| {
        if ui.button("Close").clicked() {
            action = DetailAction::Close;
        }
    });
    ui.separator();
    match state.task(task) {
        Some(t) => {
            ui.heading(&t.subject);
        }
        None => {
            ui.label(format!("task {task} not found"));
        }
    }
    action
}
