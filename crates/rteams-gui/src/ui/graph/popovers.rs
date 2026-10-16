//! Hover cards (agent / task) and click popovers (blocking dependency, hidden tasks).

use eframe::egui::{self, vec2, Color32, Pos2, Rect, RichText};
use rteams_services::graph_model::{GraphModel, GraphNode, NodeKind};
use rteams_services::i18n::I18n;

use super::draw_agents::{state_color, state_label};
use super::draw_tasks::status_color;
use crate::launcher::{AgentStatus, AgentView};
use crate::theme;
use crate::view::{Intent, Intents};

const CARD_W: f32 = 260.0;

/// Last non-empty output line of an agent, shortened.
pub fn last_line(a: &AgentView) -> Option<String> {
    a.last_lines
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| {
            let l = l.trim();
            if l.chars().count() > 140 {
                let cut: String = l.chars().take(139).collect();
                format!("{cut}…")
            } else {
                l.to_string()
            }
        })
}

fn card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(theme::RAISED)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.2)))
        .corner_radius(8)
        .inner_margin(10)
}

/// Where a card goes: right of the node when it fits, else left, kept inside `viewport`.
pub fn card_pos(node_screen: Rect, viewport: Rect, size: egui::Vec2) -> Pos2 {
    let mut x = node_screen.right() + 10.0;
    if x + size.x > viewport.right() {
        x = node_screen.left() - 10.0 - size.x;
    }
    let x = x.clamp(
        viewport.left(),
        (viewport.right() - size.x).max(viewport.left()),
    );
    let y = node_screen.top().clamp(
        viewport.top(),
        (viewport.bottom() - size.y).max(viewport.top()),
    );
    Pos2::new(x, y)
}

pub fn member_card(
    ui: &mut egui::Ui,
    node: &GraphNode,
    color: Color32,
    agent: Option<&AgentView>,
    tr: &I18n,
) {
    ui.set_width(CARD_W - 20.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&node.label).strong().size(13.0).color(color));
        if node.kind == NodeKind::Lead {
            theme::pill(
                ui,
                super::tr_or(tr, "team.agentGraph.popover.member.lead", "Lead"),
                theme::SUCCESS,
            );
        }
        if let Some(rt) = &node.runtime {
            theme::pill(ui, rt.clone(), theme::TEXT_2);
        }
    });
    let (key, fb) = match (agent.map(|a| &a.status), node.online) {
        (Some(AgentStatus::Failed(_)), _) => {
            ("team.agentGraph.popover.member.spawn.failed", "failed")
        }
        (_, true) if node.current_task.is_some() => {
            ("team.agentGraph.popover.member.state.active", "active")
        }
        (_, true) => ("team.agentGraph.popover.member.state.idle", "idle"),
        _ => ("team.agentGraph.popover.member.state.offline", "offline"),
    };
    ui.horizontal(|ui| {
        ui.colored_label(state_color(node.state), "•");
        ui.label(
            RichText::new(super::tr_or(tr, key, fb))
                .size(11.5)
                .color(theme::TEXT_2),
        );
        ui.label(
            RichText::new(state_label(node.state))
                .size(10.5)
                .color(theme::MUTED),
        );
    });
    if let Some(t) = &node.current_task {
        ui.label(
            RichText::new(format!(
                "{} {t}",
                super::tr_or(tr, "team.agentGraph.popover.member.workingOn", "working on")
            ))
            .size(11.5)
            .color(theme::TEXT),
        );
    }
    if let Some(l) = agent.and_then(last_line).or_else(|| node.last_line.clone()) {
        ui.add_space(2.0);
        ui.add(
            egui::Label::new(RichText::new(l).monospace().size(10.5).color(theme::TEXT_2)).wrap(),
        );
    }
    if let Some(AgentStatus::Failed(e)) = agent.map(|a| &a.status) {
        ui.label(RichText::new(e).size(10.5).color(theme::ERROR_FG));
    }
}

pub fn task_card(ui: &mut egui::Ui, node: &GraphNode) {
    ui.set_width(CARD_W - 20.0);
    let id = node.id.trim_start_matches("task:");
    ui.add(
        egui::Label::new(
            RichText::new(format!("#{id} {}", node.label))
                .strong()
                .size(12.5)
                .color(theme::TEXT),
        )
        .wrap(),
    );
    if let Some(s) = node.status {
        ui.horizontal(|ui| {
            theme::pill(
                ui,
                rteams_services::graph_model::status_label(s),
                status_color(s),
            );
            if let Some(o) = &node.owner {
                ui.label(RichText::new(o).size(11.0).color(theme::TEXT_2));
            }
        });
    }
}

pub fn process_card(ui: &mut egui::Ui, node: &GraphNode, tr: &I18n) {
    ui.set_width(CARD_W - 20.0);
    ui.label(
        RichText::new(&node.label)
            .strong()
            .size(12.5)
            .color(theme::TEXT),
    );
    if let Some(o) = &node.owner {
        ui.label(
            RichText::new(format!(
                "{} {o}",
                super::tr_or(
                    tr,
                    "team.agentGraph.popover.process.startedBy",
                    "Started by:"
                )
            ))
            .size(11.0)
            .color(theme::TEXT_2),
        );
    }
    if let Some(u) = &node.url {
        ui.label(RichText::new(u).size(10.5).color(theme::ACCENT));
    }
}

