//! Workspace profiles: named sets of project paths and teams, stored in
//! `AppConfig.extra["workspace_profiles"]`. Applying one replaces `custom_project_paths`.

use std::path::PathBuf;

use eframe::egui;
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use serde_json::json;

use super::{card, hint};
use crate::theme;

const KEY: &str = "workspace_profiles";
const ACTIVE: &str = "active_workspace_profile";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceProfile {
    pub name: String,
    pub paths: Vec<String>,
    pub teams: Vec<String>,
}

#[derive(Default)]
pub struct State {
    /// (index being edited or None for new, draft, paths text, teams text)
    pub edit: Option<(Option<usize>, WorkspaceProfile, String, String)>,
    pub confirm_delete: Option<usize>,
    pub error: Option<String>,
}

fn strings(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

pub fn list(cfg: &AppConfig) -> Vec<WorkspaceProfile> {
    cfg.extra
        .get(KEY)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|p| WorkspaceProfile {
                    name: p.get("name").and_then(|n| n.as_str()).unwrap_or("").into(),
                    paths: strings(p, "paths"),
                    teams: strings(p, "teams"),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn store(cfg: &mut AppConfig, profiles: &[WorkspaceProfile]) {
    let v: Vec<_> = profiles
        .iter()
        .map(|p| json!({"name": p.name, "paths": p.paths, "teams": p.teams}))
        .collect();
    cfg.extra.insert(KEY.into(), json!(v));
}

pub fn active(cfg: &AppConfig) -> Option<String> {
    cfg.extra
        .get(ACTIVE)
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Make `p` the active profile: its paths become the custom project paths.
pub fn apply(cfg: &mut AppConfig, p: &WorkspaceProfile) {
    cfg.custom_project_paths = p.paths.iter().map(PathBuf::from).collect();
    cfg.extra.insert(ACTIVE.into(), json!(p.name));
}

fn lines(s: &str, sep: fn(char) -> bool) -> Vec<String> {
    s.split(sep)
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub fn show(ui: &mut egui::Ui, cfg: &mut AppConfig, st: &mut State, tr: &I18n) -> bool {
    let mut changed = false;
    let mut profiles = list(cfg);
    let act = active(cfg);

    card(ui, &tr.t("settings.workspaceProfiles.title"), |ui| {
        hint(
            ui,
            "Named sets of project paths and teams. Apply one to switch workspaces.",
        );
        if profiles.is_empty() && st.edit.is_none() {
            ui.label(tr.t("settings.workspaceProfiles.empty.title"));
        }
        let mut apply_idx = None;
        let mut edit_idx = None;
        for (i, p) in profiles.iter().enumerate() {
            ui.horizontal(|ui| {
                let is_active = act.as_deref() == Some(p.name.as_str());
                ui.label(egui::RichText::new(&p.name).strong());
                if is_active {
                    ui.colored_label(theme::SUCCESS, "active");
                }
                hint(
                    ui,
                    &format!("{} paths, {} teams", p.paths.len(), p.teams.len()),
                );
                if ui.small_button("Apply").clicked() {
                    apply_idx = Some(i);
                }
                if ui
                    .small_button(tr.t("settings.workspaceProfiles.actions.editProfile"))
                    .clicked()
                {
                    edit_idx = Some(i);
                }
                if ui
                    .small_button(tr.t("settings.workspaceProfiles.actions.deleteProfile"))
                    .clicked()
                {
                    st.confirm_delete = Some(i);
                }
            });
        }
        if let Some(i) = apply_idx {
            apply(cfg, &profiles[i]);
            changed = true;
        }
        if let Some(i) = edit_idx {
            let p = profiles[i].clone();
            st.edit = Some((Some(i), p.clone(), p.paths.join("\n"), p.teams.join(", ")));
        }
        if let Some(i) = st.confirm_delete {
            if let Some(p) = profiles.get(i) {
                ui.colored_label(
                    theme::WARNING,
                    tr.t_args(
                        "settings.workspaceProfiles.deleteConfirm.message",
                        &[("name", &p.name)],
                    ),
                );
                ui.horizontal(|ui| {
                    if ui
                        .button(tr.t("settings.workspaceProfiles.deleteConfirm.confirmLabel"))
                        .clicked()
                    {
                        profiles.remove(i);
                        store(cfg, &profiles);
                        st.confirm_delete = None;
                        changed = true;
                    }
                    if ui
                        .button(tr.t("settings.workspaceProfiles.actions.cancel"))
                        .clicked()
                    {
                        st.confirm_delete = None;
                    }
                });
            } else {
                st.confirm_delete = None;
            }
        }
        if st.edit.is_none()
            && ui
                .button(tr.t("settings.workspaceProfiles.actions.addProfile"))
                .clicked()
        {
            st.edit = Some((
                None,
                WorkspaceProfile::default(),
                String::new(),
                String::new(),
            ));
        }

        let mut done = false;
        if let Some((idx, draft, paths, teams)) = st.edit.as_mut() {
            ui.separator();
            ui.label(tr.t("settings.workspaceProfiles.form.name"));
            ui.add(
                egui::TextEdit::singleline(&mut draft.name)
                    .hint_text(tr.t("settings.workspaceProfiles.form.namePlaceholder")),
            );
            ui.label("Project paths (one per line)");
            ui.add(
                egui::TextEdit::multiline(paths)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
            if ui
                .button(tr.t("settings.general.localClaudeRoot.actions.selectFolder"))
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    if !paths.is_empty() && !paths.ends_with('\n') {
                        paths.push('\n');
                    }
                    paths.push_str(&p.to_string_lossy());
                }
            }
            ui.label("Teams (comma separated)");
            ui.add(egui::TextEdit::singleline(teams).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui
                    .button(tr.t("settings.workspaceProfiles.actions.save"))
                    .clicked()
                {
                    draft.name = draft.name.trim().to_string();
                    let dup = profiles
                        .iter()
                        .enumerate()
                        .any(|(i, p)| p.name == draft.name && Some(i) != *idx);
                    if draft.name.is_empty() || dup {
                        st.error = Some(tr.t("settings.workspaceProfiles.form.name"));
                    } else {
                        draft.paths = lines(paths, |c| c == '\n');
                        draft.teams = lines(teams, |c| c == ',');
                        match idx {
                            Some(i) if *i < profiles.len() => profiles[*i] = draft.clone(),
                            _ => profiles.push(draft.clone()),
                        }
                        store(cfg, &profiles);
                        changed = true;
                        st.error = None;
                        done = true;
                    }
                }
                if ui
                    .button(tr.t("settings.workspaceProfiles.actions.cancel"))
                    .clicked()
                {
                    done = true;
                    st.error = None;
                }
            });
            if let Some(e) = &st.error {
                ui.colored_label(theme::ERROR, e);
            }
        }
        if done {
            st.edit = None;
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_list_apply() {
        let mut c = AppConfig::default();
        assert!(list(&c).is_empty());
        let p = WorkspaceProfile {
            name: "w".into(),
            paths: vec!["/a".into()],
            teams: vec!["t".into()],
        };
        store(&mut c, std::slice::from_ref(&p));
        assert_eq!(list(&c), vec![p.clone()]);
        apply(&mut c, &p);
        assert_eq!(c.custom_project_paths, vec![PathBuf::from("/a")]);
        assert_eq!(active(&c).as_deref(), Some("w"));
    }

    #[test]
    fn line_splitting() {
        assert_eq!(lines("a\n\n b \n", |c| c == '\n'), vec!["a", "b"]);
        assert_eq!(lines("x, y,,", |c| c == ','), vec!["x", "y"]);
    }
}
