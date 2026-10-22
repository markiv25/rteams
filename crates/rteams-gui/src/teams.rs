//! Teams list (the original's `Teams` tab): every team under the home as a card, with search
//! and `Create Team`. Picking or creating one pushes `Intent::SwitchTeam`; a new name lands on
//! the create screen, which writes nothing until submitted.

use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_core::Status;
use rteams_services::i18n::I18n;
use rteams_store::FileStore;

use crate::icons::Icon;
use crate::theme;
use crate::ui::dashboard::{kit, recent_projects, running_teams};
use crate::ui::import_team_dialog::ImportTeamDialog;
use crate::view::{Intent, Intents};
use rteams_services::team_import::TeamDraft;

/// One card's worth of facts about a team directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamCard {
    pub dir: String,
    pub name: Option<String>,
    pub members: usize,
    pub open_tasks: usize,
    pub done_tasks: usize,
    /// Soft-deleted: listed under "Deleted" with a Restore button.
    pub deleted: bool,
    /// Non-removed members as `(name, role)`, roster order.
    pub roster: Vec<(String, String)>,
    /// Last project path, shown as the card's subtitle.
    pub project: Option<String>,
    /// Newest of the log and any run file, unix ms.
    pub last_activity_ms: i64,
    /// Agents of this team ran within the last two minutes.
    pub running: bool,
}