/// Floating, non-interactive card next to a hovered node.
pub fn hover_card(
    ctx: &egui::Context,
    node_screen: Rect,
    viewport: Rect,
    add: impl FnOnce(&mut egui::Ui),
) {
    let pos = card_pos(node_screen, viewport, vec2(CARD_W, 120.0));
    egui::Area::new(egui::Id::unique("graph_hover_card"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            card_frame().show(ui, add);
        });
}

fn open_row(ui: &mut egui::Ui, label: &str, node: Option<&GraphNode>, out: &mut Intents) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(11.0).color(theme::MUTED));
        if let Some(n) = node {
            let id = n.id.trim_start_matches("task:").to_string();
            if ui
                .link(RichText::new(format!("#{id} {}", n.label)).size(11.5))
                .clicked()
            {
                out.push(Intent::OpenTask(id));
            }
        }
    });
}

/// Blocking dependency popover; returns true when it should close.
pub fn blocking(
    ctx: &egui::Context,
    at: Pos2,
    from: Option<&GraphNode>,
    to: Option<&GraphNode>,
    out: &mut Intents,
    tr: &I18n,
) -> bool {
    let mut close = false;
    egui::Area::new(egui::Id::unique("graph_blocking_popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            card_frame().show(ui, |ui| {
                ui.set_width(CARD_W - 20.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(super::tr_or(
                            tr,
                            "team.agentGraph.blockingEdge.title",
                            "Blocking Dependency",
                        ))
                        .strong()
                        .size(12.5)
                        .color(theme::TEXT),
                    );
                    if ui
                        .small_button(super::tr_or(
                            tr,
                            "team.agentGraph.blockingEdge.close",
                            "Close",
                        ))
                        .clicked()
                    {
                        close = true;
                    }
                });
                open_row(
                    ui,
                    &super::tr_or(
                        tr,
                        "team.agentGraph.blockingEdge.openBlockerTask",
                        "Open blocker task",
                    ),
                    from,
                    out,
                );
                ui.label(
                    RichText::new(super::tr_or(
                        tr,
                        "team.agentGraph.blockingEdge.blocks",
                        "blocks",
                    ))
                    .size(11.0)
                    .color(theme::ERROR_FG),
                );
                open_row(
                    ui,
                    &super::tr_or(
                        tr,
                        "team.agentGraph.blockingEdge.openBlockedTask",
                        "Open blocked task",
                    ),
                    to,
                    out,
                );
            });
        });
    close
}

/// Hidden tasks popover; returns true when it should close.
pub fn overflow(
    ctx: &egui::Context,
    at: Pos2,
    hidden: &[String],
    model: &GraphModel,
    out: &mut Intents,
    tr: &I18n,
) -> bool {
    let mut close = false;
    egui::Area::new(egui::Id::unique("graph_overflow_popover"))
        .order(egui::Order::Foreground)
        .fixed_pos(at)
        .show(ctx, |ui| {
            card_frame().show(ui, |ui| {
                ui.set_width(CARD_W - 20.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(super::tr_or(
                            tr,
                            "team.agentGraph.popover.overflow.hiddenTasks",
                            "Hidden tasks",
                        ))
                        .strong()
                        .size(12.5)
                        .color(theme::TEXT),
                    );
                    if ui
                        .small_button(super::tr_or(tr, "common.actions.close", "Close"))
                        .clicked()
                    {
                        close = true;
                    }
                });
                if hidden.is_empty() {
                    ui.label(
                        RichText::new(super::tr_or(
                            tr,
                            "team.agentGraph.popover.overflow.empty",
                            "No hidden tasks available.",
                        ))
                        .size(11.0)
                        .color(theme::MUTED),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .show(ui, |ui| {
                        for id in hidden {
                            let n = model.node(id);
                            ui.horizontal(|ui| {
                                if let Some(s) = n.and_then(|n| n.status) {
                                    ui.colored_label(status_color(s), "•");
                                }
                                open_row(ui, "", n, out);
                            });
                        }
                    });
            });
        });
    close
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::pos2;

    #[test]
    fn graph_popover_card_stays_inside_viewport() {
        let vp = Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0));
        let node = Rect::from_min_size(pos2(700.0, 580.0), vec2(100.0, 40.0));
        let p = card_pos(node, vp, vec2(260.0, 120.0));
        assert!(p.x >= 0.0 && p.x + 260.0 <= 800.0);
        assert!(p.y >= 0.0 && p.y + 120.0 <= 600.0 + 1.0);
        // Viewport smaller than the card: no panic, no NaN.
        let tiny = Rect::from_min_size(pos2(0.0, 0.0), vec2(100.0, 50.0));
        let p = card_pos(node, tiny, vec2(260.0, 120.0));
        assert!(p.x.is_finite() && p.y.is_finite());
    }

    #[test]
    fn graph_popover_last_line_skips_blank_and_truncates() {
        let a = AgentView {
            member: "a".into(),
            status: AgentStatus::Running { pid: 1 },
            last_lines: vec!["first".into(), "x".repeat(300), "   ".into()],
            rate_limited_until: None,
            last_error: None,
            runtime: "claude".into(),
        };
        let l = last_line(&a).unwrap();
        assert_eq!(l.chars().count(), 140);
        assert!(l.ends_with('…'));
    }
}
