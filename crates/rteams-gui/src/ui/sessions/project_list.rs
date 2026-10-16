//! Left pane: repositories (worktrees grouped) and their projects.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::paths::Source;
use rteams_services::sessions::worktrees::RepoGroup;

use super::{fmt_ago, tt};
use crate::theme;

/// Filter text for the project list.
#[derive(Default)]
pub struct ProjectListState {
    pub filter: String,
}

fn source_tag(s: Source) -> (&'static str, egui::Color32) {
    match s {
        Source::Claude => ("C", theme::ACCENT),
        Source::Codex => ("X", theme::SUCCESS),
        Source::Rteams => ("R", theme::WARNING),
    }
}

/// Draw the pane; `selected` is the chosen project id (`None` = all projects).
pub fn show(
    ui: &mut egui::Ui,
    tr: &I18n,
    st: &mut ProjectListState,
    groups: &[RepoGroup],
    total_sessions: usize,
    selected: &mut Option<String>,
    now_ms: i64,
) {
    ui.add(
        egui::TextEdit::singleline(&mut st.filter)
            .hint_text(tt(
                tr,
                "recentProjects.searchPlaceholder",
                "Search projects...",
            ))
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);
    let all = selected.is_none();
    ui.horizontal(|ui| {
        if ui
            .selectable_label(all, tt(tr, "sessions.allProjects", "All projects"))
            .clicked()
        {
            *selected = None;
        }
        theme::count_badge(ui, total_sessions, false);
    });
    ui.add_space(4.0);
    let q = st.filter.trim().to_lowercase();
    egui::ScrollArea::vertical()
        .id_salt("sessions_projects")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for g in groups {
                let projects: Vec<_> = g
                    .projects
                    .iter()
                    .filter(|p| {
                        q.is_empty()
                            || p.cwd.to_lowercase().contains(&q)
                            || g.label.to_lowercase().contains(&q)
                    })
                    .collect();
                if projects.is_empty() {
                    continue;
                }
                let single = projects.len() == 1 && !projects[0].is_worktree;
                if !single {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(&g.label).strong().color(theme::TEXT_2));
                        theme::count_badge(ui, g.session_count, false);
                    });
                }
                for p in projects {
                    let on = selected.as_deref() == Some(p.id.as_str());
                    let title = if single {
                        g.label.clone()
                    } else if p.is_worktree {
                        format!("  {} {}", '\u{2022}', p.label)
                    } else {
                        format!("  {}", p.label)
                    };
                    let resp = ui
                        .horizontal(|ui| {
                            let r = ui.selectable_label(on, title).on_hover_text(&p.cwd);
                            for s in &p.sources {
                                let (t, c) = source_tag(*s);
                                theme::pill(ui, t, c);
                            }
                            ui.label(
                                RichText::new(format!("{}", p.session_count))
                                    .small()
                                    .color(theme::MUTED),
                            );
                            ui.label(
                                RichText::new(fmt_ago(now_ms, p.last_ms))
                                    .small()
                                    .color(theme::MUTED),
                            );
                            r
                        })
                        .inner;
                    if resp.clicked() {
                        *selected = Some(p.id.clone());
                    }
                }
                ui.add_space(4.0);
            }
        });
}
