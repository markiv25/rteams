//! Team settings modal with tabs: General (project path, lead prompt, git), Permissions,
//! Schedules and Notifications.

use eframe::egui::{self, Color32, RichText};
use rteams_core::Schedule;

use crate::actions::GuiCmd;
use crate::notifications::NotificationCenter;
use crate::theme;
use crate::view::{Intent, Intents, View};
use rteams_services::i18n::I18n;

/// Permission modes the shell persists and passes to agents.
pub const PERM_SKIP: &str = "skip";
pub const PERM_EDITS: &str = "edits";
pub const PERM_ASK: &str = "ask";

/// `(mode, label key, hint key)`.
const PERM_CHOICES: [(&str, &str, &str); 3] = [
    (PERM_SKIP, "rteams.perm.skip", "rteams.perm.skipHint"),
    (PERM_EDITS, "rteams.perm.edits", "rteams.perm.editsHint"),
    (PERM_ASK, "rteams.perm.ask", "rteams.perm.askHint"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    General,
    Permissions,
    Schedules,
    Notifications,
}

#[derive(Debug, Default)]
pub struct SettingsDialog {
    is_open: bool,
    tab: Tab,
    path: String,
    prompt: String,
    perm: String,
    /// Set when `Save` was pressed; read by the shell via `permissions_choice`.
    perm_saved: Option<String>,
    sched_member: String,
    sched_minutes: u32,
    sched_prompt: String,
}

/// `s1`, `s2`, ...: the next free schedule id.
pub fn next_schedule_id(existing: &[Schedule]) -> String {
    let n = existing
        .iter()
        .filter_map(|s| s.id.strip_prefix('s')?.parse::<u32>().ok())
        .max()
        .unwrap_or(0);
    format!("s{}", n + 1)
}

/// "every 30 min", "every 2 h", "every 1 h 30 min".
pub fn every_label(tr: &I18n, minutes: u32) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    let (h, m) = (h.to_string(), m.to_string());
    match (minutes / 60, minutes % 60) {
        (0, _) => tr.t_args("rteams.settings.everyMin", &[("m", &m)]),
        (_, 0) => tr.t_args("rteams.settings.everyH", &[("h", &h)]),
        _ => tr.t_args("rteams.settings.everyHM", &[("h", &h), ("m", &m)]),
    }
}

/// Label of the git state line.
pub fn branch_label(tr: &I18n, branch: Option<&str>) -> String {
    match branch {
        Some(b) if !b.trim().is_empty() => {
            tr.t_args("rteams.settings.branch", &[("branch", b.trim())])
        }
        _ => tr.t("rteams.settings.notRepo"),
    }
}

impl SettingsDialog {
    /// Open the dialog, prefilled from the team. `perm` = the current permission mode
    /// (`skip`, `edits` or `ask`).
    pub fn open(&mut self, v: &View, perm: &str) {
        let t = v.team();
        self.path = t.and_then(|t| t.project_path.clone()).unwrap_or_default();
        self.prompt = t.and_then(|t| t.lead_prompt.clone()).unwrap_or_default();
        self.perm = normalize_perm(perm).to_string();
        self.perm_saved = None;
        self.sched_member = v
            .agent_members()
            .first()
            .map(|m| m.name.clone())
            .unwrap_or_default();
        self.sched_minutes = 60;
        self.sched_prompt.clear();
        self.tab = Tab::General;
        self.is_open = true;
    }

    pub fn close(&mut self) {
        self.is_open = false;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.is_open
    }

    /// The permission mode chosen when `Save` was last pressed (`skip` / `edits` / `ask`);
    /// `None` until then. The shell persists it and applies it to the next launches.
    #[cfg(test)]
    pub fn permissions_choice(&self) -> Option<&str> {
        self.perm_saved.as_deref()
    }

    /// Like `permissions_choice`, but consumes it so the shell acts once.
    pub fn take_permissions_choice(&mut self) -> Option<String> {
        self.perm_saved.take()
    }

    /// The write that `Save` produces (empty fields clear the setting with `Some("")`).
    fn command(&self) -> GuiCmd {
        GuiCmd::ConfigureTeam {
            project_path: Some(self.path.trim().to_string()),
            lead_prompt: Some(self.prompt.trim().to_string()),
        }
    }

