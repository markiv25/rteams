//! Card detail panel. OWNER: scout. echo's stub: `app.rs` calls `show` when a card is selected.
//! Fill in the body freely, but please keep the signature, or tell echo before changing it.

use eframe::egui;
use rteams_core::State;

/// Render everything about `task` (description, owner/reviewer, dependencies, comments...).
pub fn show(ui: &mut egui::Ui, state: &State, task: &str) {
    match state.task(task) {
        Some(t) => {
            ui.heading(&t.subject);
        }
        None => {
            ui.label(format!("task {task} not found"));
        }
    }
}
