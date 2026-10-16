//! Member work sync diagnostics: per-member agenda, fingerprint, whether a nudge would fire,
//! and Continue / Stop auto-resume controls. Read-only except those buttons; it never wakes
//! anything itself.

use eframe::egui;
use rteams_services::worksync::reconcile::Decision;
use rteams_services::worksync::{diagnose, MemberDiagnostic, WorkSync};

use crate::launcher::AgentStatus;
use crate::ui::FeatureCtx;

fn tx(c: &FeatureCtx, key: &str, default: &str) -> String {
    if c.tr.has(key) {
        c.tr.t(key)
    } else {
        default.to_string()
    }
}

#[derive(Default)]
pub struct WorkSyncPanel;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn eligible_and_running(c: &FeatureCtx) -> (Vec<String>, Vec<String>) {
    let Some(v) = c.view else {
        return (vec![], vec![]);
    };
    let eligible = v.agent_members().iter().map(|m| m.name.clone()).collect();
    let running = v
        .agents
        .iter()
        .filter(|a| matches!(a.status, AgentStatus::Running { .. }))
        .map(|a| a.member.clone())
        .collect();
    (eligible, running)
}

impl WorkSyncPanel {
    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let (Some(team), Some(view)) = (c.team, c.view) else {
            ui.weak(tx(
                c,
                "team.memberWorkSync.diagnosticsUnavailable",
                "Member work sync diagnostics are unavailable.",
            ));
            return;
        };
        let (eligible, running) = eligible_and_running(c);
        let diags = diagnose(c.home, team, view.state, &eligible, &running, now_ms());
        let ws = WorkSync::new(c.home, team);
        let (
            t_title,
            t_items,
            t_fp,
            t_would,
            t_yes,
            t_no,
            t_none,
            t_cont,
            t_stop,
            t_resume,
            t_stopped,
        ) = (
            tx(c, "team.memberWorkSync.title", "Member work sync"),
            tx(
                c,
                "team.memberWorkSync.details.actionableItems",
                "Actionable items",
            ),
            tx(c, "team.memberWorkSync.details.fingerprint", "Fingerprint"),
            tx(
                c,
                "team.memberWorkSync.details.shadowWouldNudge",
                "Shadow would nudge",
            ),
            tx(c, "team.memberWorkSync.details.yes", "yes"),
            tx(c, "team.memberWorkSync.details.no", "no"),
            tx(c, "team.memberWorkSync.details.none", "none"),
            tx(c, "team.memberWorkSync.details.continue", "Continue"),
            tx(
                c,
                "team.memberWorkSync.details.stopAutoResume",
                "Stop auto-resume",
            ),
            tx(
                c,
                "team.memberWorkSync.details.resumeAutoResume",
                "Resume auto-resume",
            ),
            tx(
                c,
                "team.memberWorkSync.details.autoResumeStopped",
                "Automatic continuation is stopped.",
            ),
        );
        ui.heading(t_title);
        for d in &diags {
            card(
                ui,
                d,
                &t_items,
                &t_fp,
                &t_would,
                &t_yes,
                &t_no,
                &t_none,
                &t_stopped,
                |ui, d| {
                    ui.horizontal(|ui| {
                        if ui.button(&t_cont).clicked() {
                            ws.continue_now(&d.member);
                        }
                        let label = if d.auto_resume_stopped {
                            &t_resume
                        } else {
                            &t_stop
                        };
                        if ui.button(label).clicked() {
                            ws.set_auto_resume_stopped(&d.member, !d.auto_resume_stopped);
                        }
                    });
                },
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut egui::Ui,
    d: &MemberDiagnostic,
    t_items: &str,
    t_fp: &str,
    t_would: &str,
    t_yes: &str,
    t_no: &str,
    t_none: &str,
    t_stopped: &str,
    buttons: impl FnOnce(&mut egui::Ui, &MemberDiagnostic),
) {
    ui.group(|ui| {
        ui.strong(&d.member);
        ui.label(format!("Diagnostics: {}", d.status_text()));
        ui.label(format!(
            "{t_would}: {}",
            if d.decision == Decision::Nudge {
                t_yes
            } else {
                t_no
            }
        ));
        ui.label(format!(
            "{t_fp}: {}",
            if d.fingerprint.is_empty() {
                t_none
            } else {
                &d.fingerprint
            }
        ));
        ui.label(format!(
            "{t_items}: {}",
            if d.items.is_empty() {
                t_none.to_string()
            } else {
                d.items.len().to_string()
            }
        ));
        for i in d.items.iter().take(5) {
            ui.weak(format!("#{} {} ({})", i.task, i.subject, i.reason));
        }
        if d.items.len() > 5 {
            ui.weak(format!("{} more actionable item(s)", d.items.len() - 5));
        }
        if d.auto_resume_stopped {
            ui.colored_label(ui.visuals().warn_fg_color, t_stopped);
        }
        buttons(ui, d);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing, Intents, View};
    use rteams_services::i18n::I18n;

    #[test]
    fn work_sync_panel_renders() {
        let (home, d) = testing::fixture("work-sync-panel");
        let tr = I18n::english();
        let view = View {
            state: d.state(),
            data: &d,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut out = Intents::default();
        let mut req = vec![];
        let mut p = WorkSyncPanel;
        testing::render(|ui| {
            let mut c = FeatureCtx {
                home: &home,
                team: Some("t"),
                view: Some(&view),
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            p.show(ui, &mut c);
        });
        let mut c = FeatureCtx {
            home: &home,
            team: None,
            view: None,
            out: &mut out,
            tr: &tr,
            req: &mut req,
        };
        testing::render(|ui| p.show(ui, &mut c));
        assert!(out.0.is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }
}
