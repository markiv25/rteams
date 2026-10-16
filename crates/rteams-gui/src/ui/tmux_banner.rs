//! tmux_banner: info banner shown when tmux is missing. It never installs anything by itself:
//! "Show command" reveals the exact command, "Install" asks for confirmation, and only the
//! confirm click runs it. Dismissal is persisted in `<home>/config.json` (free-form key).

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use eframe::egui::{self, RichText};

use super::close_guard::tx;
use super::FeatureCtx;
use crate::theme;
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use rteams_services::tmux::{self, InstallEvent, InstallRun};

const DISMISS_KEY: &str = "tmux_banner_dismissed";
const LOG_CAP: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Unknown,
    Checking,
    Present(String),
    Missing,
    Installing,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Act {
    ShowCmd,
    AskInstall,
    ConfirmInstall,
    CancelConfirm,
    Dismiss,
    Recheck,
}

pub struct TmuxBanner {
    pub(crate) phase: Phase,
    pub(crate) dismissed: Option<bool>,
    pub(crate) cmd: Option<Vec<String>>,
    show_cmd: bool,
    confirm: bool,
    log: Vec<String>,
    probe: Option<Receiver<Option<String>>>,
    run: Option<InstallRun>,
}

impl Default for TmuxBanner {
    fn default() -> Self {
        TmuxBanner {
            phase: Phase::Unknown,
            dismissed: None,
            cmd: None,
            show_cmd: false,
            confirm: false,
            log: vec![],
            probe: None,
            run: None,
        }
    }
}