/// Team directories under `home` that hold a log, sorted by name. Unreadable ones are skipped.
pub fn scan(home: &Path) -> Vec<TeamCard> {
    let Ok(entries) = std::fs::read_dir(home) else {
        return Vec::new();
    };
    let mut cards: Vec<TeamCard> = entries
        .flatten()
        .filter(|e| e.path().join("events.jsonl").is_file())
        .filter_map(|e| {
            let dir = e.file_name().to_string_lossy().into_owned();
            let store = FileStore::open_read_only(home, &dir).ok()?;
            let s = store.loaded_state();
            let live = s.tasks.iter().filter(|t| !t.archived);
            let done = live
                .clone()
                .filter(|t| matches!(t.status, Status::Done | Status::Approved))
                .count();
            let team_dir = e.path();
            let log_ms = std::fs::metadata(team_dir.join("events.jsonl"))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as i64);
            let run_ms = running_teams::last_run_activity_ms(&team_dir);
            let now = recent_projects::now_ms();
            Some(TeamCard {
                roster: s
                    .team
                    .as_ref()
                    .map(|t| {
                        t.members
                            .iter()
                            .filter(|m| !m.removed)
                            .map(|m| (m.name.clone(), m.role.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                project: s.team.as_ref().and_then(|t| t.project_path.clone()),
                last_activity_ms: log_ms.max(run_ms),
                running: now - run_ms <= running_teams::RECENT_RUN_MS,
                name: s.team.as_ref().map(|t| t.name.clone()),
                members: s.team.as_ref().map_or(0, |t| t.members.len()),
                open_tasks: live.count() - done,
                done_tasks: done,
                deleted: s.team.as_ref().is_some_and(|t| t.deleted),
                dir,
            })
        })
        .collect();
    cards.sort_by(|a, b| a.dir.cmp(&b.dir));
    cards
}

/// Something the Teams header asked the shell to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeamsAction {
    /// The `Org Map` button: open the organization map tab.
    OpenOrgMap,
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Filter {
    #[default]
    All,
    Running,
    Offline,
}

#[derive(Default)]
pub struct Teams {
    search: String,
    new_name: String,
    filter: Filter,
    filter_open: bool,
    create_open: bool,
    import: ImportTeamDialog,
    draft: Option<TeamDraft>,
    action: Option<TeamsAction>,
    /// Rescanned when the view opens, not every frame.
    cards: Option<Vec<TeamCard>>,
    /// Card whose delete icon was clicked and awaits confirmation.
    confirm: Option<String>,
    /// `(team dir, deleted)` the user asked for; the shell polls `take_delete_request`.
    request: Option<(String, bool)>,
    /// Team dir the user confirmed to delete forever (it was already in the trash).
    purge_request: Option<String>,
    /// Rename dialog open for `(team dir, name being typed)`.
    renaming: Option<(String, String)>,
    /// `(team dir, new display name)` confirmed in the dialog; polled by `take_rename_request`.
    rename_request: Option<(String, String)>,
}

impl Teams {
    /// Forget the cached scan so the next `show` rereads the home.
    pub fn invalidate(&mut self) {
        self.cards = None;
    }

    /// A confirmed rename `(team dir, new display name)`; the shell runs `GuiCmd::RenameTeam`
    /// against that team and then calls `invalidate`.
    pub fn take_rename_request(&mut self) -> Option<(String, String)> {
        self.rename_request.take()
    }

    /// A pending delete (`true`) or restore (`false`) of the team in `.0`; the shell runs
    /// `GuiCmd::DeleteTeam` against that team and then calls `invalidate`.
    pub fn take_delete_request(&mut self) -> Option<(String, bool)> {
        self.request.take()
    }

    /// A confirmed permanent delete of a trashed team; the shell calls
    /// `FileStore::delete_team_permanently` and then `invalidate`.
    pub fn take_purge_request(&mut self) -> Option<String> {
        self.purge_request.take()
    }

    /// A header button the shell must act on (`Org Map`).
    pub fn take_action(&mut self) -> Option<TeamsAction> {
        self.action.take()
    }

    /// A confirmed draft from the `Import` dialog; the shell turns it into commands exactly
    /// like `Dashboard::take_draft`.
    pub fn take_draft(&mut self) -> Option<TeamDraft> {
        self.draft.take()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        home: &Path,
        current: &str,
        out: &mut Intents,
    ) {
        let all = self.cards.get_or_insert_with(|| scan(home)).clone();
        let (deleted, cards): (Vec<TeamCard>, Vec<TeamCard>) =
            all.into_iter().partition(|c| c.deleted);
        self.header(ui, tr, cards.len());

        let q = self.search.trim().to_lowercase();
        let filter = self.filter;
        let matches = |c: &&TeamCard| {
            (q.is_empty()
                || c.dir.to_lowercase().contains(&q)
                || c.name
                    .as_deref()
                    .is_some_and(|n| n.to_lowercase().contains(&q)))
                && match filter {
                    Filter::All => true,
                    Filter::Running => c.running,
                    Filter::Offline => !c.running,
                }
        };
        let shown: Vec<&TeamCard> = cards.iter().filter(matches).collect();
        if shown.is_empty() {
            if cards.is_empty() {
                kit::empty_state(
                    ui,
                    Icon::Users,
                    &tr.t("team.list.empty.title"),
                    &tr.t("team.list.empty.description"),
                );
            } else {
                kit::empty_state(ui, Icon::Search, &tr.t("team.list.noMatches"), "");
            }
        }
        kit::grid(ui, 300.0, &shown, |ui, w, c| {
            self.card(ui, tr, c, current, w, out)
        });
        let gone: Vec<&TeamCard> = deleted.iter().filter(matches).collect();
        if !gone.is_empty() {
            ui.add_space(14.0);
            kit::heading(
                ui,
                Icon::Trash,
                &tr.t("team.list.status.deleted"),
                Some(gone.len()),
            );
            ui.add_space(8.0);
            kit::grid(ui, 300.0, &gone, |ui, w, c| {
                self.card(ui, tr, c, current, w, out)
            });
        }
        self.dialogs(ui.ctx(), tr, home, out);
    }

    fn header(&mut self, ui: &mut egui::Ui, tr: &I18n, count: usize) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(
                RichText::new(tr.t("common.layout.menu.teams"))
                    .size(22.0)
                    .strong(),
            );
            theme::count_badge(ui, count, false);
            ui.add_space(10.0);
            kit::search_box(
                ui,
                &mut self.search,
                &tr.t("team.list.searchPlaceholder"),
                260.0,
            );
            if theme::pill_icon_button(
                ui,
                Some(Icon::Filter),
                &tr.t("team.list.filter.label"),
                self.filter_open || self.filter != Filter::All,
            )
            .clicked()
            {
                self.filter_open = !self.filter_open;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if theme::pill_icon_button(
                    ui,
                    Some(Icon::Network),
                    &tr.t("team.list.actions.organizationMap"),
                    false,
                )
                .clicked()
                {
                    self.action = Some(TeamsAction::OpenOrgMap);
                }
                if theme::pill_icon_button(
                    ui,
                    Some(Icon::UserPlus),
                    &tr.t("team.list.actions.importTeam"),
                    false,
                )
                .clicked()
                {
                    self.import.open();
                }
                let create = egui::Button::new(
                    RichText::new(tr.t("team.list.actions.createTeam"))
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(theme::ACCENT)
                .corner_radius(egui::CornerRadius::same(255))
                .min_size(egui::vec2(0.0, 30.0));
                if ui.add(create).clicked() {
                    self.create_open = true;
                }
            });
        });
        if self.filter_open {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (f, key) in [
                    (Filter::All, "common.states.all"),
                    (Filter::Running, "team.list.status.running"),
                    (Filter::Offline, "team.list.status.offline"),
                ] {
                    let label = if f == Filter::All {
                        "All".to_string()
                    } else {
                        tr.t(key)
                    };
                    if theme::pill_button(ui, &label, self.filter == f).clicked() {
                        self.filter = f;
                    }
                }
            });
        }
        ui.add_space(16.0);
    }

    fn dialogs(&mut self, ctx: &egui::Context, tr: &I18n, home: &Path, out: &mut Intents) {
        if let Some(n) = kit::create_team_dialog(ctx, tr, &mut self.create_open, &mut self.new_name)
        {
            out.push(Intent::SwitchTeam(n));
            self.cards = None;
        }
        if let Some(d) = self.import.show(ctx, tr, home, &[]) {
            self.draft = Some(d);
        }
        self.rename_dialog(ctx, tr);
    }

    fn rename_dialog(&mut self, ctx: &egui::Context, tr: &I18n) {
        let Some((dir, text)) = &mut self.renaming else {
            return;
        };
        let mut done = None;
        egui::Window::new(tr.t("rteams.teams.rename"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let edit = ui.add(egui::TextEdit::singleline(text).desired_width(280.0));
                edit.request_focus();
                let ok = !text.trim().is_empty();
                let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if theme::pill_button(ui, &tr.t("common.actions.cancel"), false).clicked() {
                        done = Some(None);
                    }
                    let save = ui.add_enabled(ok, egui::Button::new(tr.t("common.actions.save")));
                    if (save.clicked() || (enter && ok)) && ok {
                        done = Some(Some((dir.clone(), text.trim().to_string())));
                    }
                });
            });
        if let Some(d) = done {
            self.renaming = None;
            self.rename_request = d;
        }
    }

    fn card(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        c: &TeamCard,
        current: &str,
        w: f32,
        out: &mut Intents,
    ) {
        let active = c.dir == current && !c.deleted;
        let title_color = if c.deleted { theme::MUTED } else { theme::TEXT };
        let now = recent_projects::now_ms();
        let resp = kit::tile(ui, ("team", &c.dir), w, 176.0, active, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (dot, label) = if c.running {
                    (theme::SUCCESS, tr.t("team.list.status.running"))
                } else {
                    (theme::MUTED, tr.t("team.list.status.offline"))
                };
                let (r, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, dot);
                ui.add(
                    egui::Label::new(
                        RichText::new(c.name.as_deref().unwrap_or(&c.dir))
                            .size(16.0)
                            .strong()
                            .color(title_color),
                    )
                    .truncate(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if c.name.is_some() {
                        self.card_actions(ui, tr, c);
                    }
                    if !c.deleted {
                        theme::pill(ui, label, dot);
                    }
                });
            });
            let sub = match (&c.name, &c.project) {
                (None, _) => tr.t("rteams.teams.notCreated"),
                (_, Some(p)) => p.clone(),
                _ => tr.t("team.list.noDescription"),
            };
            let sub_color = if c.name.is_none() {
                theme::WARNING
            } else {
                theme::MUTED
            };
            ui.add(egui::Label::new(RichText::new(sub).color(sub_color)).truncate());
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = -7.0;
                for (name, _) in c.roster.iter().take(5) {
                    let col = theme::member_color(None, name);
                    theme::avatar(ui, name, col, 28.0, None);
                }
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.add_space(14.0);
                let extra = c.roster.len().saturating_sub(5);
                let text = if extra > 0 {
                    format!("+{extra}")
                } else {
                    tr.t_count("team.list.membersCount", c.members as u64)
                };
                ui.label(RichText::new(text).color(theme::TEXT_2));
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                theme::pill(ui, format!("{} open", c.open_tasks), theme::INFO);
                theme::pill(ui, format!("{} done", c.done_tasks), theme::COL_DONE);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if c.last_activity_ms > 0 {
                        ui.label(
                            RichText::new(recent_projects::ago(now, c.last_activity_ms))
                                .size(12.5)
                                .color(theme::MUTED),
                        );
                    }
                });
            });
        });
        if resp.clicked() && !c.deleted {
            out.push(Intent::SwitchTeam(c.dir.clone()));
        }
    }

    fn card_actions(&mut self, ui: &mut egui::Ui, tr: &I18n, c: &TeamCard) {
        if c.deleted && self.confirm.as_deref() == Some(c.dir.as_str()) {
            if theme::pill_button(ui, &tr.t("common.actions.cancel"), false).clicked() {
                self.confirm = None;
            }
            let label = if tr.has("rteams.teams.deleteForever") {
                tr.t("rteams.teams.deleteForever")
            } else {
                "Delete forever".to_string()
            };
            let b = egui::Button::new(RichText::new(label).color(theme::ERROR))
                .corner_radius(egui::CornerRadius::same(255));
            if ui.add(b).clicked() {
                self.purge_request = Some(c.dir.clone());
                self.confirm = None;
            }
        } else if c.deleted {
            if theme::pill_button(ui, &tr.t("team.kanban.trash.restore"), false).clicked() {
                self.request = Some((c.dir.clone(), false));
            }
            if theme::icon_button(ui, Icon::Trash, &tr.t("common.notifications.row.delete"))
                .clicked()
            {
                self.confirm = Some(c.dir.clone());
            }
        } else if self.confirm.as_deref() == Some(c.dir.as_str()) {
            if theme::pill_button(ui, &tr.t("common.actions.cancel"), false).clicked() {
                self.confirm = None;
            }
            let b = egui::Button::new(
                RichText::new(tr.t("team.kanban.taskCard.confirm")).color(theme::ERROR),
            )
            .corner_radius(egui::CornerRadius::same(255));
            if ui.add(b).clicked() {
                self.request = Some((c.dir.clone(), true));
                self.confirm = None;
            }
        } else {
            if theme::icon_button(ui, Icon::Trash, &tr.t("common.notifications.row.delete"))
                .clicked()
            {
                self.confirm = Some(c.dir.clone());
            }
            if theme::icon_button(ui, Icon::Edit, &tr.t("rteams.teams.rename")).clicked() {
                self.renaming = Some((c.dir.clone(), c.name.clone().unwrap_or_default()));
            }
        }
    }
}

