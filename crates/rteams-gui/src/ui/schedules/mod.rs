//! Schedules screen (TabId::Schedules): the open team's recurring prompts with next run,
//! pause / resume, a cron editor (presets, validation, next fires) and a run log.
//!
//! Shell contract: saving or toggling a schedule needs the full `rteams_core::Schedule`
//! (cron / timezone / warm-up), which `GuiCmd::SetSchedule` cannot carry, so the screen queues
//! them in `outbox`; drain it each frame and run `Command::SetSchedule { schedule }` for each
//! `outbox.save`, and wake the member now for each `outbox.run_now` (recording a
//! `history::RunRecord`). Deleting goes through `GuiCmd::RemoveSchedule` as an `Intent`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eframe::egui;
use rteams_core::Schedule;
use rteams_services::schedule::history::{self, RunRecord};

use crate::actions::GuiCmd;
use crate::ui::{FeatureCtx, Screen, TabId};
use crate::view::Intent;

pub mod cron_editor;
pub mod list;
pub mod run_log;

use cron_editor::{Editor, EditorResult};
use list::{ListAction, ListState};
use run_log::RunLog;

const HISTORY_TTL: Duration = Duration::from_secs(3);

/// Work the screen hands to the shell.
#[derive(Debug, Default)]
pub struct Outbox {
    /// Schedules to create or replace.
    pub save: Vec<Schedule>,
    /// Schedules the user asked to run immediately.
    pub run_now: Vec<Schedule>,
}

#[derive(Default)]
pub struct Schedules {
    list: ListState,
    editor: Option<Editor>,
    log: Option<RunLog>,
    history: HashMap<String, Vec<RunRecord>>,
    refreshed: Option<(String, Instant)>,
    pub outbox: Outbox,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Schedules {
    pub fn new() -> Self {
        Self::default()
    }

    /// Take the queued saves / run-now requests.
    pub fn take_outbox(&mut self) -> Outbox {
        std::mem::take(&mut self.outbox)
    }

    fn refresh(&mut self, c: &FeatureCtx) {
        let Some(team) = c.team else { return };
        let fresh = self
            .refreshed
            .as_ref()
            .is_some_and(|(t, at)| t == team && at.elapsed() < HISTORY_TTL);
        if fresh {
            return;
        }
        self.history.clear();
        for r in history::read_all(c.home, team) {
            self.history
                .entry(r.schedule_id.clone())
                .or_default()
                .push(r);
        }
        for v in self.history.values_mut() {
            v.sort_by_key(|r| r.scheduled_for);
        }
        self.refreshed = Some((team.to_string(), Instant::now()));
    }

    fn apply(&mut self, a: ListAction, c: &mut FeatureCtx, all: &[Schedule], members: &[String]) {
        let find = |id: &str| all.iter().find(|s| s.id == id).cloned();
        match a {
            ListAction::Add => {
                self.editor = Some(Editor::new(members.first().map_or("", String::as_str)));
            }
            ListAction::Edit(id) => {
                self.editor = find(&id).map(|s| Editor::from_schedule(&s));
            }
            ListAction::Delete(id) => c.out.push(Intent::Run(GuiCmd::RemoveSchedule { id })),
            ListAction::Toggle(id, on) => {
                if let Some(mut s) = find(&id) {
                    s.enabled = on;
                    self.outbox.save.push(s);
                }
            }
            ListAction::RunNow(id) => {
                if let Some(s) = find(&id) {
                    self.outbox.run_now.push(s);
                }
            }
            ListAction::OpenLog(id) => {
                if let Some(s) = find(&id) {
                    self.log = Some(RunLog::new(&s.id, &s.member, s.timezone.clone()));
                }
            }
        }
    }
}

impl Screen for Schedules {
    fn id(&self) -> TabId {
        TabId::Schedules
    }

    fn title_key(&self) -> &'static str {
        "team.schedule.title"
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        let Some(team_name) = c.team else {
            ui.label(tr.t("team.schedule.empty.title"));
            return;
        };
        self.refresh(c);
        let (all, members): (Vec<Schedule>, Vec<String>) = match c.view {
            Some(v) => (
                v.team().map(|t| t.schedules.clone()).unwrap_or_default(),
                v.agent_members().iter().map(|m| m.name.clone()).collect(),
            ),
            None => (vec![], vec![]),
        };
        let now = now_ms();

        if let Some(ed) = self.editor.as_mut() {
            let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
            match ed.show(ui, tr, &members, now, &ids) {
                Some(EditorResult::Save(s)) => {
                    self.outbox.save.push(s);
                    self.editor = None;
                }
                Some(EditorResult::Cancel) => self.editor = None,
                None => {}
            }
        } else {
            let actions = list::show(ui, tr, &mut self.list, &all, &self.history, now);
            for a in actions {
                self.apply(a, c, &all, &members);
            }
        }
        if let Some(log) = self.log.as_mut() {
            if !log.show(ui.ctx(), tr, c.home, team_name) {
                self.log = None;
            }
        }
        // Next-run labels and run statuses move on their own.
        ui.ctx().request_repaint_after(HISTORY_TTL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing, Intents, View};
    use rteams_services::i18n::I18n;

    #[test]
    fn renders_headless_with_and_without_schedules() {
        let (home, data) = testing::fixture("sched-screen");
        let tr = I18n::english();
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut screen = Schedules::new();
        screen.editor = None;
        testing::render(|ui| {
            let mut out = Intents::default();
            let mut req = vec![];
            let mut c = FeatureCtx {
                home: &home,
                team: Some("t"),
                view: Some(&view),
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            screen.show(ui, &mut c);
        });
        // The editor form renders too.
        screen.editor = Some(Editor::new("ada"));
        testing::render(|ui| {
            let mut out = Intents::default();
            let mut req = vec![];
            let mut c = FeatureCtx {
                home: &home,
                team: Some("t"),
                view: Some(&view),
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            screen.show(ui, &mut c);
        });
        // No team open.
        testing::render(|ui| {
            let mut out = Intents::default();
            let mut req = vec![];
            let mut c = FeatureCtx {
                home: &home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            screen.show(ui, &mut c);
        });
    }

    #[test]
    fn list_actions_queue_work() {
        let (home, data) = testing::fixture("sched-actions");
        let tr = I18n::english();
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let sched = Schedule {
            id: "s1".into(),
            member: "ada".into(),
            prompt: "p".into(),
            every_minutes: 30,
            enabled: true,
            cron: None,
            timezone: None,
            warmup_minutes: None,
        };
        let mut screen = Schedules::new();
        let mut out = Intents::default();
        let mut req = vec![];
        let mut c = FeatureCtx {
            home: &home,
            team: Some("t"),
            view: Some(&view),
            out: &mut out,
            tr: &tr,
            req: &mut req,
        };
        let all = vec![sched];
        screen.apply(ListAction::Toggle("s1".into(), false), &mut c, &all, &[]);
        screen.apply(ListAction::RunNow("s1".into()), &mut c, &all, &[]);
        screen.apply(ListAction::Delete("s1".into()), &mut c, &all, &[]);
        let o = screen.take_outbox();
        assert!(!o.save[0].enabled);
        assert_eq!(o.run_now.len(), 1);
        assert_eq!(out.0.len(), 1);
    }
}