pub fn is_dismissed(home: &Path) -> bool {
    AppConfig::load(home)
        .extra
        .get(DISMISS_KEY)
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub fn set_dismissed(home: &Path, v: bool) -> std::io::Result<()> {
    let mut c = AppConfig::load(home);
    c.extra
        .insert(DISMISS_KEY.into(), serde_json::Value::Bool(v));
    c.save(home)
}

impl TmuxBanner {
    fn start_probe(&mut self, ctx: &egui::Context) {
        let (s, r) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = s.send(tmux::detect_tmux());
            ctx.request_repaint();
        });
        self.probe = Some(r);
        self.phase = Phase::Checking;
        self.cmd = tmux::install_command();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(r) = &self.probe {
            if let Ok(v) = r.try_recv() {
                self.phase = v.map_or(Phase::Missing, Phase::Present);
                self.probe = None;
            }
        }
        let mut finished = None;
        if let Some(run) = &self.run {
            for e in run.poll() {
                match e {
                    InstallEvent::Line(l) => {
                        self.log.push(l);
                        if self.log.len() > LOG_CAP {
                            self.log.remove(0);
                        }
                    }
                    InstallEvent::Done(r) => finished = Some(r),
                }
            }
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        if let Some(r) = finished {
            self.run = None;
            match r {
                Ok(()) => self.start_probe(ctx),
                Err(e) => self.phase = Phase::Failed(e),
            }
        }
    }

    /// Draw the banner when tmux is missing and not dismissed. Renders nothing otherwise.
    pub fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        if self.dismissed.is_none() {
            self.dismissed = Some(is_dismissed(c.home));
        }
        if self.dismissed == Some(true) {
            return;
        }
        if self.phase == Phase::Unknown {
            self.start_probe(ui.ctx());
        }
        self.poll(ui.ctx());
        if let Some(act) = self.render(ui, c.tr) {
            self.apply(act, ui.ctx(), c);
        }
    }

    fn apply(&mut self, act: Act, ctx: &egui::Context, c: &mut FeatureCtx) {
        match act {
            Act::ShowCmd => self.show_cmd = !self.show_cmd,
            Act::AskInstall => {
                self.show_cmd = true;
                self.confirm = true;
            }
            Act::CancelConfirm => self.confirm = false,
            Act::ConfirmInstall => {
                self.confirm = false;
                if let Some(cmd) = &self.cmd {
                    self.log.clear();
                    match tmux::run(cmd) {
                        Ok(r) => {
                            self.run = Some(r);
                            self.phase = Phase::Installing;
                        }
                        Err(e) => self.phase = Phase::Failed(e),
                    }
                }
            }
            Act::Dismiss => {
                self.dismissed = Some(true);
                if let Err(e) = set_dismissed(c.home, true) {
                    c.req.push(super::ShellRequest::Toast(format!(
                        "Could not save dismissal: {e}"
                    )));
                }
            }
            Act::Recheck => self.start_probe(ctx),
        }
    }

    /// Pure drawing; returns the clicked action.
    fn render(&mut self, ui: &mut egui::Ui, tr: &I18n) -> Option<Act> {
        if !matches!(
            self.phase,
            Phase::Missing | Phase::Installing | Phase::Failed(_)
        ) {
            return None;
        }
        let mut act = None;
        egui::Frame::new()
            .fill(theme::tint(theme::INFO, 0.10))
            .stroke(egui::Stroke::new(1.0, theme::tint(theme::INFO, 0.4)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(tx(
                        tr,
                        "tmuxBanner.title",
                        "tmux not found \u{2014} agent terminals can't persist",
                    ))
                    .strong(),
                );
                let installing = self.phase == Phase::Installing;
                let cmd_text = self.cmd.as_deref().map(tmux::display);
                if self.show_cmd {
                    match &cmd_text {
                        Some(t) => {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(tx(
                                    tr,
                                    "tmuxBanner.commandLabel",
                                    "This command will run:",
                                ))
                                .color(theme::TEXT_2),
                            );
                            ui.label(
                                RichText::new(t)
                                    .monospace()
                                    .background_color(theme::CODE_BG),
                            );
                        }
                        None => {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(tx(
                                    tr,
                                    "tmuxBanner.noStrategy",
                                    "No supported package manager found. Install tmux manually.",
                                ))
                                .color(theme::TEXT_2),
                            );
                            ui.hyperlink_to(
                                tx(
                                    tr,
                                    "common.tmuxInstaller.actions.manualGuide",
                                    "Manual guide",
                                ),
                                tmux::strategy::MANUAL_GUIDE_URL,
                            );
                        }
                    }
                }
                if let Phase::Failed(e) = &self.phase {
                    ui.add_space(4.0);
                    ui.label(RichText::new(e).color(theme::ERROR_FG));
                    ui.label(
                        RichText::new(tx(
                            tr,
                            "tmuxBanner.failedHint",
                            "If it asked for a password, run the command in a terminal instead.",
                        ))
                        .color(theme::MUTED),
                    );
                }
                if !self.log.is_empty() {
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for l in &self.log {
                                ui.label(
                                    RichText::new(l).monospace().size(11.0).color(theme::TEXT_2),
                                );
                            }
                        });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if self.confirm {
                        if theme::solid(
                            ui,
                            &tx(tr, "tmuxBanner.confirmRun", "Run this command"),
                            theme::ACCENT,
                        )
                        .clicked()
                        {
                            act = Some(Act::ConfirmInstall);
                        }
                        if ui
                            .button(tx(tr, "common.actions.cancel", "Cancel"))
                            .clicked()
                        {
                            act = Some(Act::CancelConfirm);
                        }
                        return;
                    }
                    if installing {
                        ui.spinner();
                        ui.label(tx(tr, "tmuxBanner.installing", "Installing\u{2026}"));
                        return;
                    }
                    if ui
                        .button(tx(tr, "tmuxBanner.showCommand", "Show command"))
                        .clicked()
                    {
                        act = Some(Act::ShowCmd);
                    }
                    if self.cmd.is_some()
                        && theme::solid(ui, &tx(tr, "tmuxBanner.install", "Install"), theme::ACCENT)
                            .clicked()
                    {
                        act = Some(Act::AskInstall);
                    }
                    if ui
                        .button(tx(tr, "common.tmuxInstaller.actions.recheck", "Re-check"))
                        .clicked()
                    {
                        act = Some(Act::Recheck);
                    }
                    if theme::ghost(ui, &tx(tr, "tmuxBanner.dismiss", "Dismiss")).clicked() {
                        act = Some(Act::Dismiss);
                    }
                });
            });
        act
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_once(b: &mut TmuxBanner) -> Option<Act> {
        let ctx = egui::Context::default();
        let tr = I18n::english();
        let mut out = None;
        crate::ui::close_guard::headless(&ctx, |ui| {
            out = b.render(ui, &tr);
        });
        out
    }

    #[test]
    fn renders_only_when_missing_and_never_runs() {
        let mut b = TmuxBanner {
            phase: Phase::Present("3.4".into()),
            ..TmuxBanner::default()
        };
        assert!(render_once(&mut b).is_none());
        b.phase = Phase::Missing;
        b.cmd = Some(vec!["brew".into(), "install".into(), "tmux".into()]);
        b.show_cmd = true;
        assert!(render_once(&mut b).is_none()); // no click happened
        assert!(b.run.is_none(), "rendering must not start an install");
    }

    #[test]
    fn dismissal_persists_in_config_without_touching_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_dismissed(dir.path()));
        set_dismissed(dir.path(), true).unwrap();
        assert!(is_dismissed(dir.path()));
        let c = AppConfig::load(dir.path());
        assert_eq!(c.schema_version, rteams_services::config::CURRENT_SCHEMA);
    }
}