/// Same rule as the store: 1-64 chars of `[A-Za-z0-9._-]`, not starting with '.'.
pub fn valid_name(s: &str) -> bool {
    let s = s.trim();
    !s.is_empty()
        && s.len() <= 64
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_store_rule() {
        assert!(valid_name("my-team_2.x"));
        assert!(!valid_name(".hidden"));
        assert!(!valid_name("has space"));
        assert!(!valid_name(""));
    }

    #[test]
    fn deleted_teams_are_split_out_and_render() {
        let (home, _d) = crate::view::testing::fixture("teamsdel");
        let mut c = crate::actions::Commander::new(home.clone(), "t".into());
        c.run(
            "boss",
            &crate::actions::GuiCmd::DeleteTeam { deleted: true },
        )
        .unwrap();
        let cards = scan(&home);
        assert!(cards[0].deleted);
        let mut teams = Teams {
            confirm: Some("t".into()),
            ..Default::default()
        };
        let mut out = Intents::default();
        crate::view::testing::render(|ui| {
            teams.show(ui, crate::view::testing::tr(), &home, "t", &mut out)
        });
        assert!(out.0.is_empty());
        teams.request = Some(("t".into(), false));
        assert_eq!(teams.take_delete_request(), Some(("t".into(), false)));
        assert_eq!(teams.take_delete_request(), None);
        // Deleting forever is a separate, confirmed request for a trashed team.
        teams.purge_request = Some("t".into());
        assert_eq!(teams.take_purge_request().as_deref(), Some("t"));
        assert_eq!(teams.take_purge_request(), None);
        rteams_store::FileStore::delete_team_permanently(&home, "t").unwrap();
        assert!(scan(&home).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn rename_keeps_dir_and_shows_new_name() {
        let (home, _d) = crate::view::testing::fixture("teamsren");
        let mut c = crate::actions::Commander::new(home.clone(), "t".into());
        c.run(
            "boss",
            &crate::actions::GuiCmd::RenameTeam {
                name: "Fresh Name".into(),
            },
        )
        .unwrap();
        let cards = scan(&home);
        assert_eq!(cards[0].dir, "t");
        assert_eq!(cards[0].name.as_deref(), Some("Fresh Name"));
        let mut teams = Teams {
            renaming: Some(("t".into(), "Fresh Name".into())),
            ..Default::default()
        };
        let mut out = Intents::default();
        crate::view::testing::render(|ui| {
            teams.show(ui, crate::view::testing::tr(), &home, "t", &mut out)
        });
        assert_eq!(teams.take_rename_request(), None);
        teams.rename_request = Some(("t".into(), "X".into()));
        assert_eq!(teams.take_rename_request(), Some(("t".into(), "X".into())));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn scan_finds_team_dirs_and_renders() {
        let (home, _d) = crate::view::testing::fixture("teams");
        std::fs::create_dir_all(home.join("not-a-team")).unwrap();
        let cards = scan(&home);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].dir, "t");
        assert_eq!(cards[0].members, 3);
        assert!(!cards[0].deleted);
        let mut teams = Teams::default();
        let mut out = Intents::default();
        crate::view::testing::render(|ui| {
            teams.show(ui, crate::view::testing::tr(), &home, "t", &mut out)
        });
        assert_eq!(teams.take_delete_request(), None);
        let _ = std::fs::remove_dir_all(&home);
    }
}
