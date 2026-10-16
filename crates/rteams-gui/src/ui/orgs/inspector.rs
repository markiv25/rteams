//! Right column: facts about the selected org, group or team.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::orgs::model::OrgMap;
use rteams_services::orgs::projection::{OrgNode, OrgView, TeamSummary, Totals};

use super::list::{parse_color, Selection};
use crate::theme;
use crate::ui::ShellRequest;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectorAction {
    /// Open the create-team dialog placing the new team in this org / group.
    CreateTeamIn {
        org: String,
        group: Option<String>,
    },
    Select(Selection),
}

fn find<'a>(nodes: &'a [OrgNode], id: &str) -> Option<&'a OrgNode> {
    nodes.iter().find_map(|n| {
        if n.org.id == id {
            Some(n)
        } else {
            find(&n.children, id)
        }
    })
}

pub fn find_team<'a>(view: &'a OrgView, dir: &str) -> Option<&'a TeamSummary> {
    fn walk<'a>(ns: &'a [OrgNode], dir: &str) -> Option<&'a TeamSummary> {
        ns.iter().find_map(|n| {
            n.teams
                .iter()
                .chain(n.groups.iter().flat_map(|g| g.teams.iter()))
                .find(|t| t.dir == dir)
                .or_else(|| walk(&n.children, dir))
        })
    }
    walk(&view.roots, dir).or_else(|| view.unplaced.iter().find(|t| t.dir == dir))
}

fn counts(ui: &mut egui::Ui, tr: &I18n, t: &Totals) {
    ui.horizontal_wrapped(|ui| {
        theme::pill(
            ui,
            tr.t_count("team.organizations.inspector.agents", t.members as u64),
            theme::TEXT_2,
        );
        theme::pill(
            ui,
            format!(
                "{} {}",
                t.open_tasks,
                tr.t("team.organizations.inspector.taskCounts.pending")
            ),
            theme::COL_IN_PROGRESS,
        );
        theme::pill(
            ui,
            format!(
                "{} {}",
                t.done_tasks,
                tr.t("team.organizations.inspector.taskCounts.done")
            ),
            theme::COL_DONE,
        );
        theme::pill(
            ui,
            format!(
                "{}/{} {}",
                t.running,
                t.teams,
                tr.t("team.organizations.inspector.status.online")
            ),
            theme::SUCCESS,
        );
    });
}

