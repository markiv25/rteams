//! Advanced: about, browser access (HTTP server), automatic recovery, configuration
//! (edit / export / import / open / reset).

use std::hash::{BuildHasher, Hasher};
use std::path::Path;

use eframe::egui;
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;

use super::config_editor::ConfigEditor;
use super::{card, hint, toggle};
use crate::theme;

#[derive(Default)]
pub struct State {
    pub editor: ConfigEditor,
    pub confirm_reset: bool,
    pub message: Option<(bool, String)>,
}

/// 64 hex chars from OS-seeded hashers (`RandomState`), enough for a local access token.
pub fn generate_token() -> String {
    let mut out = String::new();
    for i in 0..4u64 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(i);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

pub fn show(
    ui: &mut egui::Ui,
    cfg: &mut AppConfig,
    st: &mut State,
    tr: &I18n,
    home: &Path,
) -> bool {
    let mut changed = false;

    card(ui, &tr.t("settings.advanced.about.title"), |ui| {
        ui.label(tr.t_args(
            "settings.advanced.about.version",
            &[("version", env!("CARGO_PKG_VERSION"))],
        ));
        hint(ui, &tr.t("settings.advanced.about.description"));
    });

    card(ui, &tr.t("settings.general.browserAccess.title"), |ui| {
        changed |= toggle(
            ui,
            &mut cfg.general.server_enabled,
            &tr.t("settings.general.browserAccess.serverMode.label"),
            &tr.t("settings.general.browserAccess.serverMode.description"),
        );
        if cfg.general.server_enabled {
            ui.horizontal(|ui| {
                ui.label(tr.t("settings.general.server.runningOn"));
                changed |= ui
                    .add(egui::DragValue::new(&mut cfg.general.server_port).range(1024..=65535))
                    .changed();
                ui.monospace(format!("http://127.0.0.1:{}", cfg.general.server_port));
            });
            ui.horizontal(|ui| {
                ui.label("Token");
                ui.monospace(if cfg.general.server_token.is_empty() {
                    "-".to_string()
                } else {
                    mask(&cfg.general.server_token)
                });
                if ui.button("Copy").clicked() && !cfg.general.server_token.is_empty() {
                    ui.ctx().copy_text(cfg.general.server_token.clone());
                }
                let label = if cfg.general.server_token.is_empty() {
                    "Generate"
                } else {
                    "Regenerate"
                };
                if ui.button(label).clicked() {
                    cfg.general.server_token = generate_token();
                    changed = true;
                }
            });
        }
    });

    card(ui, &tr.t("rteams.advanced.feeds.title"), |ui| {
        for (key, label_key) in [
            ("update_feed_url", "rteams.advanced.feeds.updateUrl"),
            (
                "announcements_feed_url",
                "rteams.advanced.feeds.announcementsUrl",
            ),
        ] {
            let mut url = cfg
                .general
                .extra
                .get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ui.label(tr.t(label_key));
            let edited = ui
                .add(
                    egui::TextEdit::singleline(&mut url)
                        .hint_text("https://")
                        .desired_width(f32::INFINITY),
                )
                .changed();
            if edited {
                let url = url.trim().to_string();
                if url.is_empty() {
                    cfg.general.extra.remove(key);
                } else {
                    cfg.general.extra.insert(key.to_string(), url.into());
                }
                changed = true;
            }
        }
        hint(ui, &tr.t("rteams.advanced.feeds.hint"));
    });

    card(ui, &tr.t("settings.notifications.recovery.title"), |ui| {
        changed |= crate::ui::recovery_settings::show_tr(ui, cfg, tr);
    });

    card(ui, &tr.t("settings.advanced.configuration.title"), |ui| {
        ui.horizontal_wrapped(|ui| {
            if ui
                .button(tr.t("settings.advanced.configuration.editConfig"))
                .clicked()
            {
                if st.editor.open {
                    st.editor.open = false;
                } else {
                    st.editor.open(cfg);
                }
            }
            if ui
                .button(tr.t("settings.advanced.configuration.exportConfig"))
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .set_file_name("rteams-config.json")
                    .save_file()
                {
                    st.message = Some(match std::fs::write(&p, cfg.export()) {
                        Ok(()) => (true, p.display().to_string()),
                        Err(e) => (false, e.to_string()),
                    });
                }
            }
            if ui
                .button(tr.t("settings.advanced.configuration.importConfig"))
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
                {
                    match std::fs::read_to_string(&p)
                        .map_err(|e| e.to_string())
                        .and_then(|t| AppConfig::import(&t))
                    {
                        Ok(new) => {
                            *cfg = new;
                            changed = true;
                            st.editor.open = false;
                            st.message = Some((true, p.display().to_string()));
                        }
                        Err(e) => st.message = Some((false, e)),
                    }
                }
            }
            if ui
                .button(tr.t("settings.advanced.configuration.openInEditor"))
                .clicked()
            {
                let path = AppConfig::path(home);
                if !path.exists() {
                    let _ = cfg.save(home);
                }
                if let Err(e) = open::that(&path) {
                    st.message = Some((false, e.to_string()));
                }
            }
            if ui
                .button(tr.t("settings.advanced.configuration.resetToDefaults"))
                .clicked()
            {
                st.confirm_reset = true;
            }
        });
        if st.confirm_reset {
            ui.colored_label(
                theme::WARNING,
                tr.t("settings.advanced.configuration.resetToDefaults"),
            );
            ui.horizontal(|ui| {
                if ui
                    .button(tr.t("settings.workspaceProfiles.deleteConfirm.confirmLabel"))
                    .clicked()
                {
                    *cfg = AppConfig::default();
                    st.editor.open = false;
                    st.confirm_reset = false;
                    changed = true;
                }
                if ui
                    .button(tr.t("settings.workspaceProfiles.actions.cancel"))
                    .clicked()
                {
                    st.confirm_reset = false;
                }
            });
        }
        if let Some((ok, m)) = &st.message {
            ui.colored_label(if *ok { theme::SUCCESS } else { theme::ERROR }, m);
        }
        changed |= st.editor.show(ui, cfg, tr);
    });
    changed
}

fn mask(t: &str) -> String {
    if t.len() <= 8 {
        "*".repeat(t.len())
    } else {
        format!("{}...{}", &t[..4], &t[t.len() - 4..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_hex() {
        let (a, b) = (generate_token(), generate_token());
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn masking() {
        assert_eq!(mask("abcd"), "****");
        assert_eq!(mask("0123456789abcdef"), "0123...cdef");
    }
}
