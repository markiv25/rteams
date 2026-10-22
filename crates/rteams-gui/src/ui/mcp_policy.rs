//! Per-member MCP policy panel (the original's MemberMcpPolicyPanel): lists the MCP servers
//! configured for the member's runtime (user + project scopes) and lets the lead choose which
//! ones the agent may use in addition to rteams itself (`Member.mcp_allow`).

use std::path::PathBuf;

use eframe::egui::{self, RichText};
use rteams_services::ext::mcp_config::{self, ClientPaths, Runtime, Scope};
use rteams_services::i18n::I18n;

use crate::theme;

/// One configured server, with the scopes it is defined in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    pub name: String,
    pub scopes: Vec<&'static str>,
}

/// Servers of `rt` in the user and project scopes, de-duplicated by name and sorted.
/// The built-in `rteams` server is always allowed, so it is not listed.
pub fn servers(paths: &ClientPaths, rt: Runtime) -> Vec<ServerRow> {
    let mut rows: Vec<ServerRow> = Vec::new();
    for scope in [Scope::User, Scope::Project] {
        if !rt.scopes().contains(&scope) || (scope == Scope::Project && paths.project.is_none()) {
            continue;
        }
        let Ok(list) = mcp_config::list_scope(paths, rt, scope) else {
            continue;
        };
        for s in list {
            if s.name == "rteams" {
                continue;
            }
            match rows.iter_mut().find(|r| r.name == s.name) {
                Some(r) => r.scopes.push(scope.id()),
                None => rows.push(ServerRow {
                    name: s.name,
                    scopes: vec![scope.id()],
                }),
            }
        }
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));
    rows
}

/// Adds or removes `name`, keeping the list sorted and unique.
pub fn set_allowed(allow: &mut Vec<String>, name: &str, on: bool) {
    allow.retain(|n| n != name);
    if on {
        allow.push(name.to_string());
    }
    allow.sort();
}

#[derive(Debug, Default)]
pub struct McpPolicy {
    /// `(member, runtime id)` the state was loaded for.
    loaded: Option<(String, String)>,
    saved: Vec<String>,
    allow: Vec<String>,
    servers: Vec<ServerRow>,
}

impl McpPolicy {
    /// Loads the server list and the member's saved allow-list (once per member / runtime).
    pub fn load(
        &mut self,
        member: &str,
        runtime: Option<&str>,
        saved: &[String],
        project: Option<PathBuf>,
    ) {
        let rt_id = runtime.unwrap_or("claude").to_string();
        let key = (member.to_string(), rt_id.clone());
        if self.loaded.as_ref() == Some(&key) {
            return;
        }
        let rt = Runtime::from_id(&rt_id).unwrap_or(Runtime::Claude);
        self.servers = servers(&ClientPaths::real(project), rt);
        self.saved = saved.to_vec();
        self.saved.sort();
        self.allow = self.saved.clone();
        self.loaded = Some(key);
    }

    /// Re-read the config files on the next frame.
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    pub fn dirty(&self) -> bool {
        self.allow != self.saved
    }

    /// Draws the panel; returns the new allow-list when "Save" is clicked.
    pub fn show(&mut self, ui: &mut egui::Ui, tr: &I18n, can_write: bool) -> Option<Vec<String>> {
        ui.label(RichText::new(tr.t("rteams.mcp.title")).strong());
        ui.label(
            RichText::new(tr.t("rteams.mcp.hint"))
                .size(12.0)
                .color(theme::MUTED),
        );
        ui.add_space(4.0);
        let mut changes: Vec<(String, bool)> = Vec::new();
        if self.servers.is_empty() {
            ui.label(RichText::new(tr.t("rteams.mcp.none")).color(theme::MUTED));
        }
        for s in &self.servers {
            let mut on = self.allow.contains(&s.name);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_write, egui::Checkbox::new(&mut on, &s.name))
                    .changed()
                {
                    changes.push((s.name.clone(), on));
                }
                for sc in &s.scopes {
                    theme::pill(ui, tr.t(&format!("rteams.mcp.scope.{sc}")), theme::MUTED);
                }
            });
        }
        // Allowed names that are no longer configured stay visible so they can be removed.
        for name in self
            .allow
            .iter()
            .filter(|n| !self.servers.iter().any(|s| &s.name == *n))
        {
            let mut on = true;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(can_write, egui::Checkbox::new(&mut on, name))
                    .changed()
                {
                    changes.push((name.clone(), on));
                }
                theme::pill(ui, tr.t("rteams.mcp.missing"), theme::WARNING);
            });
        }
        for (name, on) in changes {
            set_allowed(&mut self.allow, &name, on);
        }
        ui.add_space(4.0);
        let mut saved = None;
        ui.horizontal(|ui| {
            let ready = can_write && self.dirty();
            if ui
                .add_enabled_ui(ready, |ui| {
                    theme::solid(ui, &tr.t("rteams.mcp.save"), theme::INFO)
                })
                .inner
                .clicked()
            {
                saved = Some(self.allow.clone());
                self.saved = self.allow.clone();
            }
            if ui.small_button(tr.t("rteams.mcp.reload")).clicked() {
                self.invalidate();
            }
        });
        saved
    }
}

#[cfg(test)]
mod mcp_policy_tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rteams-mcppol-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn allow_list_is_sorted_and_unique() {
        let mut a = vec!["b".to_string()];
        set_allowed(&mut a, "a", true);
        set_allowed(&mut a, "a", true);
        assert_eq!(a, ["a", "b"]);
        set_allowed(&mut a, "b", false);
        assert_eq!(a, ["a"]);
    }

    #[test]
    fn lists_user_and_project_servers() {
        let home = tmp("home");
        let proj = tmp("proj");
        let paths = ClientPaths::new(&home, Some(proj.clone()));
        let spec = mcp_config::ServerSpec::stdio("npx", &["x"]);
        mcp_config::upsert(&paths, Runtime::Claude, Scope::User, "alpha", &spec).unwrap();
        mcp_config::upsert(&paths, Runtime::Claude, Scope::Project, "beta", &spec).unwrap();
        mcp_config::upsert(&paths, Runtime::Claude, Scope::Project, "alpha", &spec).unwrap();
        mcp_config::upsert(&paths, Runtime::Claude, Scope::Project, "rteams", &spec).unwrap();
        let rows = servers(&paths, Runtime::Claude);
        assert_eq!(
            rows,
            vec![
                ServerRow {
                    name: "alpha".into(),
                    scopes: vec!["user", "project"]
                },
                ServerRow {
                    name: "beta".into(),
                    scopes: vec!["project"]
                },
            ]
        );
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(proj);
    }

    #[test]
    fn panel_tracks_dirty_and_renders() {
        let mut p = McpPolicy {
            loaded: Some(("ada".into(), "claude".into())),
            saved: vec![],
            allow: vec![],
            servers: vec![ServerRow {
                name: "alpha".into(),
                scopes: vec!["user"],
            }],
        };
        assert!(!p.dirty());
        set_allowed(&mut p.allow, "alpha", true);
        set_allowed(&mut p.allow, "gone", true);
        assert!(p.dirty());
        crate::view::testing::render(|ui| {
            assert!(p.show(ui, crate::view::testing::tr(), true).is_none());
        });
    }
}