pub fn show(
    ui: &mut egui::Ui,
    tr: &I18n,
    map: &OrgMap,
    view: &OrgView,
    sel: &Selection,
    req: &mut Vec<ShellRequest>,
) -> Option<InspectorAction> {
    let mut act = None;
    egui::ScrollArea::vertical()
        .id_salt("orgs-inspector")
        .show(ui, |ui| match sel {
            Selection::None => {
                ui.label(
                    RichText::new(
                        tr.t("team.organizations.inspector.descriptions.allOrganizations"),
                    )
                    .color(theme::MUTED),
                );
            }
            Selection::Unplaced => {
                ui.heading(tr.t("team.organizations.graph.canvas.unassignedTeams"));
                ui.label(
                    RichText::new(
                        tr.t("team.organizations.inspector.descriptions.unassignedTeams"),
                    )
                    .color(theme::MUTED),
                );
                ui.label(tr.t_count(
                    "team.organizations.graph.canvas.teams",
                    view.unplaced.len() as u64,
                ));
            }
            Selection::Org(id) => {
                let Some(n) = find(&view.roots, id) else {
                    ui.label(tr.t("team.organizations.graph.canvas.notFound"));
                    return;
                };
                ui.heading(RichText::new(&n.org.name).color(parse_color(&n.org.color)));
                ui.label(
                    RichText::new(tr.t("team.organizations.inspector.kind.organization"))
                        .color(theme::MUTED),
                );
                if !n.org.description.is_empty() {
                    ui.label(&n.org.description);
                }
                counts(ui, tr, &n.totals);
                ui.label(tr.t_count(
                    "team.organizations.inspector.childNodes",
                    (n.children.len() + n.groups.len()) as u64,
                ));
                let label = n.org.name.clone();
                if ui
                    .button(tr.t_args(
                        "team.organizations.graph.actions.createTeamIn",
                        &[("label", &label)],
                    ))
                    .clicked()
                {
                    act = Some(InspectorAction::CreateTeamIn {
                        org: id.clone(),
                        group: None,
                    });
                }
            }
            Selection::Group(org, gid) => {
                let Some(g) = find(&view.roots, org)
                    .and_then(|n| n.groups.iter().find(|g| g.group.id == *gid))
                else {
                    ui.label(tr.t("team.organizations.graph.canvas.notFound"));
                    return;
                };
                ui.heading(RichText::new(&g.group.name).color(parse_color(&g.group.color)));
                ui.label(
                    RichText::new(tr.t("team.organizations.inspector.kind.group"))
                        .color(theme::MUTED),
                );
                if !g.group.description.is_empty() {
                    ui.label(&g.group.description);
                }
                counts(ui, tr, &g.totals);
                let label = g.group.name.clone();
                if ui
                    .button(tr.t_args(
                        "team.organizations.graph.actions.createTeamIn",
                        &[("label", &label)],
                    ))
                    .clicked()
                {
                    act = Some(InspectorAction::CreateTeamIn {
                        org: org.clone(),
                        group: Some(gid.clone()),
                    });
                }
            }
            Selection::Team(dir) => {
                let Some(t) = find_team(view, dir) else {
                    ui.label(tr.t("team.organizations.graph.canvas.notFound"));
                    return;
                };
                ui.heading(&t.name);
                ui.horizontal(|ui| {
                    let (key, c) = if t.running {
                        ("team.organizations.inspector.status.online", theme::SUCCESS)
                    } else {
                        ("team.organizations.inspector.status.offline", theme::MUTED)
                    };
                    theme::pill(ui, tr.t(key), c);
                    match map.placement(dir) {
                        Some((o, _)) => {
                            let name = map.org(o).map_or(o, |o| o.name.as_str());
                            theme::pill(ui, name, theme::ACCENT);
                        }
                        None => {
                            theme::pill(
                                ui,
                                tr.t("team.organizations.inspector.tags.unassigned"),
                                theme::WARNING,
                            );
                        }
                    }
                });
                counts(
                    ui,
                    tr,
                    &Totals {
                        teams: 1,
                        members: t.members,
                        open_tasks: t.open_tasks,
                        done_tasks: t.done_tasks,
                        running: usize::from(t.running),
                    },
                );
                if t.in_progress == 0 {
                    ui.label(
                        RichText::new(tr.t("team.organizations.inspector.noActiveAgentTasks"))
                            .color(theme::MUTED),
                    );
                } else {
                    ui.label(format!(
                        "{}: {}",
                        tr.t("team.organizations.inspector.inProgress"),
                        t.in_progress
                    ));
                }
                if !t.leads.is_empty() {
                    ui.label(RichText::new(t.leads.join(", ")).color(theme::TEXT_2));
                }
                if !t.running_members.is_empty() {
                    ui.label(RichText::new(t.running_members.join(", ")).color(theme::SUCCESS));
                }
                let chain = map.reports_chain(dir);
                if chain.len() > 1 {
                    ui.label(format!("{}: {}", "Path to root", chain[1..].join(" > ")));
                }
                for r in view
                    .relations
                    .iter()
                    .filter(|r| r.relation.source == *dir || r.relation.target == *dir)
                {
                    let other = if r.relation.source == *dir {
                        &r.relation.target
                    } else {
                        &r.relation.source
                    };
                    if ui
                        .link(format!(
                            "{}  {}  ({})",
                            r.relation.kind.label(),
                            other,
                            r.weight
                        ))
                        .clicked()
                    {
                        act = Some(InspectorAction::Select(Selection::Team(other.clone())));
                    }
                }
                if ui
                    .button(tr.t("team.organizations.inspector.actions.open"))
                    .clicked()
                {
                    req.push(ShellRequest::OpenTeam(dir.clone()));
                }
            }
        });
    act
}
