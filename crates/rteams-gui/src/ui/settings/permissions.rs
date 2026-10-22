//! Permissions / tool approval. Stored flat in `AppConfig.extra` (no rteams_services types, so the
//! launcher and server can read them from the raw JSON):
//! - `tool_approval.default`   : "skip" | "edits" | "ask" (default "skip", the user's choice)
//! - `tool_approval.auto_edits`: bool, auto-allow file edits (default true)
//! - `tool_approval.safe_bash` : [string] command prefixes that never need approval
//! - `tool_approval.deny`      : [string] tool names / command prefixes that are always denied
//! - `tool_approval.timeout_s` : u64 seconds before an unanswered approval times out (default 600)
//!
//! The server's `RTEAMS_PERMISSION_TIMEOUT_MS` equals `timeout_s * 1000`.

#![allow(dead_code)] // getters are read by the launcher/server

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use serde_json::{json, Map, Value};

use super::{card, hint, toggle};
use crate::theme;
use crate::ui::onboarding::tx;

pub const DEFAULT: &str = "tool_approval.default";
pub const AUTO_EDITS: &str = "tool_approval.auto_edits";
pub const SAFE_BASH: &str = "tool_approval.safe_bash";
pub const DENY: &str = "tool_approval.deny";
pub const TIMEOUT_S: &str = "tool_approval.timeout_s";
pub const DEFAULT_TIMEOUT_S: u64 = 600;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolApproval {
    /// "skip" | "edits" | "ask"
    pub default: String,
    pub auto_edits: bool,
    pub safe_bash: Vec<String>,
    pub deny: Vec<String>,
    pub timeout_s: u64,
}

impl Default for ToolApproval {
    fn default() -> Self {
        ToolApproval {
            default: "skip".into(),
            auto_edits: true,
            safe_bash: ["ls", "pwd", "cat", "git status", "git diff", "git log"]
                .map(String::from)
                .to_vec(),
            deny: vec![],
            timeout_s: DEFAULT_TIMEOUT_S,
        }
    }
}

fn list(extra: &Map<String, Value>, key: &str, dflt: Vec<String>) -> Vec<String> {
    match extra.get(key).and_then(Value::as_array) {
        Some(a) => a
            .iter()
            .filter_map(Value::as_str)
            .map(String::from)
            .collect(),
        None => dflt,
    }
}

impl ToolApproval {
    pub fn from_extra(extra: &Map<String, Value>) -> Self {
        let d = ToolApproval::default();
        let mode = extra.get(DEFAULT).and_then(Value::as_str).unwrap_or("");
        ToolApproval {
            default: if ["skip", "edits", "ask"].contains(&mode) {
                mode.into()
            } else {
                d.default
            },
            auto_edits: extra
                .get(AUTO_EDITS)
                .and_then(Value::as_bool)
                .unwrap_or(d.auto_edits),
            safe_bash: list(extra, SAFE_BASH, d.safe_bash),
            deny: list(extra, DENY, d.deny),
            timeout_s: extra
                .get(TIMEOUT_S)
                .and_then(Value::as_u64)
                .filter(|s| *s > 0)
                .unwrap_or(d.timeout_s),
        }
    }

    pub fn load(home: &std::path::Path) -> Self {
        Self::from_extra(&AppConfig::load(home).extra)
    }

    pub fn write(&self, extra: &mut Map<String, Value>) {
        extra.insert(DEFAULT.into(), json!(self.default));
        extra.insert(AUTO_EDITS.into(), json!(self.auto_edits));
        extra.insert(SAFE_BASH.into(), json!(self.safe_bash));
        extra.insert(DENY.into(), json!(self.deny));
        extra.insert(TIMEOUT_S.into(), json!(self.timeout_s));
    }

    /// Value for `RTEAMS_PERMISSION_TIMEOUT_MS`.
    pub fn timeout_ms(&self) -> u64 {
        self.timeout_s.saturating_mul(1000)
    }

