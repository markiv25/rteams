//! Running teams section: teams whose agents are running, sorted active < launching < running
//! (idle), then more in-progress tasks first, then latest activity (`buildRunningTeamsDashboard`).

use std::collections::HashSet;
use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_core::Status;
use rteams_store::FileStore;

use crate::icons::Icon;
use crate::theme;
use crate::ui::dashboard::kit;
use crate::ui::{FeatureCtx, ShellRequest};

/// A run file or `sessions.json` touched within this window counts as running.
pub const RECENT_RUN_MS: i64 = 2 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RunStatus {
    Active,
    Provisioning,
    Idle,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TaskCounts {
    pub pending: usize,
    pub in_progress: usize,
    pub completed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningCandidate {
    /// Directory name under the home.
    pub dir: String,
    pub display_name: String,
    pub project_path: Option<String>,
    pub last_activity_ms: i64,
    pub status: RunStatus,
    pub counts: TaskCounts,
    pub deleted: bool,
}

/// Keep running teams (anything but offline) and order them; ported 1:1 from the original.
pub fn build_running_teams(mut teams: Vec<RunningCandidate>) -> Vec<RunningCandidate> {
    teams.retain(|t| t.status != RunStatus::Offline && !t.deleted);
    teams.sort_by(|a, b| {
        a.status
            .cmp(&b.status)
            .then(b.counts.in_progress.cmp(&a.counts.in_progress))
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
            .then_with(|| a.display_name.cmp(&b.display_name))
    });
    teams
}

fn mtime_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as i64)
}

/// Newest mtime of any `<team>/runs/<member>/*.jsonl` and `<team>/sessions.json`.
pub fn last_run_activity_ms(team_dir: &Path) -> i64 {
    let mut newest = mtime_ms(&team_dir.join("sessions.json"));
    if let Ok(members) = std::fs::read_dir(team_dir.join("runs")) {
        for m in members.flatten() {
            if let Ok(files) = std::fs::read_dir(m.path()) {
                for f in files.flatten() {
                    newest = newest.max(mtime_ms(&f.path()));
                }
            }
        }
    }
    newest
}

/// Everything the dashboard shows about every team under `home`. `launched` are teams whose
/// agents this process runs, `provisioning` those being launched right now.
pub fn collect_candidates(
    home: &Path,
    launched: &HashSet<String>,
    provisioning: &HashSet<String>,
    now_ms: i64,
) -> Vec<RunningCandidate> {
    let Ok(rd) = std::fs::read_dir(home) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let dir = e.file_name().to_string_lossy().into_owned();
        let team_dir = e.path();
        let log = team_dir.join("events.jsonl");
        if !log.is_file() {
            continue;
        }
        let Ok(store) = FileStore::open_read_only(home, &dir) else {
            continue;
        };
        let s = store.loaded_state();
        let mut counts = TaskCounts::default();
        for t in s.tasks.iter().filter(|t| !t.archived) {
            match t.status {
                Status::InProgress => counts.in_progress += 1,
                Status::Done | Status::Approved => counts.completed += 1,
                _ => counts.pending += 1,
            }
        }
        let run_ms = last_run_activity_ms(&team_dir);
        let recent_run = now_ms - run_ms <= RECENT_RUN_MS;
        let status = if provisioning.contains(&dir) {
            RunStatus::Provisioning
        } else if launched.contains(&dir) || recent_run {
            if counts.in_progress > 0 || recent_run {
                RunStatus::Active
            } else {
                RunStatus::Idle
            }
        } else {
            RunStatus::Offline
        };
        out.push(RunningCandidate {
            display_name: s
                .team
                .as_ref()
                .map_or_else(|| dir.clone(), |t| t.name.clone()),
            project_path: s.team.as_ref().and_then(|t| t.project_path.clone()),
            deleted: s.team.as_ref().is_some_and(|t| t.deleted),
            last_activity_ms: mtime_ms(&log).max(run_ms),
            status,
            counts,
            dir,
        });
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

pub fn status_color(s: RunStatus) -> egui::Color32 {
    match s {
        RunStatus::Active => theme::SUCCESS,
        RunStatus::Provisioning => theme::WARNING,
        RunStatus::Idle => theme::INFO,
        RunStatus::Offline => theme::MUTED,
    }
}

pub fn status_key(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Active => "team.runningTeams.status.active",
        RunStatus::Provisioning => "team.runningTeams.status.provisioning",
        RunStatus::Idle => "team.runningTeams.status.idle",
        RunStatus::Offline => "team.list.status.offline",
    }
}

