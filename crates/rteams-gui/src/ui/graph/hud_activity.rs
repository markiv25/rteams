//! Live activity HUD (top-left): the newest events of the team.

use eframe::egui::{self, vec2, Color32, Rect, RichText};
use rteams_services::graph_model::GraphModel;
use rteams_services::i18n::I18n;

use crate::theme;

pub const SHOWN: usize = 5;

pub fn show(ctx: &egui::Context, viewport: Rect, model: &GraphModel, tr: &I18n) {
    let w = (viewport.width() - 24.0).clamp(0.0, 300.0);
    if w < 120.0 {
        return;
    }
    let hidden_id = egui::Id::NULL.with("graph_activity_hidden");
    let mut hidden: bool = ctx.data(|d| d.get_temp(hidden_id)).unwrap_or(false);
    egui::Area::new(egui::Id::NULL.with("graph_activity_hud"))
        .order(egui::Order::Middle)
        .fixed_pos(viewport.left_top() + vec2(14.0, 14.0))
        .show(ctx, |ui| {
            super::kit::hud_frame().show(ui, |ui| {
                ui.set_width(w - 24.0);
                ui.horizontal(|ui| {
                    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                    super::kit::draw_dot(ui.painter(), dot.center(), 3.0, theme::SUCCESS);
                    ui.label(
                        RichText::new(super::tr_or(
                            tr,
                            "team.agentGraph.activityHud.activity",
                            "Activity",
                        ))
                        .strong()
                        .size(12.5)
                        .color(theme::TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if hidden { "Show" } else { "Hide" };
                        if ui
                            .add(
                                egui::Label::new(
                                    RichText::new(label).size(11.0).color(theme::MUTED),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            hidden = !hidden;
                        }
                    });
                });
                if hidden {
                    return;
                }
                ui.add_space(4.0);
                if model.activity.is_empty() {
                    ui.label(
                        RichText::new(super::tr_or(
                            tr,
                            "team.agentGraph.activityHud.noRecentActivity",
                            "No recent activity",
                        ))
                        .size(11.5)
                        .color(theme::MUTED),
                    );
                    return;
                }
                for a in model.activity.iter().take(SHOWN) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            RichText::new(theme::hhmm(a.ts))
                                .size(10.5)
                                .color(theme::MUTED),
                        );
                        ui.add(
                            egui::Label::new(
                                RichText::new(format!("{} {}", a.actor, a.text))
                                    .size(11.5)
                                    .color(Color32::from_gray(205)),
                            )
                            .truncate(),
                        );
                    });
                }
                let more = model.activity.len().saturating_sub(SHOWN);
                if more > 0 {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(tr.t_args(
                            "team.agentGraph.activityHud.more",
                            &[("count", &more.to_string())],
                        ))
                        .size(10.5)
                        .color(theme::MUTED),
                    );
                }
            });
        });
    ctx.data_mut(|d| d.insert_temp(hidden_id, hidden));
}
