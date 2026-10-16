//! General: app language, agent language, appearance, startup (launch at login), Local Claude
//! root.

use std::path::{Path, PathBuf};

use eframe::egui;
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;

use super::{card, hint, local_claude_root, toggle};
use crate::theme;
use crate::ui::i18n_widgets::locale_picker;

#[derive(Default)]
pub struct State {
    pub root: local_claude_root::State,
    pub autostart_error: Option<String>,
    /// Native title bar was toggled this session (needs a restart).
    pub restart_needed: bool,
}

const AGENT_LANGUAGES: [&str; 12] = [
    "system",
    "English",
    "Spanish",
    "French",
    "German",
    "Portuguese",
    "Russian",
    "Chinese",
    "Japanese",
    "Korean",
    "Hindi",
    "Arabic",
];

pub fn show(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    st: &mut State,
    tr: &I18n,
    locale_tr: &mut I18n,
    user_home: &Path,
) -> bool {
    let mut changed = false;

    card(ui, &tr.t("settings.general.appLanguage.title"), |ui| {
        hint(ui, &tr.t("settings.general.appLanguage.description"));
        ui.horizontal(|ui| {
            ui.label(tr.t("settings.general.appLanguage.label"));
            if locale_picker(ui, locale_tr) {
                cfg.general.locale = locale_tr.locale.clone();
                changed = true;
            }
            if theme::pill_button(
                ui,
                &tr.t("settings.general.appearance.theme.options.system"),
                cfg.general.locale == "system",
            )
            .clicked()
                && cfg.general.locale != "system"
            {
                cfg.general.locale = "system".into();
                *locale_tr = I18n::from_pref("system");
                changed = true;
            }
        });
    });

    card(ui, &tr.t("settings.general.agentLanguage.title"), |ui| {
        let detected = rteams_services::i18n::system_locale();
        hint(
            ui,
            &tr.t_args(
                "settings.general.agentLanguage.descriptionWithDetected",
                &[("detected", detected)],
            ),
        );
        ui.horizontal(|ui| {
            ui.label(tr.t("settings.general.agentLanguage.label"));
            let shown = if cfg.general.agent_language == "system" {
                tr.t("settings.general.appearance.theme.options.system")
            } else {
                cfg.general.agent_language.clone()
            };
            egui::ComboBox::from_id_salt("settings-agent-language")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    for l in AGENT_LANGUAGES {
                        let text = if l == "system" {
                            tr.t("settings.general.appearance.theme.options.system")
                        } else {
                            l.to_string()
                        };
                        if ui
                            .selectable_label(cfg.general.agent_language == l, text)
                            .clicked()
                            && cfg.general.agent_language != l
                        {
                            cfg.general.agent_language = l.into();
                            changed = true;
                        }
                    }
                });
        });
    });

    card(ui, &tr.t("settings.general.appearance.title"), |ui| {
        ui.horizontal(|ui| {
            ui.label(tr.t("settings.general.appearance.theme.label"));
            for (v, k) in [("dark", "dark"), ("light", "light"), ("system", "system")] {
                let label = tr.t(&format!("settings.general.appearance.theme.options.{k}"));
                if theme::pill_button(ui, &label, cfg.general.theme == v).clicked()
                    && cfg.general.theme != v
                {
                    cfg.general.theme = v.into();
                    changed = true;
                }
            }
        });
        hint(ui, &tr.t("settings.general.appearance.theme.description"));
        if toggle(
            ui,
            &mut cfg.general.native_title_bar,
            &tr.t("settings.general.appearance.nativeTitleBar.label"),
            &tr.t("settings.general.appearance.nativeTitleBar.description"),
        ) {
            st.restart_needed = true;
            changed = true;
        }
        if st.restart_needed {
            ui.colored_label(
                crate::theme::WARNING,
                format!(
                    "{}: {}",
                    tr.t("settings.general.appearance.nativeTitleBar.restartConfirm.title"),
                    tr.t("settings.general.appearance.nativeTitleBar.restartConfirm.message")
                ),
            );
        }
        changed |= toggle(
            ui,
            &mut cfg.general.expand_ai_groups,
            &tr.t("settings.general.appearance.autoExpandAIGroups.label"),
            &tr.t("settings.general.appearance.autoExpandAIGroups.description"),
        );
    });

    card(ui, &tr.t("settings.general.startup.title"), |ui| {
        if toggle(
            ui,
            &mut cfg.general.launch_at_login,
            &tr.t("settings.general.startup.launchAtLogin.label"),
            &tr.t("settings.general.startup.launchAtLogin.description"),
        ) {
            changed = true;
            let exe = std::env::current_exe().unwrap_or_default();
            st.autostart_error =
                set_launch_at_login(user_home, &exe, cfg.general.launch_at_login).err();
            if st.autostart_error.is_some() {
                cfg.general.launch_at_login = false;
            }
        }
        if let Some(e) = &st.autostart_error {
            ui.colored_label(crate::theme::ERROR, e);
        }
    });

    changed |= local_claude_root::show(ui, cfg, &mut st.root, tr, user_home);
    changed
}

// ---- launch at login -----------------------------------------------------------------------

/// Autostart entry for the current OS under `user_home`, if the OS is supported.
pub fn autostart_file(user_home: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(user_home.join("Library/LaunchAgents/com.rteams.gui.plist"))
    } else if cfg!(target_os = "linux") {
        Some(user_home.join(".config/autostart/rteams.desktop"))
    } else {
        None
    }
}

pub fn launch_agent_plist(exe: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>com.rteams.gui</string>\n  \
<key>ProgramArguments</key><array><string>{exe}</string></array>\n  \
<key>RunAtLoad</key><true/>\n</dict>\n</plist>\n"
    )
}

pub fn desktop_entry(exe: &Path) -> String {
    let exe = exe
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "[Desktop Entry]\nType=Application\nName=rteams\nExec=\"{exe}\"\nX-GNOME-Autostart-enabled=true\n"
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Create or remove the autostart entry under `user_home` (never touches anything else).
pub fn set_launch_at_login(user_home: &Path, exe: &Path, on: bool) -> Result<(), String> {
    let Some(path) = autostart_file(user_home) else {
        return Err("Launch at login is not supported on this platform".into());
    };
    if on {
        let body = if cfg!(target_os = "macos") {
            launch_agent_plist(exe)
        } else {
            desktop_entry(exe)
        };
        rteams_services::atomic::write(&path, body.as_bytes()).map_err(|e| e.to_string())
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_writes_and_removes_only_under_given_home() {
        let d = tempfile::tempdir().unwrap();
        let exe = Path::new("/opt/r & t/rteams-gui");
        match set_launch_at_login(d.path(), exe, true) {
            Ok(()) => {
                let f = autostart_file(d.path()).unwrap();
                let body = std::fs::read_to_string(&f).unwrap();
                assert!(body.contains("rteams-gui"));
                set_launch_at_login(d.path(), exe, false).unwrap();
                assert!(!f.exists());
            }
            Err(_) => assert!(autostart_file(d.path()).is_none()),
        }
        assert!(set_launch_at_login(d.path(), exe, false).is_ok());
    }

    #[test]
    fn entry_formats_escape_paths() {
        assert!(launch_agent_plist(Path::new("/a&b")).contains("/a&amp;b"));
        assert!(desktop_entry(Path::new("/a b")).contains("Exec=\"/a b\""));
    }
}
