//! Global task sidebar: tasks of every team under the home, with team / status / owner filters.
//! Clicking a row asks the shell to switch to that team and open the task.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_core::Status;

use crate::theme;

const REFRESH: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct Row {
    pub team: String,
    pub team_name: String,
    pub id: String,
    pub subject: String,
    pub status: Status,
    pub owner: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pick {
    pub team: String,
    pub task: String,
}

#[derive(Default)]
pub struct TaskSidebar {
    open: bool,
    rows: Vec<Row>,
    loaded: Option<Instant>,
    team: Option<String>,
    status: Option<Status>,
    owner: Option<String>,
}

/// Every live task of every team under `home` (soft-deleted teams are skipped).
pub fn load(home: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    for card in crate::teams::scan(home).into_iter().filter(|c| !c.deleted) {
        let Ok(store) = rteams_store::FileStore::open_read_only(home, &card.dir) else {
            continue;
        };
        let name = card.name.clone().unwrap_or_else(|| card.dir.clone());
        for t in store.loaded_state().tasks.iter().filter(|t| !t.archived) {
            rows.push(Row {
                team: card.dir.clone(),
                team_name: name.clone(),
                id: t.id.clone(),
                subject: t.subject.clone(),
                status: t.status,
                owner: t.owner.clone(),
            });
        }
    }
    rows
}

impl TaskSidebar {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.loaded = None;
    }

    fn matches(&self, r: &Row) -> bool {
        self.team.as_ref().is_none_or(|t| *t == r.team)
            && self.status.is_none_or(|s| s == r.status)
            && self
                .owner
                .as_ref()
                .is_none_or(|o| r.owner.as_ref() == Some(o))
    }

    /// Draw as a right panel; `Some` when a row was clicked.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        home: &Path,
        current: &str,
        tr: &rteams_services::i18n::I18n,
    ) -> Option<Pick> {
        if !self.open {
            return None;
        }
        if self.loaded.is_none_or(|t| t.elapsed() > REFRESH) {
            self.rows = load(home);
            self.loaded = Some(Instant::now());
        }
        let mut picked = None;
        let mut close = false;
        egui::Panel::right("task_sidebar")
            .exact_size(280.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(10, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(tr.t("rteams.shell.tasks.title"))
                            .strong()
                            .color(theme::TEXT),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(
                            ui,
                            crate::icons::Icon::X,
                            &tr.t("common.actions.close"),
                        )
                        .clicked()
                        {
                            close = true;
                        }
                    });
                });
                let mut teams: Vec<(String, String)> = Vec::new();
                let mut owners: Vec<String> = Vec::new();
                for r in &self.rows {
                    if !teams.iter().any(|(d, _)| *d == r.team) {
                        teams.push((r.team.clone(), r.team_name.clone()));
                    }
                    if let Some(o) = &r.owner {
                        if !owners.contains(o) {
                            owners.push(o.clone());
                        }
                    }
                }
                owners.sort();
                let all_teams = tr.t("rteams.shell.tasks.allTeams");
                let all_status = tr.t("rteams.shell.tasks.allStatuses");
                let all_owners = tr.t("rteams.shell.tasks.allOwners");
                ui.horizontal_wrapped(|ui| {
                    let cur = teams
                        .iter()
                        .find(|(d, _)| Some(d) == self.team.as_ref())
                        .map_or(all_teams.clone(), |t| t.1.clone());
                    egui::ComboBox::from_id_salt("ts_team")
                        .selected_text(cur)
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.team, None, &all_teams);
                            for (d, n) in &teams {
                                ui.selectable_value(&mut self.team, Some(d.clone()), n);
                            }
                        });
                    let cur = self.status.map_or(all_status.clone(), |s| {
                        super::super::task_dialog::status_pill(tr, s).0
                    });
                    egui::ComboBox::from_id_salt("ts_status")
                        .selected_text(cur)
                        .width(100.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.status, None, &all_status);
                            for s in Status::ALL {
                                let l = super::super::task_dialog::status_pill(tr, s).0;
                                ui.selectable_value(&mut self.status, Some(s), l);
                            }
                        });
                    egui::ComboBox::from_id_salt("ts_owner")
                        .selected_text(self.owner.clone().unwrap_or_else(|| all_owners.clone()))
                        .width(100.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.owner, None, &all_owners);
                            for o in &owners {
                                ui.selectable_value(&mut self.owner, Some(o.clone()), o);
                            }
                        });
                });
                theme::rule(ui);
                let shown: Vec<&Row> = self.rows.iter().filter(|r| self.matches(r)).collect();
                if shown.is_empty() {
                    ui.label(RichText::new(tr.t("rteams.shell.tasks.empty")).color(theme::MUTED));
                }
                egui::ScrollArea::vertical()
                    .id_salt("task_sidebar_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for r in shown.iter().take(300) {
                            let (label, color) =
                                super::super::task_dialog::status_pill(tr, r.status);
                            let resp = egui::Frame::new()
                                .fill(if r.team == current {
                                    theme::tint(theme::ACCENT, 0.07)
                                } else {
                                    theme::CARD
                                })
                                .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
                                .corner_radius(egui::CornerRadius::same(8))
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(
                                        RichText::new(format!(
                                            "{} \u{00b7} #{}",
                                            r.team_name, r.id
                                        ))
                                        .size(11.0)
                                        .color(theme::MUTED),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&r.subject).color(theme::TEXT),
                                        )
                                        .truncate(),
                                    );
                                    ui.horizontal(|ui| {
                                        theme::pill(ui, label, color);
                                        if let Some(o) = &r.owner {
                                            ui.label(
                                                RichText::new(o).size(11.5).color(theme::TEXT_2),
                                            );
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click())
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if resp.clicked() {
                                picked = Some(Pick {
                                    team: r.team.clone(),
                                    task: r.id.clone(),
                                });
                            }
                            ui.add_space(4.0);
                        }
                    });
            });
        if close {
            self.open = false;
        }
        picked
    }
}
