//! Organizations panel set: org list, editor panel, inspector, create-team dialog. The shell
//! mounts `OrgsPanel` inside the Teams tab; the map canvas files are added to this module
//! separately.

pub mod canvas_hierarchy;
pub mod canvas_nested;
pub mod create_team_dialog;
pub mod editor_panel;
pub mod focus;
pub mod hud_legend;
pub mod inspector;
pub mod layout;
pub mod list;
pub mod map_tab;
pub mod toolbar;

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use rteams_services::orgs::model::OrgMap;
use rteams_services::orgs::projection::OrgView;
use rteams_services::orgs::structure::Edit;

use self::create_team_dialog::{CreateTeamDialog, DialogResult};
use self::editor_panel::EditorPanel;
use self::inspector::InspectorAction;
use self::list::{ListAction, Selection};
use super::{FeatureCtx, ShellRequest};
use crate::theme;

/// Re-read teams (run files, logs) at most this often.
const REFRESH: Duration = Duration::from_secs(5);

#[derive(Default)]
pub struct OrgsPanel {
    map: OrgMap,
    view: OrgView,
    loaded: Option<Instant>,
    pub selection: Selection,
    editor: EditorPanel,
    dialog: Option<CreateTeamDialog>,
    error: Option<String>,
}

impl OrgsPanel {
    pub fn new() -> Self {
        Self::default()
    }

    fn reload(&mut self, home: &std::path::Path) {
        self.map = OrgMap::load(home);
        self.view = OrgView::load(home, &self.map);
        self.loaded = Some(Instant::now());
    }

    /// Apply edits, save, and refresh. The first error stops the batch and is shown.
    pub fn apply(&mut self, home: &std::path::Path, edits: Vec<Edit>) {
        for e in edits {
            match self.map.apply(e.clone()) {
                Ok(id) => {
                    if let (Edit::CreateOrg { .. }, Some(id)) = (&e, id) {
                        self.selection = Selection::Org(id);
                    }
                    if matches!(e, Edit::DeleteOrg { .. }) {
                        self.selection = Selection::None;
                    }
                    self.error = None;
                }
                Err(err) => {
                    self.error = Some(err.to_string());
                    break;
                }
            }
        }
        if let Err(e) = self.map.save(home) {
            self.error = Some(format!("Could not save organizations: {e}"));
        }
        self.reload(home);
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        if self.loaded.is_none_or(|t| t.elapsed() > REFRESH) {
            self.reload(c.home);
        }
        ui.ctx().request_repaint_after(REFRESH);
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::ERROR));
        }
        let mut edits = Vec::new();
        let mut open_dialog = None;
        let avail = ui.available_width();
        let h = ui.available_height().max(200.0);
        let (lw, rw) = if avail < 600.0 {
            (avail * 0.34, avail * 0.30)
        } else {
            (
                (avail * 0.26).clamp(180.0, 320.0),
                (avail * 0.26).clamp(180.0, 320.0),
            )
        };
        let mw = (avail - lw - rw - 24.0).max(60.0);
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(lw, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_height(h);
                    for a in list::show(ui, c.tr, &self.view, &self.selection) {
                        match a {
                            ListAction::Select(s) => self.selection = s,
                            ListAction::DropTeam { team, org, group } => {
                                edits.push(Edit::PlaceTeam { team, org, group })
                            }
                        }
                    }
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(mw, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    edits.extend(self.editor.show(
                        ui,
                        c.tr,
                        &self.map,
                        &self.view,
                        &self.selection,
                    ));
                },
            );
            ui.separator();
            ui.allocate_ui_with_layout(
                egui::vec2(rw, h),
                egui::Layout::top_down(egui::Align::Min),
                |ui| match inspector::show(ui, c.tr, &self.map, &self.view, &self.selection, c.req)
                {
                    Some(InspectorAction::CreateTeamIn { org, group }) => {
                        open_dialog = Some((org, group))
                    }
                    Some(InspectorAction::Select(s)) => self.selection = s,
                    None => {}
                },
            );
        });
        if let Some((org, group)) = open_dialog {
            let label = match &group {
                Some(g) => self
                    .map
                    .org(&org)
                    .and_then(|o| o.groups.iter().find(|x| x.id == *g))
                    .map_or(g.clone(), |g| g.name.clone()),
                None => self.map.org(&org).map_or(org.clone(), |o| o.name.clone()),
            };
            self.dialog = Some(CreateTeamDialog::new(org, group, label));
        }
        if let Some(d) = &mut self.dialog {
            match d.show(ui.ctx(), c.home, c.tr) {
                DialogResult::Open => {}
                DialogResult::Cancelled => self.dialog = None,
                DialogResult::Created(team) => {
                    let (org, group) = (d.org.clone(), d.group.clone());
                    self.dialog = None;
                    edits.push(Edit::PlaceTeam {
                        team: team.clone(),
                        org,
                        group,
                    });
                    c.req.push(ShellRequest::OpenTeam(team));
                }
            }
        }
        if !edits.is_empty() {
            self.apply(c.home, edits);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};
    use crate::view::Intents;
    use rteams_services::i18n::I18n;

    #[test]
    fn headless_render_and_edit_flow() {
        let (home, _d) = fixture("orgs");
        let mut panel = OrgsPanel::new();
        panel.apply(
            &home,
            vec![
                Edit::CreateOrg {
                    name: "Acme".into(),
                    parent: None,
                },
                Edit::CreateGroup {
                    org: "acme".into(),
                    name: "Squad".into(),
                },
                Edit::PlaceTeam {
                    team: "t".into(),
                    org: "acme".into(),
                    group: Some("squad".into()),
                },
            ],
        );
        assert!(panel.error.is_none(), "{:?}", panel.error);
        assert_eq!(
            OrgMap::load(&home).placement("t"),
            Some(("acme", Some("squad")))
        );
        let tr = I18n::english();
        let mut out = Intents::default();
        let mut req = Vec::new();
        for sel in [
            Selection::None,
            Selection::Org("acme".into()),
            Selection::Group("acme".into(), "squad".into()),
            Selection::Team("t".into()),
            Selection::Unplaced,
        ] {
            panel.selection = sel;
            for tab in [
                editor_panel::Tab::Structure,
                editor_panel::Tab::Teams,
                editor_panel::Tab::Links,
                editor_panel::Tab::Orgs,
            ] {
                panel.editor.tab = tab;
                render(|ui| {
                    let mut c = FeatureCtx {
                        home: &home,
                        team: None,
                        view: None,
                        out: &mut out,
                        tr: &tr,
                        req: &mut req,
                    };
                    panel.show(ui, &mut c);
                });
            }
        }
        panel.dialog = Some(CreateTeamDialog::new("acme".into(), None, "Acme".into()));
        render(|ui| {
            let mut c = FeatureCtx {
                home: &home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            panel.show(ui, &mut c);
        });
        let t = &panel.view.roots[0].groups[0].teams[0];
        assert_eq!((t.dir.as_str(), t.members), ("t", 3));
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn cycle_errors_are_surfaced() {
        let home = std::env::temp_dir().join(format!("rteams-gui-orgcyc-{}", std::process::id()));
        let mut p = OrgsPanel::new();
        p.apply(
            &home,
            vec![
                Edit::CreateOrg {
                    name: "a".into(),
                    parent: None,
                },
                Edit::CreateOrg {
                    name: "b".into(),
                    parent: Some("a".into()),
                },
                Edit::SetParent {
                    id: "a".into(),
                    parent: Some("b".into()),
                },
            ],
        );
        assert!(p.error.is_some());
        let _ = std::fs::remove_dir_all(&home);
    }
}
