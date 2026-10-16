//! The kanban board: five columns of cards. Pure rendering over a `KanbanBoard`.

use eframe::egui::{self, Color32, RichText, Sense, Stroke};
use rteams_core::{Card, KanbanBoard, Status};

/// Draw the board; clicking a card sets `selected` to its task id.
pub fn show(ui: &mut egui::Ui, board: &KanbanBoard, selected: &mut Option<String>) {
    let columns: [(&str, &[Card]); 5] = [
        ("Todo", &board.todo),
        ("In Progress", &board.in_progress),
        ("Review", &board.review),
        ("Approved", &board.approved),
        ("Done", &board.done),
    ];
    ui.columns(columns.len(), |cols| {
        for (ui, (title, cards)) in cols.iter_mut().zip(columns) {
            ui.vertical(|ui| {
                ui.heading(format!("{title} ({})", cards.len()));
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt(title)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for card in cards {
                            card_ui(ui, card, selected);
                        }
                    });
            });
        }
    });
}

fn card_ui(ui: &mut egui::Ui, card: &Card, selected: &mut Option<String>) {
    let is_selected = selected.as_deref() == Some(card.id.as_str());
    let stroke = if is_selected {
        Stroke::new(2.0, ui.visuals().selection.stroke.color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    let resp = egui::Frame::group(ui.style())
        .stroke(stroke)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.monospace(short_id(&card.id));
                badge(ui, card.status);
            });
            ui.label(RichText::new(&card.subject).strong());
            ui.weak(match &card.owner {
                Some(o) => o.as_str(),
                None => "unassigned",
            });
        })
        .response
        .interact(Sense::click());
    if resp.clicked() {
        *selected = Some(card.id.clone());
    }
}

fn badge(ui: &mut egui::Ui, status: Status) {
    let (text, color) = status_style(status);
    ui.label(
        RichText::new(text)
            .small()
            .color(Color32::BLACK)
            .background_color(color),
    );
}

/// First 8 chars, the same display id the on-disk task snapshots use.
fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

pub(crate) fn status_style(s: Status) -> (&'static str, Color32) {
    match s {
        Status::Pending => ("pending", Color32::from_rgb(190, 190, 190)),
        Status::InProgress => ("in progress", Color32::from_rgb(120, 170, 255)),
        Status::Review => ("review", Color32::from_rgb(255, 205, 100)),
        Status::Approved => ("approved", Color32::from_rgb(140, 220, 140)),
        Status::Done => ("done", Color32::from_rgb(100, 190, 120)),
        Status::NeedsFix => ("needs fix", Color32::from_rgb(255, 150, 120)),
        Status::Blocked => ("blocked", Color32::from_rgb(230, 110, 110)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_status_has_a_distinct_label() {
        let mut labels: Vec<_> = Status::ALL.iter().map(|s| status_style(*s).0).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), Status::ALL.len());
    }

    #[test]
    fn short_id_is_eight_chars() {
        assert_eq!(short_id("abcdefghijk"), "abcdefgh");
        assert_eq!(short_id("1"), "1");
    }
}
