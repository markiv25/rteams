//! Floating controls on the org map canvas: the centered Hierarchy / Structure / Relations
//! switch, the search field (top-left), icon buttons (top-right) and the zoom bar (bottom-right).

use eframe::egui::{self, vec2, Align2, Pos2, Rect, RichText};
use rteams_services::i18n::I18n;

use super::focus::Focus;
use super::layout::Mode;
use crate::icons::Icon;
use crate::theme;
use crate::ui::graph::kit::{self, Tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toolbar {
    pub mode: Mode,
    pub relations: bool,
    pub legend: bool,
}

impl Default for Toolbar {
    fn default() -> Self {
        Toolbar {
            mode: Mode::Hierarchy,
            relations: false,
            legend: true,
        }
    }
}

impl Toolbar {
    /// Index of the segmented control: 0 Hierarchy, 1 Structure, 2 Relations.
    pub fn view_idx(&self) -> usize {
        match (self.mode, self.relations) {
            (Mode::Hierarchy, _) => 0,
            (Mode::Nested, false) => 1,
            (Mode::Nested, true) => 2,
        }
    }

    /// Returns the new mode when the view switch changes it.
    pub fn set_view(&mut self, idx: usize) -> Option<Mode> {
        let before = self.mode;
        match idx {
            0 => {
                self.mode = Mode::Hierarchy;
                self.relations = false;
            }
            1 => {
                self.mode = Mode::Nested;
                self.relations = false;
            }
            _ => {
                self.mode = Mode::Nested;
                self.relations = true;
            }
        }
        (self.mode != before).then_some(self.mode)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Fit,
    ZoomIn,
    ZoomOut,
    Reset,
    ClearFocus,
    SetMode(Mode),
}

fn tr_or(tr: &I18n, key: &str, fallback: &str) -> String {
    crate::ui::graph::tr_or(tr, key, fallback)
}

/// Draw the floating controls over `rect`. Returns the triggered actions and the screen
/// position just under the search box (anchor for the results dropdown).
pub fn show(
    ctx: &egui::Context,
    rect: Rect,
    tr: &I18n,
    t: &mut Toolbar,
    focus: &mut Focus,
) -> (Vec<Action>, Pos2) {
    let mut acts = Vec::new();
    let mut anchor = rect.left_top() + vec2(14.0, 52.0);

    // Centered view switch.
    egui::Area::new(egui::Id::NULL.with("orgmap_view_switch"))
        .order(egui::Order::Middle)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(Pos2::new(rect.center().x, rect.top() + 14.0))
        .show(ctx, |ui| {
            kit::pill_bar_frame().show(ui, |ui| {
                let labels = [
                    tr.t("team.organizations.graph.view.hierarchy"),
                    tr.t("team.organizations.graph.view.structure"),
                    tr.t("team.organizations.graph.view.relations"),
                ];
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let mut idx = t.view_idx();
                if theme::segmented(ui, &mut idx, &refs) {
                    if let Some(m) = t.set_view(idx) {
                        acts.push(Action::SetMode(m));
                    }
                }
            });
        });

    // Search, top-left.
    let search = egui::Area::new(egui::Id::NULL.with("orgmap_search"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.left_top() + vec2(14.0, 14.0))
        .show(ctx, |ui| {
            kit::pill_bar_frame()
                .inner_margin(egui::Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (ir, _) =
                            ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
                        crate::icons::draw(ui.painter(), ir, Icon::Search, theme::MUTED);
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut focus.query)
                                .hint_text(tr.t("team.organizations.graph.focus.searchPlaceholder"))
                                .frame(egui::Frame::NONE)
                                .desired_width(190.0),
                        );
                        if r.changed() {
                            focus.target = None;
                        }
                        if focus.is_active()
                            && theme::icon_button(
                                ui,
                                Icon::X,
                                &tr.t("team.organizations.graph.focus.clearFocus"),
                            )
                            .clicked()
                        {
                            acts.push(Action::ClearFocus);
                        }
                    });
                });
        });
    anchor.x = search.response.rect.left();
    anchor.y = search.response.rect.bottom() + 4.0;

    // Icon buttons, top-right.
    egui::Area::new(egui::Id::NULL.with("orgmap_icons"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(Pos2::new(rect.right() - 14.0, rect.top() + 14.0))
        .show(ctx, |ui| {
            kit::pill_bar_frame().show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                ui.horizontal(|ui| {
                    if theme::icon_button_active(ui, Icon::Columns, "Legend", t.legend).clicked() {
                        t.legend = !t.legend;
                    }
                    if theme::icon_button(
                        ui,
                        Icon::Refresh,
                        &tr.t("team.organizations.graph.toolbar.reset"),
                    )
                    .clicked()
                    {
                        acts.push(Action::Reset);
                    }
                });
            });
        });

    // Zoom bar, bottom-right (the minimap sits above it).
    egui::Area::new(egui::Id::NULL.with("orgmap_zoom"))
        .order(egui::Order::Middle)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(Pos2::new(rect.right() - 14.0, rect.bottom() - 12.0))
        .show(ctx, |ui| {
            kit::pill_bar_frame()
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.horizontal(|ui| {
                        if kit::tool_button(
                            ui,
                            Tool::ZoomOut,
                            &tr.t("team.organizations.graph.toolbar.zoomOut"),
                        )
                        .clicked()
                        {
                            acts.push(Action::ZoomOut);
                        }
                        let fit = ui
                            .add(
                                egui::Label::new(
                                    RichText::new("Fit").size(12.0).color(theme::TEXT_2),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .on_hover_text(tr_or(
                                tr,
                                "team.organizations.graph.toolbar.fit",
                                "Fit",
                            ));
                        if fit.clicked() {
                            acts.push(Action::Fit);
                        }
                        if kit::tool_button(
                            ui,
                            Tool::ZoomIn,
                            &tr.t("team.organizations.graph.toolbar.zoomIn"),
                        )
                        .clicked()
                        {
                            acts.push(Action::ZoomIn);
                        }
                    });
                });
        });
    (acts, anchor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orgs_toolbar_view_switch_maps_modes() {
        let mut t = Toolbar::default();
        assert_eq!(t.view_idx(), 0);
        assert_eq!(t.set_view(1), Some(Mode::Nested));
        assert!(!t.relations);
        assert_eq!(t.set_view(2), None);
        assert!(t.relations && t.view_idx() == 2);
        assert_eq!(t.set_view(0), Some(Mode::Hierarchy));
        assert!(!t.relations);
    }
}
