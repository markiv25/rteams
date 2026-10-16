//! viz: renders ```viz chart specs (bar / line / pie / table) with the egui painter.
//! The messages view calls `rteams_services::viz::extract` and `render_block` per block.

use eframe::egui::{self, RichText};

use crate::theme;
#[allow(unused_imports)]
pub use rteams_services::viz::{extract, extract_all, Block, VizSpec};

pub mod chart;
pub mod table;

/// Render one validated spec.
pub fn render_block(ui: &mut egui::Ui, spec: &VizSpec) {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            if let Some(t) = spec.title() {
                ui.label(RichText::new(t).strong());
                ui.add_space(4.0);
            }
            match spec {
                VizSpec::Chart(c) => chart::show(ui, c),
                VizSpec::Table(t) => table::show(ui, t),
            }
        });
}

/// Render a parsed fence: the chart, or a one-line note plus the raw source when invalid.
#[cfg_attr(not(test), allow(dead_code))] // called by messages to render invalid viz blocks
pub fn render_parsed(ui: &mut egui::Ui, block: &Block, source: &str) {
    match &block.spec {
        Ok(s) => render_block(ui, s),
        Err(e) => {
            ui.label(RichText::new("Visualization could not be rendered").color(theme::WARNING));
            ui.label(RichText::new(e).small().color(theme::MUTED));
            ui.label(
                RichText::new(source)
                    .monospace()
                    .size(11.0)
                    .color(theme::TEXT_2),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: [&str; 4] = [
        r#"{"type":"bar","title":"B","labels":["a","b","c"],"series":[{"name":"x","values":[1,-2,3]},{"name":"y","values":[2,2,2]}]}"#,
        r#"{"type":"line","labels":["a","b","c"],"series":[{"name":"x","values":[1,5,3]}]}"#,
        r#"{"type":"pie","labels":["a","b"],"series":[{"name":"x","values":[1,3]}]}"#,
        r#"{"type":"table","title":"T","columns":["n","v"],"rows":[["a",1],["b",2.5]]}"#,
    ];

    #[test]
    fn headless_render_all_kinds() {
        let ctx = egui::Context::default();
        for s in SPECS {
            let spec = rteams_services::viz::parse(s).unwrap();
            crate::ui::close_guard::headless(&ctx, |ui| render_block(ui, &spec));
        }
        let b = Block {
            range: 0..1,
            spec: Err("bad".into()),
        };
        crate::ui::close_guard::headless(&ctx, |ui| render_parsed(ui, &b, "{x"));
    }
}
