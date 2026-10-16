//! Launch details HUD (bottom-left): which members are waiting to start, running or failed.
//! Only shown while a launch is incomplete (some agents exist but not all are running).

use eframe::egui::{self, Rect, RichText};
use rteams_services::i18n::I18n;

use crate::launcher::AgentStatus;
use crate::theme;
use crate::view::View;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spawn {
    Running,
    WaitingToStart,
    Failed,
    Exited,
}

/// Spawn state per agent member; empty when nothing was launched at all.
pub fn spawn_states(view: &View) -> Vec<(String, Spawn)> {
    if view.agents.is_empty() {
        return Vec::new();
    }
    view.agent_members()
        .iter()
        .map(|m| {
            let s = match view.agent(&m.name).map(|a| &a.status) {
                Some(AgentStatus::Running { .. }) => Spawn::Running,
                Some(AgentStatus::Failed(_)) => Spawn::Failed,
                Some(AgentStatus::Exited { .. }) => Spawn::Exited,
                None => Spawn::WaitingToStart,
            };
            (m.name.clone(), s)
        })
        .collect()
}

/// Whether the HUD has anything to say.
pub fn needed(states: &[(String, Spawn)]) -> bool {
    states
        .iter()
        .any(|(_, s)| matches!(s, Spawn::WaitingToStart | Spawn::Failed))
}

pub fn show(ctx: &egui::Context, viewport: Rect, view: &View, tr: &I18n) {
    let states = spawn_states(view);
    let w = (viewport.width() - 24.0).clamp(0.0, 240.0);
    if !needed(&states) || w < 120.0 {
        return;
    }
    let y = viewport.bottom() - 12.0 - (states.len().min(6) as f32 * 18.0 + 40.0);
    egui::Area::new(egui::Id::unique("graph_provisioning_hud"))
        .order(egui::Order::Middle)
        .fixed_pos(egui::pos2(
            viewport.left() + 12.0,
            y.max(viewport.top() + 12.0),
        ))
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(theme::tint(theme::SIDEBAR, 0.9))
                .stroke(egui::Stroke::new(1.0, theme::hairline(0.15)))
                .corner_radius(8)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(w - 16.0);
                    ui.label(
                        RichText::new(super::tr_or(
                            tr,
                            "team.agentGraph.provisioning.launchDetails",
                            "Launch details",
                        ))
                        .strong()
                        .size(12.0)
                        .color(theme::TEXT),
                    );
                    for (name, s) in states.iter().take(6) {
                        let (key, fallback, color) = match s {
                            Spawn::Running => (
                                "team.agentGraph.popover.member.state.active",
                                "active",
                                theme::SUCCESS,
                            ),
                            Spawn::WaitingToStart => (
                                "team.agentGraph.popover.member.spawn.waitingToStart",
                                "waiting to start",
                                theme::MUTED,
                            ),
                            Spawn::Failed => (
                                "team.agentGraph.popover.member.spawn.failed",
                                "failed",
                                theme::ERROR,
                            ),
                            Spawn::Exited => (
                                "team.agentGraph.popover.member.state.offline",
                                "offline",
                                theme::WARNING,
                            ),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(name).size(11.0).color(theme::TEXT_2));
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(super::tr_or(tr, key, fallback))
                                    .size(11.0)
                                    .color(color),
                            );
                        });
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_provisioning_needed_only_when_incomplete() {
        let r = |s| vec![("a".to_string(), Spawn::Running), ("b".to_string(), s)];
        assert!(needed(&r(Spawn::WaitingToStart)));
        assert!(needed(&r(Spawn::Failed)));
        assert!(!needed(&r(Spawn::Running)));
        assert!(!needed(&r(Spawn::Exited)));
        assert!(!needed(&[]));
    }
}
