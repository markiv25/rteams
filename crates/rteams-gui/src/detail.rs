//! Card detail panel. OWNER: scout. `app.rs` calls `show` when a card is selected and acts on
//! the returned `DetailAction`. Read-only: everything is derived from `State`, nothing is mutated.
//! The panel is wrapped in a side panel + vertical ScrollArea by `app.rs`; long text wraps.

use eframe::egui::{self, Color32, RichText};
use rteams_core::{Scope, State, Status, Task};

use crate::board::status_style;
use crate::data::HistoryItem;

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

/// Render everything about `task`: status, owner/reviewer, description, dependencies, comments.
/// The Close button lives here, so the panel owns its own chrome.
pub fn show(ui: &mut egui::Ui, state: &State, task: &str) -> DetailAction {
    let mut action = DetailAction::None;
    if ui.button("Close").clicked() {
        action = DetailAction::Close;
    }
    ui.separator();

    let Some(t) = state.task(task) else {
        ui.label(format!("task {task} not found"));
        return action;
    };

    ui.add(egui::Label::new(RichText::new(&t.subject).heading()).wrap());
    ui.horizontal_wrapped(|ui| {
        status_badge(ui, t.status);
        if t.archived {
            ui.label(RichText::new("archived").small().weak());
        }
        ui.label(
            RichText::new(format!("#{}", short_id(&t.id)))
                .small()
                .weak(),
        );
    });
    ui.add_space(6.0);

    facts(ui, state, t);

    section(ui, "Description");
    if t.description.trim().is_empty() {
        ui.label(RichText::new("No description.").weak());
    } else {
        ui.add(egui::Label::new(&t.description).wrap());
    }

    if let Some(open) = dependencies(ui, state, t) {
        action = DetailAction::OpenTask(open);
    }
    comments(ui, state, t);
    action
}

fn facts(ui: &mut egui::Ui, state: &State, t: &Task) {
    egui::Grid::new(("detail_facts", &t.id))
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            row(ui, "Owner", t.owner.as_deref().unwrap_or("unassigned"));
            if let Some(r) = &t.reviewer {
                row(ui, "Reviewer", r);
            }
            if let Some(from) = t.blocked_from {
                row(ui, "Blocked from", status_style(from).0);
            }
            if let Some(reason) = &t.blocked_reason {
                ui.label(RichText::new("Blocked because").weak());
                ui.add(egui::Label::new(RichText::new(reason).color(Color32::LIGHT_RED)).wrap());
                ui.end_row();
            }
            let waiting = state.unmet_dependencies(t).len();
            if waiting > 0 {
                row(ui, "Waiting on", &format!("{waiting} unfinished"));
            }
        });
}

fn row(ui: &mut egui::Ui, key: &str, value: &str) {
    ui.label(RichText::new(key).weak());
    ui.add(egui::Label::new(value).wrap());
    ui.end_row();
}

/// Returns the id of a related task the user clicked, if any.
fn dependencies(ui: &mut egui::Ui, state: &State, t: &Task) -> Option<String> {
    let blocks = state.dependents(&t.id);
    if t.blocked_by.is_empty() && blocks.is_empty() {
        return None;
    }
    let unmet = state.unmet_dependencies(t);
    let mut clicked = None;

    if !t.blocked_by.is_empty() {
        section(ui, "Depends on");
        for dep in &t.blocked_by {
            let waiting = unmet.contains(&dep.as_str());
            if task_line(ui, state, dep, waiting) {
                clicked = Some(dep.clone());
            }
        }
    }
    if !blocks.is_empty() {
        section(ui, "Blocks");
        for id in blocks {
            if task_line(ui, state, id, false) {
                clicked = Some(id.to_string());
            }
        }
    }
    clicked
}

/// `#abcd1234 subject [status]`; the id is a link. `waiting` marks a dependency that still holds
/// this task up. Returns true when the id was clicked.
fn task_line(ui: &mut egui::Ui, state: &State, id: &str, waiting: bool) -> bool {
    let mut clicked = false;
    ui.horizontal_wrapped(|ui| {
        clicked = ui.link(format!("#{}", short_id(id))).clicked();
        match state.task(id) {
            Some(d) => {
                ui.add(egui::Label::new(&d.subject).wrap());
                status_badge(ui, d.status);
                if waiting {
                    ui.colored_label(Color32::from_rgb(255, 205, 100), "waiting");
                }
            }
            None => {
                ui.label(RichText::new("(unknown task)").weak());
            }
        }
    });
    clicked
}

fn comments(ui: &mut egui::Ui, state: &State, t: &Task) {
    let mine: Vec<_> = state
        .messages
        .iter()
        .filter(|m| matches!(&m.scope, Scope::Task { task } if *task == t.id))
        .collect();

    section(ui, &format!("Comments ({})", mine.len()));
    if mine.is_empty() {
        ui.label(RichText::new("No comments yet.").weak());
        return;
    }
    for m in mine {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&m.from).strong());
                ui.label(RichText::new(format_ts(m.ts)).small().weak());
            });
            ui.add(egui::Label::new(&m.text).wrap());
        });
    }
}

/// Collapsible event history for the selected task (oldest first, as in the log).
/// Call after `show`, inside the same scroll area, with `Data::task_detail(..).history`.
pub fn history(ui: &mut egui::Ui, items: &[HistoryItem]) {
    if items.is_empty() {
        return;
    }
    ui.add_space(10.0);
    egui::CollapsingHeader::new(RichText::new(format!("History ({})", items.len())).strong())
        .default_open(false)
        .show(ui, |ui| {
            for h in items {
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(format_ts(h.ts)).small().weak());
                    ui.label(RichText::new(&h.actor).strong());
                    ui.add(egui::Label::new(&h.summary).wrap());
                });
            }
        });
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).strong());
    ui.separator();
}

fn status_badge(ui: &mut egui::Ui, s: Status) {
    let (text, color) = status_style(s);
    ui.label(
        RichText::new(text)
            .small()
            .color(Color32::BLACK)
            .background_color(color),
    );
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// Event timestamps are unix milliseconds (see `now_ms` in the server). Rendered in UTC.
fn format_ts(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's civil_from_days.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_render_as_utc() {
        assert_eq!(format_ts(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_ts(1_792_113_140_931), "2026-10-16 01:12 UTC");
        assert_eq!(format_ts(951_782_400_000), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn short_id_is_eight_chars() {
        assert_eq!(short_id("1"), "1");
        assert_eq!(short_id("6b183a24-5520-5919"), "6b183a24");
    }
}
