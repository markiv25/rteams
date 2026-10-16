//! Middle column: `Organization setup` with the Structure / Teams / Links / Orgs tabs. The
//! panel never mutates the map; it returns `Edit`s for the caller to apply and save.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::orgs::model::{OrgMap, RelationKind};
use rteams_services::orgs::projection::OrgView;
use rteams_services::orgs::structure::Edit;

use super::list::{color_hex, parse_color, Selection};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Structure,
    Teams,
    Links,
    Orgs,
}

#[derive(Default)]
pub struct EditorPanel {
    pub tab: Tab,
    buf_for: Selection,
    name: String,
    desc: String,
    new_org: String,
    new_org_parent: Option<String>,
    new_group: String,
    place_team: String,
    /// `(org, group)` target of "Assign team".
    place_under: Option<(String, Option<String>)>,
    rel_source: String,
    rel_target: String,
    rel_kind: Option<RelationKind>,
}

fn combo<T: Clone + PartialEq>(
    ui: &mut egui::Ui,
    id: &str,
    current: &mut T,
    options: impl IntoIterator<Item = (T, String)>,
    placeholder: &str,
) {
    let options: Vec<(T, String)> = options.into_iter().collect();
    let text = options
        .iter()
        .find(|(v, _)| v == current)
        .map_or_else(|| placeholder.to_string(), |(_, l)| l.clone());
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .show_ui(ui, |ui| {
            for (v, l) in options {
                ui.selectable_value(current, v, l);
            }
        });
}

/// `(org id, group id, label)` for every place a team can go.
pub fn places(map: &OrgMap) -> Vec<((String, Option<String>), String)> {
    let mut v = Vec::new();
    for o in &map.orgs {
        v.push(((o.id.clone(), None), o.name.clone()));
        for g in &o.groups {
            v.push((
                (o.id.clone(), Some(g.id.clone())),
                format!("{} / {}", o.name, g.name),
            ));
        }
    }
    v
}