    /// Does a tool call need no prompt? `bash` checks the command prefix lists.
    pub fn decide(&self, tool: &str, bash_cmd: Option<&str>) -> Decision {
        let hit = |l: &[String], s: &str| {
            l.iter()
                .any(|p| !p.is_empty() && s.trim_start().starts_with(p.as_str()))
        };
        if self.deny.iter().any(|d| d == tool) || bash_cmd.is_some_and(|c| hit(&self.deny, c)) {
            return Decision::Deny;
        }
        if self.default == "skip" {
            return Decision::Allow;
        }
        let edit = matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit");
        if edit && (self.auto_edits || self.default == "edits") {
            return Decision::Allow;
        }
        if bash_cmd.is_some_and(|c| hit(&self.safe_bash, c)) {
            return Decision::Allow;
        }
        Decision::Ask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Ask,
    Deny,
}

#[derive(Default)]
pub struct State {
    loaded: bool,
    safe: String,
    deny: String,
}

fn lines(s: &str) -> Vec<String> {
    s.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

pub fn show(ui: &mut egui::Ui, cfg: &mut AppConfig, st: &mut State, tr: &I18n) -> bool {
    let mut ta = ToolApproval::from_extra(&cfg.extra);
    if !st.loaded {
        st.loaded = true;
        st.safe = ta.safe_bash.join("\n");
        st.deny = ta.deny.join("\n");
    }
    let before = ta.clone();
    card(
        ui,
        &tx(tr, "rteams.settings.permissions.title", "Tool approval"),
        |ui| {
            hint(
                ui,
                &tx(
                    tr,
                    "rteams.settings.permissions.hint",
                    "How agents' tool calls are approved when no per-launch override is set.",
                ),
            );
            let mut idx = ["skip", "edits", "ask"]
                .iter()
                .position(|m| *m == ta.default)
                .unwrap_or(2);
            let labels = [
                tx(tr, "rteams.settings.permissions.skip", "Skip all"),
                tx(tr, "rteams.settings.permissions.edits", "Auto-allow edits"),
                tx(tr, "rteams.settings.permissions.ask", "Ask every time"),
            ];
            let l: Vec<&str> = labels.iter().map(String::as_str).collect();
            if theme::segmented(ui, &mut idx, &l) {
                ta.default = ["skip", "edits", "ask"][idx].into();
            }
            if ta.default == "skip" {
                ui.label(
                    RichText::new(tx(
                        tr,
                        "rteams.settings.permissions.skipWarn",
                        "Agents can run any command without asking. Deny rules still apply.",
                    ))
                    .size(12.0)
                    .color(theme::WARNING),
                );
            }
        },
    );
    card(
        ui,
        &tx(tr, "rteams.settings.permissions.rules", "Per-tool rules"),
        |ui| {
            toggle(
                ui,
                &mut ta.auto_edits,
                &tx(
                    tr,
                    "rteams.settings.permissions.autoEdits",
                    "Auto-allow file edits",
                ),
                &tx(
                    tr,
                    "rteams.settings.permissions.autoEditsHint",
                    "Edit, Write and MultiEdit never prompt.",
                ),
            );
            ui.label(tx(
                tr,
                "rteams.settings.permissions.safeBash",
                "Safe bash prefixes (one per line)",
            ));
            if ui
                .add(
                    egui::TextEdit::multiline(&mut st.safe)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                ta.safe_bash = lines(&st.safe);
            }
            ui.label(tx(
                tr,
                "rteams.settings.permissions.deny",
                "Deny list: tool names or command prefixes (one per line)",
            ));
            if ui
                .add(
                    egui::TextEdit::multiline(&mut st.deny)
                        .hint_text("rm -rf")
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                )
                .changed()
            {
                ta.deny = lines(&st.deny);
            }
        },
    );
    card(
        ui,
        &tx(
            tr,
            "rteams.settings.permissions.timeout",
            "Approval timeout",
        ),
        |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut ta.timeout_s)
                        .range(5..=86_400)
                        .suffix(" s"),
                );
                ui.monospace(format!("RTEAMS_PERMISSION_TIMEOUT_MS={}", ta.timeout_ms()));
            });
            hint(
                ui,
                &tx(
                    tr,
                    "rteams.settings.permissions.timeoutHint",
                    "Unanswered approvals are denied after this long (default 600 s).",
                ),
            );
        },
    );
    if ta != before {
        ta.write(&mut cfg.extra);
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let mut e = Map::new();
        assert_eq!(ToolApproval::from_extra(&e), ToolApproval::default());
        let t = ToolApproval {
            default: "edits".into(),
            auto_edits: false,
            safe_bash: vec!["make".into()],
            deny: vec!["rm -rf".into()],
            timeout_s: 30,
        };
        t.write(&mut e);
        assert_eq!(ToolApproval::from_extra(&e), t);
        assert_eq!(t.timeout_ms(), 30_000);
        e.insert(DEFAULT.into(), json!("bogus"));
        assert_eq!(ToolApproval::from_extra(&e).default, "skip");
    }

    #[test]
    fn decide_rules() {
        let mut t = ToolApproval {
            default: "ask".into(),
            deny: vec!["rm -rf".into(), "WebFetch".into()],
            ..Default::default()
        };
        assert_eq!(t.decide("Edit", None), Decision::Allow);
        assert_eq!(t.decide("Bash", Some("git status -s")), Decision::Allow);
        assert_eq!(t.decide("Bash", Some("curl x")), Decision::Ask);
        assert_eq!(t.decide("Bash", Some("rm -rf /")), Decision::Deny);
        assert_eq!(t.decide("WebFetch", None), Decision::Deny);
        t.default = "skip".into();
        assert_eq!(t.decide("Bash", Some("curl x")), Decision::Allow);
        assert_eq!(t.decide("Bash", Some("rm -rf /")), Decision::Deny);
    }
}
