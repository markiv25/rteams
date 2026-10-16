//! Provider management screen: one card per agent runtime with install state, sign-in state,
//! models and how many members use it. Detection spawns processes, so it runs on a background
//! thread (once when the screen first shows, and on Refresh) and the UI only reads the cache.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use crate::icons::Icon;
use crate::runtimes::{self, AuthStatus, Detection, ModelInfo};
use crate::theme;
use crate::ui::dashboard::kit;
use crate::view::View;
use rteams_services::i18n::I18n;

/// Everything shown about one runtime.
#[derive(Debug, Clone)]
pub struct RuntimeInfo {
    pub id: String,
    pub label: String,
    pub detection: Detection,
    pub auth: AuthStatus,
    pub models: Vec<ModelInfo>,
    pub install_hint: String,
}

impl RuntimeInfo {
    pub fn installed(&self) -> bool {
        self.detection.path.is_some()
    }
}

/// Probe every runtime (blocking: spawns processes). Call from a worker thread only.
pub fn probe_all() -> Vec<RuntimeInfo> {
    runtimes::all()
        .iter()
        .map(|r| {
            let detection = r.detect();
            let installed = detection.path.is_some();
            RuntimeInfo {
                id: r.id().to_string(),
                label: r.label().to_string(),
                auth: if installed {
                    r.auth_status()
                } else {
                    AuthStatus::Unknown
                },
                models: if installed { r.models() } else { Vec::new() },
                install_hint: r.install_hint().to_string(),
                detection,
            }
        })
        .collect()
}

/// Background-probed, cached runtime info. Never blocks the caller.
#[derive(Clone, Default, Debug)]
pub struct Probe {
    cache: Arc<Mutex<Option<Vec<RuntimeInfo>>>>,
    busy: Arc<Mutex<bool>>,
    started: bool,
}

impl Probe {
    /// Start the first probe if none has started yet.
    pub fn ensure(&mut self, ctx: &egui::Context) {
        if !self.started {
            self.started = true;
            self.refresh(ctx);
        }
    }

    /// Re-probe in the background; the old results stay visible until the new ones land.
    pub fn refresh(&mut self, ctx: &egui::Context) {
        self.started = true;
        {
            let Ok(mut busy) = self.busy.lock() else {
                return;
            };
            if *busy {
                return;
            }
            *busy = true;
        }
        let (cache, busy, ctx) = (self.cache.clone(), self.busy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let infos = probe_all();
            if let Ok(mut c) = cache.lock() {
                *c = Some(infos);
            }
            if let Ok(mut b) = busy.lock() {
                *b = false;
            }
            ctx.request_repaint();
        });
    }

    pub fn is_busy(&self) -> bool {
        self.busy.lock().map(|b| *b).unwrap_or(false)
    }

    /// Latest results, `None` until the first probe finishes.
    pub fn get(&self) -> Option<Vec<RuntimeInfo>> {
        self.cache.lock().ok().and_then(|c| c.clone())
    }
}

/// How many members run on `runtime` (a member without a runtime uses `default_id`).
pub fn count_using<'a>(
    member_runtimes: impl IntoIterator<Item = Option<&'a str>>,
    runtime: &str,
    default_id: &str,
) -> usize {
    member_runtimes
        .into_iter()
        .filter(|r| r.unwrap_or(default_id) == runtime)
        .count()
}

/// "Signed in as X" / "Signed out" / "Unknown" with a color.
pub fn auth_chip(tr: &I18n, a: &AuthStatus) -> (String, egui::Color32) {
    match a {
        AuthStatus::SignedIn(Some(who)) => (
            tr.t_args("rteams.providers.signedInAs", &[("who", who)]),
            theme::SUCCESS,
        ),
        AuthStatus::SignedIn(None) => (tr.t("rteams.providers.signedIn"), theme::SUCCESS),
        AuthStatus::SignedOut => (tr.t("rteams.providers.signedOut"), theme::WARNING),
        AuthStatus::Unknown => (tr.t("common.states.unknown"), theme::MUTED),
    }
}

#[derive(Default)]
pub struct Providers {
    probe: Probe,
}

impl Providers {
    pub fn show(&mut self, ui: &mut egui::Ui, v: &View) {
        let members: Vec<Option<String>> = v
            .agent_members()
            .iter()
            .map(|m| member_runtime(m).map(str::to_string))
            .collect();
        self.show_section(ui, v.tr, &members);
    }

