//! Left column of the Organizations panel: the org tree (orgs > groups > teams), the unplaced
//! teams, selection and drag-and-drop of teams onto orgs / groups.

use eframe::egui::{self, Color32, RichText};
use rteams_services::i18n::I18n;
use rteams_services::orgs::projection::{GroupNode, OrgNode, OrgView, TeamSummary};

use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Selection {
    #[default]
    None,
    Org(String),
    Group(String, String),
    Team(String),
    /// The "Unassigned Teams" bucket.
    Unplaced,
}

/// Drag payload: a team directory name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TeamDrag(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    Select(Selection),
    /// A team was dropped on an org (`group == None`) or one of its groups.
    DropTeam {
        team: String,
        org: String,
        group: Option<String>,
    },
}

pub fn parse_color(hex: &str) -> Color32 {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8);
        }
    }
    theme::ACCENT
}

pub fn color_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn drop_target(ui: &egui::Ui, r: &egui::Response) -> Option<String> {
    if egui::DragAndDrop::has_payload_of_type::<TeamDrag>(ui.ctx())
        && r.dnd_hover_payload::<TeamDrag>().is_some()
    {
        ui.painter()
            .rect_stroke(r.rect, 3.0, (1.0, theme::ACCENT), egui::StrokeKind::Inside);
    }
    r.dnd_release_payload::<TeamDrag>().map(|p| p.0.clone())
}

fn team_row(ui: &mut egui::Ui, t: &TeamSummary, sel: &Selection, acts: &mut Vec<ListAction>) {
    let dot = if t.running {
        theme::SUCCESS
    } else {
        theme::MUTED
    };
    let selected = *sel == Selection::Team(t.dir.clone());
    let r = ui
        .horizontal(|ui| {
            ui.label(RichText::new("•").color(dot));
            ui.selectable_label(selected, &t.name)
        })
        .inner;
    let r = r.interact(egui::Sense::click_and_drag());
    r.dnd_set_drag_payload(TeamDrag(t.dir.clone()));
    if r.clicked() {
        acts.push(ListAction::Select(Selection::Team(t.dir.clone())));
    }
}

fn group_node(
    ui: &mut egui::Ui,
    org: &str,
    g: &GroupNode,
    sel: &Selection,
    acts: &mut Vec<ListAction>,
) {
    let me = Selection::Group(org.into(), g.group.id.clone());
    let head = egui::CollapsingHeader::new(
        RichText::new(&g.group.name).color(parse_color(&g.group.color)),
    )
    .id_salt(("orgs-group", org, &g.group.id))
    .default_open(true)
    .show(ui, |ui| {
        for t in &g.teams {
            team_row(ui, t, sel, acts);
        }
    });
    if head.header_response.clicked() {
        acts.push(ListAction::Select(me));
    }
    if let Some(team) = drop_target(ui, &head.header_response) {
        acts.push(ListAction::DropTeam {
            team,
            org: org.into(),
            group: Some(g.group.id.clone()),
        });
    }
}

fn org_node(ui: &mut egui::Ui, n: &OrgNode, sel: &Selection, acts: &mut Vec<ListAction>) {
    let me = Selection::Org(n.org.id.clone());
    let title = RichText::new(&n.org.name)
        .strong()
        .color(parse_color(&n.org.color));
    let head = egui::CollapsingHeader::new(title)
        .id_salt(("orgs-org", &n.org.id))
        .default_open(true)
        .show(ui, |ui| {
            for g in &n.groups {
                group_node(ui, &n.org.id, g, sel, acts);
            }
            for t in &n.teams {
                team_row(ui, t, sel, acts);
            }
            for c in &n.children {
                org_node(ui, c, sel, acts);
            }
        });
    if head.header_response.clicked() {
        acts.push(ListAction::Select(me));
    }
    if let Some(team) = drop_target(ui, &head.header_response) {
        acts.push(ListAction::DropTeam {
            team,
            org: n.org.id.clone(),
            group: None,
        });
    }
}

pub fn show(ui: &mut egui::Ui, tr: &I18n, view: &OrgView, sel: &Selection) -> Vec<ListAction> {
    let mut acts = Vec::new();
    ui.strong(tr.t("team.organizations.editor.organizationTree"));
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .id_salt("orgs-list")
        .show(ui, |ui| {
            if view.roots.is_empty() {
                ui.label(
                    RichText::new(tr.t("team.organizations.editor.noOrganizationUnits"))
                        .color(theme::MUTED),
                );
            }
            for n in &view.roots {
                org_node(ui, n, sel, &mut acts);
            }
            ui.add_space(8.0);
            let head = egui::CollapsingHeader::new(
                RichText::new(tr.t("team.organizations.graph.canvas.unassignedTeams"))
                    .color(theme::TEXT_2),
            )
            .id_salt("orgs-unplaced")
            .default_open(true)
            .show(ui, |ui| {
                for t in &view.unplaced {
                    team_row(ui, t, sel, &mut acts);
                }
            });
            if head.header_response.clicked() {
                acts.push(ListAction::Select(Selection::Unplaced));
            }
        });
    acts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_round_trip() {
        assert_eq!(parse_color("#112233"), Color32::from_rgb(0x11, 0x22, 0x33));
        assert_eq!(parse_color("nope"), theme::ACCENT);
        assert_eq!(color_hex([0x11, 0x22, 0x33]), "#112233");
    }
}