/// Section with one card per running team. Draws nothing when none run.
pub fn show(ui: &mut egui::Ui, c: &mut FeatureCtx, running: &[RunningCandidate]) {
    if running.is_empty() {
        return;
    }
    kit::heading(
        ui,
        Icon::Play,
        &c.tr.t("team.runningTeams.title"),
        Some(running.len()),
    );
    ui.add_space(8.0);
    kit::grid(ui, 300.0, running, |ui, w, t| card(ui, c, t, w));
    ui.add_space(8.0);
}

fn card(ui: &mut egui::Ui, c: &mut FeatureCtx, t: &RunningCandidate, w: f32) {
    let col = status_color(t.status);
    let resp = kit::tile(ui, ("running", &t.dir), w, 132.0, false, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, col);
            ui.add(egui::Label::new(RichText::new(&t.display_name).size(15.0).strong()).truncate());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::pill(ui, c.tr.t(status_key(t.status)), col);
            });
        });
        let path = t
            .project_path
            .clone()
            .unwrap_or_else(|| c.tr.t("team.runningTeams.noProject"));
        ui.add(egui::Label::new(RichText::new(path).size(12.5).color(theme::MUTED)).truncate());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            theme::pill(ui, format!("{} pending", t.counts.pending), theme::TEXT_2);
            theme::pill(
                ui,
                format!("{} active", t.counts.in_progress),
                theme::WARNING,
            );
            theme::pill(ui, format!("{} done", t.counts.completed), theme::COL_DONE);
        });
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            if theme::pill_icon_button(
                ui,
                Some(Icon::Refresh),
                &c.tr.t("team.list.actions.relaunchTeam"),
                false,
            )
            .clicked()
            {
                c.req.push(ShellRequest::LaunchTeam(t.dir.clone()));
            }
        });
    });
    if resp.clicked() {
        c.req.push(ShellRequest::OpenTeam(t.dir.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(n: &str, st: RunStatus, ip: usize, ms: i64) -> RunningCandidate {
        RunningCandidate {
            dir: n.into(),
            display_name: n.into(),
            project_path: None,
            last_activity_ms: ms,
            status: st,
            counts: TaskCounts {
                in_progress: ip,
                ..Default::default()
            },
            deleted: false,
        }
    }

    #[test]
    fn sorts_status_then_progress_then_recency() {
        let v = build_running_teams(vec![
            cand("idle", RunStatus::Idle, 5, 9),
            cand("off", RunStatus::Offline, 9, 9),
            cand("prov", RunStatus::Provisioning, 0, 1),
            cand("a-old", RunStatus::Active, 1, 1),
            cand("a-new", RunStatus::Active, 1, 2),
            cand("a-busy", RunStatus::Active, 3, 0),
        ]);
        let n: Vec<_> = v.iter().map(|t| t.dir.as_str()).collect();
        assert_eq!(n, ["a-busy", "a-new", "a-old", "prov", "idle"]);
    }

    #[test]
    fn collects_running_state_from_disk() {
        let (home, _d) = crate::view::testing::fixture("dash-run");
        let none = HashSet::new();
        let now = 1_000_000_000_000_i64;
        let far = i64::MAX / 2;
        // Not launched, no recent run files -> offline.
        let c = collect_candidates(&home, &none, &none, far);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].status, RunStatus::Offline);
        assert_eq!(c[0].counts.in_progress, 1);
        // Launched -> active (tasks in progress).
        let l: HashSet<String> = ["t".to_string()].into();
        assert_eq!(
            collect_candidates(&home, &l, &none, far)[0].status,
            RunStatus::Active
        );
        // sessions.json just written -> running without being launched here.
        std::fs::write(home.join("t/sessions.json"), "{}").unwrap();
        let real_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert_eq!(
            collect_candidates(&home, &none, &none, real_now)[0].status,
            RunStatus::Active
        );
        let p: HashSet<String> = ["t".to_string()].into();
        assert_eq!(
            collect_candidates(&home, &none, &p, now)[0].status,
            RunStatus::Provisioning
        );
        let _ = std::fs::remove_dir_all(home);
    }
}
