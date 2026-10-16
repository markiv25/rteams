//! Local Claude root: auto-detected (`CLAUDE_CONFIG_DIR` or `~/.claude`) or a custom folder
//! saved in the config under `claude_root_override`.

use std::path::{Path, PathBuf};

use eframe::egui;
use rteams_services::claude_root::{self, RootSource, Validation};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;

use super::{card, extra_set, extra_str, hint};
use crate::theme;

pub const KEY: &str = "claude_root_override";

#[derive(Default)]
pub struct State {
    pub input: String,
    /// A candidate that needs confirmation before it is saved.
    pub pending: Option<(PathBuf, Validation)>,
}

pub fn override_of(cfg: &AppConfig) -> Option<String> {
    let s = extra_str(&cfg.extra, KEY);
    (!s.is_empty()).then(|| s.to_string())
}

/// Resolve the effective root from the config using an explicit env value and home.
pub fn effective(cfg: &AppConfig, env: Option<&str>, user_home: &Path) -> claude_root::ClaudeRoot {
    claude_root::resolve(override_of(cfg).as_deref(), env, user_home)
}

/// Try to use `path`: saved at once when clean, otherwise held in `st.pending` for confirmation.
/// Returns true when the config changed.
pub fn propose(cfg: &mut AppConfig, st: &mut State, path: PathBuf) -> bool {
    let v = claude_root::validate(&path);
    if v.is_clean() {
        st.pending = None;
        accept(cfg, &path)
    } else {
        st.pending = Some((path, v));
        false
    }
}

fn accept(cfg: &mut AppConfig, path: &Path) -> bool {
    extra_set(
        &mut cfg.extra,
        KEY,
        serde_json::Value::String(path.to_string_lossy().into_owned()),
    );
    true
}

pub fn clear(cfg: &mut AppConfig) -> bool {
    cfg.extra.remove(KEY).is_some()
}

pub fn show(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    st: &mut State,
    tr: &I18n,
    user_home: &Path,
) -> bool {
    let mut changed = false;
    card(ui, &tr.t("settings.general.localClaudeRoot.title"), |ui| {
        hint(ui, &tr.t("settings.general.localClaudeRoot.description"));
        let env = std::env::var("CLAUDE_CONFIG_DIR").ok();
        let root = effective(cfg, env.as_deref(), user_home);
        ui.label(tr.t("settings.general.localClaudeRoot.current.label"));
        ui.monospace(root.path.display().to_string());
        let mode = if root.source == RootSource::Override {
            "settings.general.localClaudeRoot.current.customPath"
        } else {
            "settings.general.localClaudeRoot.current.autoDetectedPath"
        };
        hint(ui, &tr.t(mode));

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut st.input)
                    .desired_width(260.0)
                    .hint_text("/path/to/.claude"),
            );
            if ui
                .add_enabled(
                    !st.input.trim().is_empty(),
                    egui::Button::new(tr.t("settings.general.localClaudeRoot.actions.usePath")),
                )
                .clicked()
            {
                let p = PathBuf::from(st.input.trim());
                changed |= propose(cfg, st, p);
            }
            if ui
                .button(tr.t("settings.general.localClaudeRoot.actions.selectFolder"))
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .set_directory(&root.path)
                    .pick_folder()
                {
                    changed |= propose(cfg, st, p);
                }
            }
            if root.source == RootSource::Override
                && ui
                    .button(tr.t("settings.general.localClaudeRoot.actions.useAutoDetect"))
                    .clicked()
            {
                changed |= clear(cfg);
                st.pending = None;
            }
        });

        if let Some((path, v)) = st.pending.clone() {
            let mut lines = Vec::new();
            if !v.named_claude {
                lines.push(tr.t_args(
                    "settings.general.localClaudeRoot.confirm.notClaudeDir.message",
                    &[("folderName", &v.folder_name)],
                ));
            }
            if !v.has_projects {
                lines.push(tr.t("settings.general.localClaudeRoot.confirm.noProjectsDir.message"));
            }
            for l in lines {
                ui.colored_label(theme::WARNING, l);
            }
            ui.horizontal(|ui| {
                if ui
                    .button(tr.t("settings.general.localClaudeRoot.actions.useThisPath"))
                    .clicked()
                {
                    changed |= accept(cfg, &path);
                    st.pending = None;
                }
                if ui
                    .button(tr.t("settings.workspaceProfiles.actions.cancel"))
                    .clicked()
                {
                    st.pending = None;
                }
            });
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_path_saves_and_unclean_needs_confirmation() {
        let d = tempfile::tempdir().unwrap();
        let good = d.path().join(".claude");
        std::fs::create_dir_all(good.join("projects")).unwrap();
        let other = d.path().join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        let mut cfg = AppConfig::default();
        let mut st = State::default();
        assert!(!propose(&mut cfg, &mut st, other));
        assert!(st.pending.is_some() && override_of(&cfg).is_none());
        assert!(propose(&mut cfg, &mut st, good.clone()));
        assert_eq!(override_of(&cfg).unwrap(), good.to_string_lossy());
        assert_eq!(
            effective(&cfg, Some("/env"), d.path()).source,
            RootSource::Override
        );
        assert!(clear(&mut cfg));
        assert_eq!(
            effective(&cfg, Some("/env"), d.path()).source,
            RootSource::Env
        );
    }
}
