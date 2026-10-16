//! Recent projects grid: search, cards with last activity / task counts / badges, Load more.

use std::collections::HashMap;

use eframe::egui::{self, RichText};
use rteams_services::projects::RecentProject;

use crate::icons::{self, Icon};
use crate::theme;
use crate::ui::dashboard::kit;
use crate::ui::dashboard::running_teams::TaskCounts;
use crate::ui::{FeatureCtx, ShellRequest};

pub const PAGE: usize = 12;

/// What the section asks the dashboard to do besides pushing `ShellRequest`s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// A project without a team was clicked: create a team there (path prefilled).
    CreateTeamAt(String),
    /// The user opened this path in the editor or terminal (bump its history).
    Opened(String),
    SelectFolder,
}

#[derive(Debug, Default)]
pub struct RecentSection {
    pub search: String,
    pub limit: usize,
}

/// Projects whose label or path contains `query`.
pub fn filter<'a>(items: &'a [RecentProject], query: &str) -> Vec<&'a RecentProject> {
    crate::ui::dashboard::project_picker::matching(items, query)
}

/// `5m ago` style text for a unix-ms timestamp.
pub fn ago(now_ms: i64, then_ms: i64) -> String {
    let s = ((now_ms - then_ms).max(0)) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl RecentSection {
    pub fn new() -> Self {
        RecentSection {
            search: String::new(),
            limit: PAGE,
        }
    }

    /// Draw the section. `counts` maps a team dir to its task counts for the card.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        c: &mut FeatureCtx,
        items: &[RecentProject],
        counts: &HashMap<String, TaskCounts>,
        loading: bool,
    ) -> Vec<Action> {
        let mut actions = Vec::new();
        if self.limit == 0 {
            self.limit = PAGE;
        }
        let searching = !self.search.trim().is_empty();
        ui.horizontal(|ui| {
            let title = if searching {
                "dashboard.recentProjects.searchResults"
            } else {
                "dashboard.recentProjects.title"
            };
            kit::heading(
                ui,
                Icon::Folder,
                &c.tr.t(title),
                (!items.is_empty()).then_some(items.len()),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                kit::search_box(
                    ui,
                    &mut self.search,
                    &c.tr.t("dashboard.recentProjects.searchPlaceholder"),
                    260.0,
                );
                if searching && theme::ghost(ui, &c.tr.t("dashboard.actions.clearSearch")).clicked()
                {
                    self.search.clear();
                }
            });
        });
        ui.add_space(8.0);

        let shown = filter(items, &self.search);
        if shown.is_empty() {
            if loading {
                kit::skeleton_tiles(ui, 3, 300.0, 150.0, &c.tr.t("common.states.loading"));
            } else if searching {
                kit::empty_state(
                    ui,
                    Icon::Search,
                    &c.tr.t_args(
                        "dashboard.recentProjects.noMatches",
                        &[("query", self.search.trim())],
                    ),
                    "",
                );
            } else {
                kit::empty_state(
                    ui,
                    Icon::Folder,
                    &c.tr.t("dashboard.recentProjects.noRecentProjects"),
                    &c.tr.t("dashboard.recentProjects.emptyDescription"),
                );
            }
            return actions;
        }
        let total = shown.len();
        let now = now_ms();
        let page: Vec<&RecentProject> = shown.into_iter().take(self.limit).collect();
        kit::grid(ui, 300.0, &page, |ui, w, p| {
            card(ui, c, p, counts, w, now, &mut actions)
        });
        if total > self.limit {
            ui.add_space(4.0);
            if theme::pill_button(ui, &c.tr.t("dashboard.recentProjects.loadMore"), false).clicked()
            {
                self.limit += PAGE;
            }
        }
        actions
    }
}

fn card(
    ui: &mut egui::Ui,
    c: &mut FeatureCtx,
    p: &RecentProject,
    counts: &HashMap<String, TaskCounts>,
    w: f32,
    now: i64,
    actions: &mut Vec<Action>,
) {
    let resp = kit::tile(ui, ("recent", &p.path), w, 172.0, false, |ui| {
        let title = if p.exists { theme::TEXT } else { theme::MUTED };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            icons::icon(ui, Icon::Folder, 16.0, theme::ACCENT);
            ui.add(
                egui::Label::new(RichText::new(&p.label).size(15.0).strong().color(title))
                    .truncate(),
            );
            if !p.exists {
                theme::pill(
                    ui,
                    c.tr.t("dashboard.recentProjects.card.deleted"),
                    theme::ERROR,
                )
                .on_hover_text(c.tr.t("dashboard.recentProjects.card.projectFolderMissing"));
            }
        });
        ui.add(egui::Label::new(RichText::new(&p.path).size(12.5).color(theme::MUTED)).truncate());
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(
                RichText::new(ago(now, p.last_activity_ms))
                    .size(13.0)
                    .color(theme::TEXT_2),
            );
            for prov in &p.providers {
                if prov != "opened" {
                    theme::pill(ui, prov, theme::ACCENT);
                }
            }
        });
        if let Some(n) = p.team.as_ref().and_then(|t| counts.get(t)) {
            let t = |k: &str, n: usize| c.tr.t_count(k, n as u64);
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "{} · {} · {}",
                        t(
                            "dashboard.recentProjects.card.taskCounts.active",
                            n.in_progress
                        ),
                        t(
                            "dashboard.recentProjects.card.taskCounts.pending",
                            n.pending
                        ),
                        t("dashboard.recentProjects.card.taskCounts.done", n.completed),
                    ))
                    .size(13.0)
                    .color(theme::TEXT_2),
                )
                .truncate(),
            );
        }
        if p.exists {
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if theme::pill_icon_button(
                        ui,
                        Some(Icon::Code),
                        &c.tr.t("settings.advanced.configuration.openInEditor"),
                        false,
                    )
                    .clicked()
                    {
                        c.req.push(ShellRequest::OpenEditor {
                            root: p.path.clone().into(),
                            file: None,
                        });
                        actions.push(Action::Opened(p.path.clone()));
                    }
                    if theme::pill_icon_button(
                        ui,
                        Some(Icon::Terminal),
                        &c.tr.t("common.terminal.title"),
                        false,
                    )
                    .clicked()
                    {
                        c.req.push(ShellRequest::OpenTerminal {
                            cwd: p.path.clone().into(),
                        });
                        actions.push(Action::Opened(p.path.clone()));
                    }
                });
            });
        }
    });
    if resp.clicked() && p.exists {
        match &p.team {
            Some(t) => c.req.push(ShellRequest::OpenTeam(t.clone())),
            None => actions.push(Action::CreateTeamAt(p.path.clone())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ago_text() {
        assert_eq!(ago(100_000, 99_000), "just now");
        assert_eq!(ago(10 * 60_000, 0), "10m ago");
        assert_eq!(ago(3 * 3_600_000, 0), "3h ago");
        assert_eq!(ago(5 * 86_400_000, 0), "5d ago");
    }
}