    /// The runtimes list without needing an open team: `members` are the runtime ids of the
    /// team's agent members (`None` = default). Used by the Settings "Providers" section.
    pub fn show_section(&mut self, ui: &mut egui::Ui, tr: &I18n, members: &[Option<String>]) {
        self.probe.ensure(ui.ctx());
        let infos = self.probe.get();
        let busy = self.probe.is_busy();
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(tr.t("settings.runtimeProvider.tabs.providers"))
                    .size(18.0)
                    .strong(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if theme::pill_icon_button(
                        ui,
                        Some(Icon::Refresh),
                        &tr.t("common.actions.refresh"),
                        false,
                    )
                    .clicked()
                    {
                        self.probe.refresh(ui.ctx());
                    }
                });
                if busy {
                    ui.label(RichText::new(tr.t("rteams.providers.detecting")).color(theme::MUTED));
                }
            });
        });
        ui.add_space(10.0);
        let Some(infos) = infos.filter(|i| !i.is_empty()) else {
            if busy {
                // First probe still running: skeleton cards, never a blank page.
                for _ in 0..3 {
                    let w = ui.available_width();
                    kit::skeleton_tiles(ui, 1, w, 96.0, "");
                }
            } else {
                kit::empty_state(
                    ui,
                    Icon::Cpu,
                    &tr.t("rteams.providers.none"),
                    "Install a CLI runtime (for example Claude Code) and press Refresh.",
                );
            }
            return;
        };
        let default_id = infos[0].id.clone();
        for info in &infos {
            let used = count_using(members.iter().map(Option::as_deref), &info.id, &default_id);
            card(ui, tr, info, used);
            ui.add_space(10.0);
        }
    }
}

/// A member's runtime id, if set.
pub fn member_runtime(m: &rteams_core::Member) -> Option<&str> {
    m.runtime.as_deref()
}

fn card(ui: &mut egui::Ui, tr: &I18n, info: &RuntimeInfo, used: usize) {
    egui::Frame::new()
        .fill(theme::RAISED)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.08)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(1.0));
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(&info.label).size(16.0).strong());
                let (text, color) = auth_chip(tr, &info.auth);
                theme::pill(ui, text, color);
                let n = tr.t_count("rteams.providers.members", used as u64);
                theme::pill(ui, n, theme::INFO);
            });
            ui.add_space(6.0);
            match (&info.detection.path, &info.detection.version) {
                (Some(p), v) => {
                    let ver = v
                        .as_deref()
                        .map(|v| format!(" \u{b7} {v}"))
                        .unwrap_or_default();
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{}{ver}", p.display()))
                                .monospace()
                                .color(theme::TEXT_2),
                        )
                        .wrap(),
                    );
                }
                (None, _) => {
                    ui.label(
                        RichText::new(tr.t("rteams.providers.notInstalled")).color(theme::WARNING),
                    );
                    if !info.install_hint.is_empty() {
                        ui.horizontal_wrapped(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&info.install_hint)
                                        .monospace()
                                        .color(theme::TEXT_2),
                                )
                                .wrap(),
                            );
                            if ui
                                .small_button(tr.t("team.terminalWorkspace.copy"))
                                .clicked()
                            {
                                ui.ctx().copy_text(info.install_hint.clone());
                            }
                        });
                    }
                }
            }
            if !info.models.is_empty() {
                egui::CollapsingHeader::new(format!(
                    "{} ({})",
                    tr.t("settings.runtimeProvider.tabs.models"),
                    info.models.len()
                ))
                .id_salt(("models", &info.id))
                .show(ui, |ui| {
                    for m in &info.models {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&m.label);
                            if m.label != m.id {
                                ui.label(RichText::new(&m.id).monospace().color(theme::MUTED));
                            }
                        });
                    }
                });
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};
    use crate::view::View;

    #[test]
    fn counts_default_runtime() {
        let rs = [None, Some("codex"), Some("claude"), None];
        assert_eq!(count_using(rs, "claude", "claude"), 3);
        assert_eq!(count_using(rs, "codex", "claude"), 1);
        assert_eq!(count_using(rs, "other", "claude"), 0);
    }

    #[test]
    fn auth_chips() {
        assert_eq!(
            auth_chip(
                crate::view::testing::tr(),
                &AuthStatus::SignedIn(Some("a@b".into()))
            )
            .0,
            "Signed in as a@b"
        );
        assert_eq!(
            auth_chip(crate::view::testing::tr(), &AuthStatus::SignedOut).0,
            "Signed out"
        );
        assert_eq!(
            auth_chip(crate::view::testing::tr(), &AuthStatus::Unknown).0,
            "Unknown"
        );
    }

    #[test]
    fn renders_headless() {
        let (home, data) = fixture("providers");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut p = Providers::default();
        render(|ui| p.show(ui, &view));
        // Let the background probe finish, then draw the cards.
        for _ in 0..200 {
            if !p.probe.is_busy() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        render(|ui| p.show(ui, &view));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn card_renders_installed_and_missing() {
        let info = |path: Option<&str>| RuntimeInfo {
            id: "x".into(),
            label: "X".into(),
            detection: Detection {
                path: path.map(Into::into),
                version: Some("1.0".into()),
            },
            auth: AuthStatus::SignedOut,
            models: vec![ModelInfo {
                id: "m".into(),
                label: "M".into(),
            }],
            install_hint: "npm i -g x".into(),
        };
        render(|ui| {
            card(ui, crate::view::testing::tr(), &info(Some("/bin/x")), 1);
            card(ui, crate::view::testing::tr(), &info(None), 0);
        });
    }
}