    /// `branch` = current git branch of the project path, `None` when it is not a repo.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        v: &View,
        branch: Option<&str>,
        center: &mut NotificationCenter,
        out: &mut Intents,
    ) {
        if !self.is_open {
            return;
        }
        let screen = ctx.content_rect();
        let width = (screen.width() * 0.92).clamp(1.0, 560.0);
        let max_h = (screen.height() * 0.9).max(1.0);
        let modal = egui::Modal::new(egui::Id::unique("team_settings"))
            .backdrop_color(Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                let mut close = false;
                egui::ScrollArea::vertical()
                    .max_height(max_h)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(v.tr.t("rteams.settings.title"))
                                .size(18.0)
                                .strong(),
                        );
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            for (tab, label) in [
                                (Tab::General, v.tr.t("settings.tabs.general.label")),
                                (Tab::Permissions, v.tr.t("rteams.settings.permissions")),
                                (Tab::Schedules, v.tr.t("common.layout.menu.schedules")),
                                (Tab::Notifications, v.tr.t("common.notifications.title")),
                            ] {
                                ui.selectable_value(&mut self.tab, tab, label);
                            }
                        });
                        theme::rule(ui);
                        match self.tab {
                            Tab::General => self.general(ui, v, branch, out),
                            Tab::Permissions => self.permissions(ui, v.tr),
                            Tab::Schedules => self.schedules(ui, v, out),
                            Tab::Notifications => center.prefs_ui(ui, v.tr),
                        }
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            let can = v.actor.is_some();
                            let save = ui
                                .add_enabled_ui(can, |ui| {
                                    theme::solid(ui, &v.tr.t("common.actions.save"), theme::INFO)
                                })
                                .inner;
                            if save.clicked() {
                                out.push(Intent::Run(self.command()));
                                self.perm_saved = Some(self.perm.clone());
                                close = true;
                            }
                            if ui.button(v.tr.t("common.actions.cancel")).clicked() {
                                close = true;
                            }
                        });
                    });
                close
            });
        if modal.inner || modal.should_close() {
            self.close();
        }
    }

    fn general(&mut self, ui: &mut egui::Ui, v: &View, branch: Option<&str>, out: &mut Intents) {
        ui.label(RichText::new(v.tr.t("team.teamImport.projectPath")).color(theme::TEXT_2));
        ui.add(
            egui::TextEdit::singleline(&mut self.path)
                .hint_text(v.tr.t("rteams.settings.pathHint"))
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(branch_label(v.tr, branch))
                    .size(12.0)
                    .color(theme::MUTED),
            );
            let can = v.actor.is_some() && !self.path.trim().is_empty();
            if branch.is_none()
                && ui
                    .add_enabled(can, egui::Button::new(v.tr.t("rteams.settings.gitInit")))
                    .clicked()
            {
                out.push(Intent::GitInit);
            }
            if ui
                .add_enabled(can, egui::Button::new(v.tr.t("rteams.settings.gitCommit")))
                .clicked()
            {
                out.push(Intent::GitInitialCommit);
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new(v.tr.t("team.teamImport.leadPrompt")).color(theme::TEXT_2));
        ui.add(
            egui::TextEdit::multiline(&mut self.prompt)
                .hint_text(v.tr.t("rteams.settings.leadHint"))
                .desired_rows(6)
                .desired_width(f32::INFINITY),
        );
    }

    fn permissions(&mut self, ui: &mut egui::Ui, tr: &I18n) {
        ui.label(RichText::new(tr.t("rteams.perm.title")).color(theme::TEXT_2));
        for (value, label, hint) in PERM_CHOICES {
            let (label, hint) = (tr.t(label), tr.t(hint));
            ui.radio_value(&mut self.perm, value.to_string(), label)
                .on_hover_text(&hint);
            ui.label(RichText::new(hint).size(11.5).color(theme::MUTED));
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(tr.t("rteams.perm.applies"))
                .size(11.5)
                .color(theme::MUTED),
        );
    }

    fn schedules(&mut self, ui: &mut egui::Ui, v: &View, out: &mut Intents) {
        let list = v.team().map_or(&[][..], |t| t.schedules.as_slice());
        if list.is_empty() {
            ui.label(RichText::new(v.tr.t("team.schedule.empty.title")).color(theme::MUTED));
        }
        for s in list {
            ui.horizontal_wrapped(|ui| {
                let mut on = s.enabled;
                if ui.checkbox(&mut on, "").changed() {
                    let mut s = s.clone();
                    s.enabled = on;
                    out.push(Intent::Run(GuiCmd::set_schedule(&s)));
                }
                ui.label(RichText::new(&s.member).strong().color(v.color(&s.member)));
                ui.label(RichText::new(every_label(v.tr, s.every_minutes)).color(theme::TEXT_2));
                if theme::ghost(ui, &v.tr.t("common.schedules.actions.delete")).clicked() {
                    out.push(Intent::Run(GuiCmd::RemoveSchedule { id: s.id.clone() }));
                }
            });
            ui.label(RichText::new(&s.prompt).size(12.0).color(theme::MUTED));
            theme::rule(ui);
        }
        ui.add_space(6.0);
        ui.label(RichText::new(v.tr.t("team.schedule.actions.addSchedule")).color(theme::TEXT_2));
        let members = v.agent_members();
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("sched_member")
                .selected_text(self.sched_member.clone())
                .show_ui(ui, |ui| {
                    for m in &members {
                        ui.selectable_value(&mut self.sched_member, m.name.clone(), &m.name);
                    }
                });
            ui.label(v.tr.t("rteams.settings.every"));
            ui.add(
                egui::DragValue::new(&mut self.sched_minutes)
                    .range(1..=10_080)
                    .suffix(format!(" {}", v.tr.t("rteams.settings.min"))),
            );
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.sched_prompt)
                .hint_text(v.tr.t("rteams.settings.promptToSend"))
                .desired_width(f32::INFINITY),
        );
        let ok = v.actor.is_some()
            && !self.sched_member.is_empty()
            && !self.sched_prompt.trim().is_empty();
        if ui
            .add_enabled(
                ok,
                egui::Button::new(v.tr.t("team.schedule.actions.addSchedule")),
            )
            .clicked()
        {
            out.push(Intent::Run(GuiCmd::SetSchedule {
                id: next_schedule_id(list),
                member: self.sched_member.clone(),
                prompt: self.sched_prompt.trim().to_string(),
                every_minutes: self.sched_minutes.max(1),
                enabled: true,
                cron: None,
                timezone: None,
                warmup_minutes: None,
            }));
            self.sched_prompt.clear();
        }
    }
}