impl EditorPanel {
    fn sync(&mut self, map: &OrgMap, sel: &Selection) {
        if self.buf_for == *sel {
            return;
        }
        self.buf_for = sel.clone();
        let (name, desc) = match sel {
            Selection::Org(id) => map
                .org(id)
                .map(|o| (o.name.clone(), o.description.clone()))
                .unwrap_or_default(),
            Selection::Group(o, g) => map
                .org(o)
                .and_then(|o| o.groups.iter().find(|x| x.id == *g))
                .map(|g| (g.name.clone(), g.description.clone()))
                .unwrap_or_default(),
            _ => Default::default(),
        };
        self.name = name;
        self.desc = desc;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        map: &OrgMap,
        view: &OrgView,
        sel: &Selection,
    ) -> Vec<Edit> {
        self.sync(map, sel);
        let k = |s: &str| tr.t(&format!("team.organizations.editor.{s}"));
        let mut out = Vec::new();
        ui.strong(k("title"));
        ui.horizontal_wrapped(|ui| {
            theme::pill(
                ui,
                tr.t_count(
                    "team.organizations.editor.badges.groups",
                    map.orgs.iter().map(|o| o.groups.len()).sum::<usize>() as u64,
                ),
                theme::TEXT_2,
            );
            let placed: usize = map
                .orgs
                .iter()
                .map(|o| o.teams.len() + o.groups.iter().map(|g| g.teams.len()).sum::<usize>())
                .sum();
            theme::pill(
                ui,
                tr.t_count("team.organizations.editor.badges.placed", placed as u64),
                theme::SUCCESS,
            );
            theme::pill(
                ui,
                tr.t_count(
                    "team.organizations.editor.badges.unplaced",
                    view.unplaced.len() as u64,
                ),
                theme::WARNING,
            );
        });
        ui.horizontal(|ui| {
            for (t, key) in [
                (Tab::Structure, "tabs.structure"),
                (Tab::Teams, "tabs.teams"),
                (Tab::Links, "tabs.links"),
                (Tab::Orgs, "tabs.organizations"),
            ] {
                ui.selectable_value(&mut self.tab, t, k(key));
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("orgs-editor")
            .show(ui, |ui| match self.tab {
                Tab::Orgs => self.orgs_tab(ui, tr, map, sel, &mut out),
                Tab::Structure => self.structure_tab(ui, tr, map, sel, &mut out),
                Tab::Teams => self.teams_tab(ui, tr, map, view, &mut out),
                Tab::Links => self.links_tab(ui, tr, map, view, &mut out),
            });
        out
    }

    fn orgs_tab(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        map: &OrgMap,
        sel: &Selection,
        out: &mut Vec<Edit>,
    ) {
        let k = |s: &str| tr.t(&format!("team.organizations.editor.{s}"));
        ui.label(RichText::new(k("newOrganization")).strong());
        ui.horizontal_wrapped(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_org).desired_width(160.0));
            let mut parent = self.new_org_parent.clone();
            combo(
                ui,
                "orgs-new-parent",
                &mut parent,
                std::iter::once((None, k("topLevel"))).chain(
                    map.orgs
                        .iter()
                        .map(|o| (Some(o.id.clone()), o.name.clone())),
                ),
                &k("parentOrganization"),
            );
            self.new_org_parent = parent;
            if ui
                .add_enabled(
                    !self.new_org.trim().is_empty(),
                    egui::Button::new(k("actions.create")),
                )
                .clicked()
            {
                out.push(Edit::CreateOrg {
                    name: std::mem::take(&mut self.new_org),
                    parent: self.new_org_parent.clone(),
                });
            }
        });
        let Selection::Org(id) = sel else {
            return;
        };
        let Some(org) = map.org(id) else {
            return;
        };
        ui.separator();
        ui.label(RichText::new(k("selectedItem")).strong());
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(200.0));
            if (r.lost_focus() && self.name.trim() != org.name && !self.name.trim().is_empty())
                || ui.button(tr.t("common.actions.save")).clicked() && self.name.trim() != org.name
            {
                out.push(Edit::RenameOrg {
                    id: id.clone(),
                    name: self.name.clone(),
                });
            }
        });
        ui.horizontal(|ui| {
            let c = parse_color(&org.color);
            let mut rgb = [c.r(), c.g(), c.b()];
            if ui.color_edit_button_srgb(&mut rgb).changed() {
                out.push(Edit::SetColor {
                    id: id.clone(),
                    color: color_hex(rgb),
                });
            }
            let mut parent = org.parent.clone();
            let sub = map.subtree(id);
            combo(
                ui,
                "orgs-parent",
                &mut parent,
                std::iter::once((None, k("topLevel"))).chain(
                    map.orgs
                        .iter()
                        .filter(|o| !sub.contains(&o.id))
                        .map(|o| (Some(o.id.clone()), o.name.clone())),
                ),
                &k("parentOrganization"),
            );
            if parent != org.parent {
                out.push(Edit::SetParent {
                    id: id.clone(),
                    parent,
                });
            }
        });
        let r = ui.add(
            egui::TextEdit::multiline(&mut self.desc)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        if r.lost_focus() && self.desc != org.description {
            out.push(Edit::SetDescription {
                id: id.clone(),
                text: self.desc.clone(),
            });
        }
        if ui
            .button(RichText::new(k("actions.remove")).color(theme::ERROR))
            .clicked()
        {
            out.push(Edit::DeleteOrg { id: id.clone() });
        }
    }

    fn structure_tab(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        map: &OrgMap,
        sel: &Selection,
        out: &mut Vec<Edit>,
    ) {
        let k = |s: &str| tr.t(&format!("team.organizations.editor.{s}"));
        let org = match sel {
            Selection::Org(o) | Selection::Group(o, _) => map.org(o),
            _ => None,
        };
        let Some(org) = org else {
            ui.label(RichText::new(k("rootSelected")).color(theme::MUTED));
            return;
        };
        ui.label(RichText::new(k("createGroup")).strong());
        ui.label(
            RichText::new(k("underLabel").replace("{{label}}", &org.name)).color(theme::TEXT_2),
        );
        ui.horizontal_wrapped(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.new_group)
                    .hint_text(k("groupPlaceholder"))
                    .desired_width(200.0),
            );
            if ui
                .add_enabled(
                    !self.new_group.trim().is_empty(),
                    egui::Button::new(k("actions.addGroup")),
                )
                .clicked()
            {
                out.push(Edit::CreateGroup {
                    org: org.id.clone(),
                    name: std::mem::take(&mut self.new_group),
                });
            }
        });
        if let Selection::Group(o, g) = sel {
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(200.0));
                if ui.button(tr.t("common.actions.save")).clicked() && !self.name.trim().is_empty()
                {
                    out.push(Edit::RenameGroup {
                        org: o.clone(),
                        group: g.clone(),
                        name: self.name.clone(),
                    });
                }
                if ui
                    .button(RichText::new(k("actions.remove")).color(theme::ERROR))
                    .clicked()
                {
                    out.push(Edit::DeleteGroup {
                        org: o.clone(),
                        group: g.clone(),
                    });
                }
            });
        }
    }

    fn teams_tab(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        map: &OrgMap,
        view: &OrgView,
        out: &mut Vec<Edit>,
    ) {
        let k = |s: &str| tr.t(&format!("team.organizations.editor.{s}"));
        ui.label(RichText::new(k("placeExistingTeam")).strong());
        let mut all: Vec<(String, String)> = view
            .unplaced
            .iter()
            .map(|t| (t.dir.clone(), t.name.clone()))
            .collect();
        for o in &map.orgs {
            for t in o
                .teams
                .iter()
                .chain(o.groups.iter().flat_map(|g| g.teams.iter()))
            {
                all.push((t.clone(), t.clone()));
            }
        }
        ui.horizontal_wrapped(|ui| {
            let mut team = self.place_team.clone();
            combo(
                ui,
                "orgs-place-team",
                &mut team,
                all.iter().map(|(d, n)| (d.clone(), n.clone())),
                &k("teamPlaceholder"),
            );
            self.place_team = team;
            let mut under = self.place_under.clone();
            combo(
                ui,
                "orgs-place-under",
                &mut under,
                places(map).into_iter().map(|(p, l)| (Some(p), l)),
                &k("placeUnderPlaceholder"),
            );
            self.place_under = under;
            let ready = !self.place_team.is_empty() && self.place_under.is_some();
            if ui
                .add_enabled(ready, egui::Button::new(k("actions.assignTeam")))
                .clicked()
            {
                if let Some((org, group)) = self.place_under.clone() {
                    out.push(Edit::PlaceTeam {
                        team: self.place_team.clone(),
                        org,
                        group,
                    });
                }
            }
        });
        ui.separator();
        for o in &map.orgs {
            let rows = o
                .teams
                .iter()
                .map(|t| (t, o.name.clone()))
                .chain(o.groups.iter().flat_map(|g| {
                    g.teams
                        .iter()
                        .map(|t| (t, format!("{} / {}", o.name, g.name)))
                }));
            for (t, place) in rows {
                ui.horizontal(|ui| {
                    ui.label(t);
                    ui.label(RichText::new(place).color(theme::MUTED));
                    if ui.small_button(k("actions.remove")).clicked() {
                        out.push(Edit::RemoveTeam { team: t.clone() });
                    }
                });
            }
        }
    }

    fn links_tab(
        &mut self,
        ui: &mut egui::Ui,
        tr: &I18n,
        map: &OrgMap,
        view: &OrgView,
        out: &mut Vec<Edit>,
    ) {
        let k = |s: &str| tr.t(&format!("team.organizations.editor.{s}"));
        ui.label(RichText::new(k("manualRelation")).strong());
        let mut teams: Vec<String> = view.unplaced.iter().map(|t| t.dir.clone()).collect();
        for o in &map.orgs {
            teams.extend(o.teams.iter().cloned());
            teams.extend(o.groups.iter().flat_map(|g| g.teams.iter().cloned()));
        }
        teams.sort();
        teams.dedup();
        ui.horizontal_wrapped(|ui| {
            let opts = |teams: &[String]| {
                teams
                    .iter()
                    .map(|t| (t.clone(), t.clone()))
                    .collect::<Vec<_>>()
            };
            let mut s = self.rel_source.clone();
            combo(
                ui,
                "orgs-rel-src",
                &mut s,
                opts(&teams),
                &k("teamPlaceholder"),
            );
            self.rel_source = s;
            let mut kind = self.rel_kind;
            combo(
                ui,
                "orgs-rel-kind",
                &mut kind,
                RelationKind::ALL
                    .iter()
                    .map(|r| (Some(*r), r.label().to_string())),
                &k("kindPlaceholder"),
            );
            self.rel_kind = kind;
            let mut t = self.rel_target.clone();
            combo(
                ui,
                "orgs-rel-dst",
                &mut t,
                opts(&teams),
                &k("targetPlaceholder"),
            );
            self.rel_target = t;
            let ready = !self.rel_source.is_empty()
                && !self.rel_target.is_empty()
                && self.rel_kind.is_some();
            if ui
                .add_enabled(ready, egui::Button::new(k("actions.addRelation")))
                .clicked()
            {
                if let Some(kind) = self.rel_kind {
                    out.push(Edit::AddRelation {
                        source: self.rel_source.clone(),
                        target: self.rel_target.clone(),
                        kind,
                    });
                }
            }
        });
        ui.separator();
        for r in &map.relations {
            ui.horizontal(|ui| {
                ui.label(format!("{}  {}  {}", r.source, r.kind.label(), r.target));
                if ui.small_button(k("actions.remove")).clicked() {
                    out.push(Edit::RemoveRelation { id: r.id.clone() });
                }
            });
        }
    }
}
