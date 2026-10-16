//! viz::table: table spec as a striped grid.

use eframe::egui::{self, Align, Layout, RichText};
use rteams_services::viz::TableSpec;

use crate::theme;

fn numeric(s: &str) -> bool {
    !s.is_empty() && s.parse::<f64>().is_ok()
}

pub fn show(ui: &mut egui::Ui, t: &TableSpec) {
    egui::ScrollArea::horizontal()
        .auto_shrink([false, true])
        .show(ui, |ui| {
            egui::Grid::new(ui.next_auto_id())
                .striped(true)
                .spacing([14.0, 4.0])
                .show(ui, |ui| {
                    for c in &t.columns {
                        ui.label(RichText::new(c).strong().color(theme::TEXT_2));
                    }
                    ui.end_row();
                    for row in &t.rows {
                        for cell in row {
                            if numeric(cell) {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.label(RichText::new(cell).monospace());
                                });
                            } else {
                                ui.label(cell);
                            }
                        }
                        ui.end_row();
                    }
                });
        });
}