/// Anything unknown means the default.
pub fn normalize_perm(s: &str) -> &'static str {
    match s {
        PERM_EDITS => PERM_EDITS,
        PERM_ASK => PERM_ASK,
        _ => PERM_SKIP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};

    #[test]
    fn pure_helpers() {
        assert_eq!(normalize_perm("bogus"), "skip");
        assert_eq!(normalize_perm("ask"), "ask");
        assert_eq!(every_label(crate::view::testing::tr(), 30), "every 30 min");
        assert_eq!(every_label(crate::view::testing::tr(), 120), "every 2 h");
        assert_eq!(
            every_label(crate::view::testing::tr(), 90),
            "every 1 h 30 min"
        );
        assert_eq!(
            branch_label(crate::view::testing::tr(), Some("main")),
            "Branch: main"
        );
        assert_eq!(
            branch_label(crate::view::testing::tr(), None),
            "Not a git repository"
        );
        let mk = |id: &str| Schedule {
            id: id.into(),
            member: "a".into(),
            prompt: "p".into(),
            every_minutes: 5,
            enabled: true,
            cron: None,
            timezone: None,
            warmup_minutes: None,
        };
        assert_eq!(next_schedule_id(&[]), "s1");
        assert_eq!(next_schedule_id(&[mk("s2"), mk("x"), mk("s9")]), "s10");
    }

    #[test]
    fn renders_every_tab_and_builds_command() {
        let (home, data) = fixture("settings");
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut d = SettingsDialog::default();
        assert!(!d.is_open());
        assert_eq!(d.permissions_choice(), None);
        d.open(&v, "ask");
        assert!(d.is_open());
        assert_eq!(d.perm, "ask");
        d.path = " /work ".into();
        d.prompt = "be kind".into();
        assert_eq!(
            d.command(),
            GuiCmd::ConfigureTeam {
                project_path: Some("/work".into()),
                lead_prompt: Some("be kind".into())
            }
        );
        let mut center = NotificationCenter::default();
        for tab in [
            Tab::General,
            Tab::Permissions,
            Tab::Schedules,
            Tab::Notifications,
        ] {
            d.tab = tab;
            for branch in [Some("main"), None] {
                render(|ui| d.show(ui.ctx(), &v, branch, &mut center, &mut Intents::default()));
            }
        }
        let _ = std::fs::remove_dir_all(home);
    }
}
