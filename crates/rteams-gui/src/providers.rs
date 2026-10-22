//! Provider management screen: one card per agent runtime with install state, sign-in state,
//! models and how many members use it. Detection spawns processes, so it runs on a background
//! thread (once when the screen first shows, and on Refresh) and the UI only reads the cache.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use crate::icons::Icon;
use crate::installer::{InstallState, Installs};
use crate::provider_cfg::{self, LocalProvider, ModelLimits};
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
    /// Terminal command that signs in (`codex login`), when the CLI has one.
    pub login_command: Option<String>,
    /// Env var of the provider API key (compat providers), when the card should offer Connect.
    pub key_env: Option<String>,
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
                login_command: r.login_command().map(str::to_string),
                key_env: r.api_key_env().map(str::to_string),
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

    pub fn started(&self) -> bool {
        self.started
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

/// Prompt for a model test: tiny, so a test costs a handful of tokens.
const TEST_PROMPT: &str = "Reply with exactly: OK";

/// CLI args that send `TEST_PROMPT` to `model`, for runtimes with a known one-shot mode.
pub fn test_args(runtime: &str, model: &str) -> Option<Vec<String>> {
    let v = |a: &[&str]| Some(a.iter().map(|s| s.to_string()).collect());
    match runtime {
        "claude" => v(&[
            "-p",
            TEST_PROMPT,
            "--model",
            model,
            "--output-format",
            "text",
        ]),
        "codex" => v(&["exec", "-m", model, TEST_PROMPT]),
        "gemini" => v(&["-m", model, "-p", TEST_PROMPT]),
        _ => None,
    }
}

/// The runnable part of an install hint (`npm i -g x   (or: brew ...)` -> `npm i -g x`).
pub fn install_command(hint: &str) -> Option<String> {
    let c = hint.split("(or:").next().unwrap_or(hint).trim();
    (!c.is_empty()).then(|| c.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub enum TestState {
    Running,
    /// Seconds taken and the first line of the reply.
    Ok(f32, String),
    Failed(String),
}

type Tests = Arc<Mutex<std::collections::HashMap<(String, String), TestState>>>;

/// Run a model test on a worker thread; the result lands in `tests`.
fn start_test(ctx: &egui::Context, tests: &Tests, bin: std::path::PathBuf, id: &str, model: &str) {
    let Some(args) = test_args(id, model) else {
        return;
    };
    let key = (id.to_string(), model.to_string());
    if let Ok(mut t) = tests.lock() {
        t.insert(key.clone(), TestState::Running);
    }
    let (tests, ctx) = (tests.clone(), ctx.clone());
    std::thread::spawn(move || {
        let t0 = std::time::Instant::now();
        let out = std::process::Command::new(&bin)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output();
        let secs = t0.elapsed().as_secs_f32();
        let state = match out {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                TestState::Ok(
                    secs,
                    text.lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .trim()
                        .chars()
                        .take(80)
                        .collect(),
                )
            }
            Ok(o) => {
                let err = String::from_utf8_lossy(if o.stderr.is_empty() {
                    &o.stdout
                } else {
                    &o.stderr
                })
                .into_owned();
                TestState::Failed(
                    err.lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("failed")
                        .trim()
                        .chars()
                        .take(160)
                        .collect(),
                )
            }
            Err(e) => TestState::Failed(e.to_string()),
        };
        if let Ok(mut t) = tests.lock() {
            t.insert(key, state);
        }
        ctx.request_repaint();
    });
}

/// (ok, failed, running) over the tests recorded for `models` of runtime `id`.
fn verify_summary(
    t: &std::collections::HashMap<(String, String), TestState>,
    id: &str,
    models: &[ModelInfo],
) -> (usize, usize, usize) {
    let mut r = (0, 0, 0);
    for m in models {
        match t.get(&(id.to_string(), m.id.clone())) {
            Some(TestState::Ok(..)) => r.0 += 1,
            Some(TestState::Failed(_)) => r.1 += 1,
            Some(TestState::Running) => r.2 += 1,
            None => {}
        }
    }
    r
}

#[derive(Default)]
pub struct Providers {
    probe: Probe,
    tests: Tests,
    installs: Installs,
    /// Runtimes whose finished install already triggered a re-probe.
    refreshed: std::collections::HashSet<String>,
    /// Set when "Sign in" is pressed: the command to run in the terminal drawer
    /// (`ShellRequest::OpenTerminal` takes only a cwd, so the shell shows/pastes this text).
    login: Option<String>,
    /// Local server scan (Ollama / LM Studio), refreshed with the probe.
    local: Arc<Mutex<Option<Vec<LocalProvider>>>>,
    local_busy: Arc<Mutex<bool>>,
    /// Runtimes whose sign-in was started in the terminal: show "I've signed in - refresh".
    awaiting: std::collections::HashSet<String>,
    was_focused: bool,
    key_inputs: std::collections::HashMap<String, String>,
    /// env var -> last4 of the stored key (cache of the key store).
    keys: Option<std::collections::HashMap<String, Option<String>>>,
    /// `AppConfig.extra` snapshot for model limits (reloaded after a save / refresh).
    extra: Option<serde_json::Map<String, serde_json::Value>>,
    /// Open limit editors: (runtime, model) -> (context, max output) text.
    edits: std::collections::HashMap<(String, String), (String, String)>,
    key_error: Option<String>,
}

impl Providers {
    fn scan_local(&self, ctx: &egui::Context) {
        {
            let Ok(mut b) = self.local_busy.lock() else {
                return;
            };
            if *b {
                return;
            }
            *b = true;
        }
        let (out, busy, ctx) = (self.local.clone(), self.local_busy.clone(), ctx.clone());
        std::thread::spawn(move || {
            let found = provider_cfg::scan_local();
            if let Ok(mut o) = out.lock() {
                *o = Some(found);
            }
            if let Ok(mut b) = busy.lock() {
                *b = false;
            }
            ctx.request_repaint();
        });
    }

    fn refresh_all(&mut self, ctx: &egui::Context) {
        self.probe.refresh(ctx);
        self.scan_local(ctx);
        self.keys = None;
        self.extra = None;
    }

    /// The sign-in command the user asked for, once.
    #[allow(dead_code)]
    pub fn take_login(&mut self) -> Option<String> {
        self.login.take()
    }
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
        if !self.probe.started() {
            self.scan_local(ui.ctx());
        }
        self.probe.ensure(ui.ctx());
        // Back from the terminal / browser sign-in: re-probe once the window regains focus.
        let focused = ui.input(|i| i.focused);
        if focused && !self.was_focused && !self.awaiting.is_empty() {
            self.refresh_all(ui.ctx());
        }
        self.was_focused = focused;
        let home = provider_cfg::home();
        if self.keys.is_none() {
            self.keys = Some(
                home.as_deref()
                    .map(|h| {
                        rteams_services::ext::apikeys::Store::new(h)
                            .list()
                            .into_iter()
                            .filter(|k| k.project.is_none())
                            .map(|k| (k.env_key, Some(k.last4)))
                            .collect()
                    })
                    .unwrap_or_default(),
            );
        }
        if self.extra.is_none() {
            self.extra = Some(
                home.as_deref()
                    .map(|h| rteams_services::config::AppConfig::load(h).extra)
                    .unwrap_or_default(),
            );
        }
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
                        self.refresh_all(ui.ctx());
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
        // A successful install flips the card to installed: re-probe once per finished install.
        for info in &infos {
            match self.installs.get(&info.id) {
                Some(InstallState::Done { ok: true, .. }) => {
                    if self.refreshed.insert(info.id.clone()) {
                        self.probe.refresh(ui.ctx());
                    }
                }
                Some(InstallState::Running { .. }) => {
                    self.refreshed.remove(&info.id);
                }
                _ => {}
            }
        }
        let local = self.local.lock().ok().and_then(|l| l.clone());
        if let Some(local) = local {
            local_section(ui, &local);
            ui.add_space(10.0);
        }
        let default_id = infos[0].id.clone();
        for info in &infos {
            let used = count_using(members.iter().map(Option::as_deref), &info.id, &default_id);
            let key_last4 = info
                .key_env
                .as_ref()
                .and_then(|e| self.keys.as_ref().and_then(|k| k.get(e).cloned().flatten()));
            let empty = serde_json::Map::new();
            let mut x = CardX {
                key_last4,
                key_input: self.key_inputs.entry(info.id.clone()).or_default(),
                awaiting: self.awaiting.contains(&info.id),
                limits: self.extra.as_ref().unwrap_or(&empty),
                edits: &mut self.edits,
                key_error: self.key_error.as_deref(),
            };
            match card(ui, tr, info, used, &self.tests, &self.installs, &mut x) {
                Some(CardAction::Terminal(cmd)) => {
                    ui.ctx().copy_text(cmd.clone());
                    self.login = Some(cmd);
                    self.awaiting.insert(info.id.clone());
                }
                Some(CardAction::Refresh) => {
                    self.awaiting.remove(&info.id);
                    self.refresh_all(ui.ctx());
                }
                Some(CardAction::Connect(env, value)) => {
                    let r = home
                        .as_deref()
                        .ok_or_else(|| "no home".to_string())
                        .and_then(|h| provider_cfg::connect_key(h, &info.label, &env, &value));
                    self.key_error = r.err();
                    self.key_inputs.remove(&info.id);
                    self.keys = None;
                    self.probe.refresh(ui.ctx());
                }
                Some(CardAction::Forget(env)) => {
                    self.key_error = home
                        .as_deref()
                        .and_then(|h| provider_cfg::forget_key(h, &env).err());
                    self.keys = None;
                    self.probe.refresh(ui.ctx());
                }
                Some(CardAction::SaveLimits(model, l)) => {
                    let _ = provider_cfg::save_limits(&info.id, &model, l);
                    self.edits.remove(&(info.id.clone(), model));
                    self.extra = None;
                }
                Some(CardAction::Install(cmd)) => {
                    if let Some(tool) = crate::installer::missing_tool(&cmd) {
                        self.installs.fail(
                            &info.id,
                            tr.t_args("rteams.providers.installNoTool", &[("tool", tool)]),
                        );
                    } else {
                        self.refreshed.remove(&info.id);
                        self.installs.start(ui.ctx(), &info.id, &cmd);
                    }
                }
                Some(CardAction::Cancel) => self.installs.cancel(&info.id),
                Some(CardAction::Test(model)) => {
                    if let Some(bin) = info.detection.path.clone() {
                        start_test(ui.ctx(), &self.tests, bin, &info.id, &model);
                    }
                }
                Some(CardAction::VerifyAll) => {
                    if let Some(bin) = info.detection.path.clone() {
                        for m in &info.models {
                            start_test(ui.ctx(), &self.tests, bin.clone(), &info.id, &m.id);
                        }
                    }
                }
                None => {}
            }
            ui.add_space(10.0);
        }
    }
}

/// A member's runtime id, if set.
pub fn member_runtime(m: &rteams_core::Member) -> Option<&str> {
    m.runtime.as_deref()
}

enum CardAction {
    /// Run this command in the terminal drawer (sign in / install).
    Terminal(String),
    /// Run this install command in-app (always a runtime's own `install_hint`).
    Install(String),
    /// Stop the running install.
    Cancel,
    /// Send a tiny test prompt to this model.
    Test(String),
    /// Test every listed model (cliInstaller:verifyProviderModels).
    VerifyAll,
    /// Re-probe (after a terminal / browser sign-in).
    Refresh,
    /// Save this API key (env var, value) through the key store.
    Connect(String, String),
    /// Forget the stored key for this env var.
    Forget(String),
    /// Save the limits override for this model.
    SaveLimits(String, ModelLimits),
}

/// Mutable per-card UI state handed in by `Providers`.
struct CardX<'a> {
    key_last4: Option<String>,
    key_input: &'a mut String,
    awaiting: bool,
    limits: &'a serde_json::Map<String, serde_json::Value>,
    edits: &'a mut std::collections::HashMap<(String, String), (String, String)>,
    key_error: Option<&'a str>,
}

/// "Local providers" panel: Ollama / LM Studio reachability and their models.
fn local_section(ui: &mut egui::Ui, local: &[LocalProvider]) {
    egui::Frame::new()
        .fill(theme::RAISED)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.08)))
        .corner_radius(egui::CornerRadius::same(12))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width().max(1.0));
            ui.label(RichText::new("Local providers").size(16.0).strong());
            for p in local {
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new(p.label).strong());
                    if p.running {
                        theme::pill(ui, "Running".to_string(), theme::SUCCESS);
                        theme::pill(ui, format!("{} models", p.models.len()), theme::INFO);
                    } else {
                        theme::pill(ui, "Not running".to_string(), theme::MUTED);
                    }
                    ui.label(RichText::new(p.url).monospace().color(theme::TEXT_2));
                });
                if !p.models.is_empty() {
                    ui.label(RichText::new(p.models.join(", ")).color(theme::TEXT_2));
                }
            }
        });
}

fn card(
    ui: &mut egui::Ui,
    tr: &I18n,
    info: &RuntimeInfo,
    used: usize,
    tests: &Tests,
    installs: &Installs,
    x: &mut CardX,
) -> Option<CardAction> {
    let mut login = None;
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
            if let (Some(cmd), true) = (&info.login_command, info.installed()) {
                if !matches!(info.auth, AuthStatus::SignedIn(_)) {
                    ui.horizontal_wrapped(|ui| {
                        if ui.button(tr.t("rteams.providers.signIn")).clicked() {
                            login = Some(CardAction::Terminal(cmd.clone()));
                        }
                        ui.label(RichText::new(cmd).monospace().color(theme::TEXT_2));
                        if x.awaiting
                            && ui
                                .button("I've signed in \u{2014} refresh")
                                .on_hover_text("Re-check the sign-in status")
                                .clicked()
                        {
                            login = Some(CardAction::Refresh);
                        }
                    });
                }
            }
            if let Some(env) = &info.key_env {
                ui.horizontal_wrapped(|ui| match &x.key_last4 {
                    Some(last4) => {
                        theme::pill(
                            ui,
                            format!("Connected \u{b7}\u{b7}\u{b7}\u{b7}{last4}"),
                            theme::SUCCESS,
                        );
                        if ui.small_button("Forget").clicked() {
                            login = Some(CardAction::Forget(env.clone()));
                        }
                    }
                    None => {
                        ui.label(RichText::new(env).monospace().color(theme::TEXT_2));
                        ui.add(
                            egui::TextEdit::singleline(x.key_input)
                                .password(true)
                                .desired_width(220.0)
                                .hint_text("API key"),
                        );
                        let ok = !x.key_input.trim().is_empty();
                        if ui.add_enabled(ok, egui::Button::new("Connect")).clicked() {
                            login = Some(CardAction::Connect(
                                env.clone(),
                                x.key_input.trim().to_string(),
                            ));
                        }
                    }
                });
                if let Some(e) = x.key_error {
                    ui.label(RichText::new(e).color(theme::ERROR));
                }
            }
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
                            if let Some(cmd) = install_command(&info.install_hint) {
                                if ui
                                    .add_enabled(
                                        !installs.is_running(&info.id),
                                        egui::Button::new(tr.t("rteams.providers.install")),
                                    )
                                    .on_hover_text(&cmd)
                                    .clicked()
                                {
                                    login = Some(CardAction::Install(cmd.clone()));
                                }
                                if ui
                                    .small_button(tr.t("rteams.providers.installInTerminal"))
                                    .clicked()
                                {
                                    login = Some(CardAction::Terminal(cmd));
                                }
                            }
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
                    if let Some(st) = installs.get(&info.id) {
                        if install_log(ui, tr, &info.id, &st) {
                            login = Some(CardAction::Cancel);
                        }
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
                    let can_test = info.installed() && test_args(&info.id, "m").is_some();
                    if can_test {
                        ui.horizontal(|ui| {
                            let (ok, failed, running) = tests
                                .lock()
                                .map(|t| verify_summary(&t, &info.id, &info.models))
                                .unwrap_or_default();
                            if running > 0 {
                                ui.spinner();
                            } else if ui.small_button("Verify all models").clicked() {
                                login = Some(CardAction::VerifyAll);
                            }
                            if ok + failed + running > 0 {
                                ui.label(
                                    RichText::new(format!(
                                        "{ok} ok \u{b7} {failed} failed \u{b7} {running} running"
                                    ))
                                    .color(if failed > 0 {
                                        theme::ERROR
                                    } else {
                                        theme::TEXT_2
                                    }),
                                );
                            }
                        });
                    }
                    for m in &info.models {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&m.label);
                            if m.label != m.id {
                                ui.label(RichText::new(&m.id).monospace().color(theme::MUTED));
                            }
                            let cur = provider_cfg::get_limits(x.limits, &info.id, &m.id);
                            if !cur.is_empty() {
                                theme::pill(ui, limits_text(&cur), theme::INFO);
                            }
                            let ek = (info.id.clone(), m.id.clone());
                            if ui.small_button("Limits").clicked() && x.edits.remove(&ek).is_none()
                            {
                                x.edits.insert(
                                    ek.clone(),
                                    (
                                        cur.context.map(|n| n.to_string()).unwrap_or_default(),
                                        cur.max_output.map(|n| n.to_string()).unwrap_or_default(),
                                    ),
                                );
                            }
                            if let Some((c, o)) = x.edits.get_mut(&ek) {
                                ui.add(
                                    egui::TextEdit::singleline(c)
                                        .desired_width(80.0)
                                        .hint_text("context"),
                                );
                                ui.add(
                                    egui::TextEdit::singleline(o)
                                        .desired_width(80.0)
                                        .hint_text("max output"),
                                );
                                if ui.small_button("Save").clicked() {
                                    let n =
                                        |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n > 0);
                                    login = Some(CardAction::SaveLimits(
                                        m.id.clone(),
                                        ModelLimits {
                                            context: n(c),
                                            max_output: n(o),
                                        },
                                    ));
                                }
                            }
                            if !can_test {
                                return;
                            }
                            let state = tests
                                .lock()
                                .ok()
                                .and_then(|t| t.get(&(info.id.clone(), m.id.clone())).cloned());
                            match state {
                                Some(TestState::Running) => {
                                    ui.spinner();
                                }
                                st => {
                                    if ui.small_button(tr.t("rteams.providers.test")).clicked() {
                                        login = Some(CardAction::Test(m.id.clone()));
                                    }
                                    match st {
                                        Some(TestState::Ok(secs, reply)) => {
                                            ui.label(
                                                RichText::new(format!("{reply} \u{b7} {secs:.1}s"))
                                                    .color(theme::SUCCESS),
                                            );
                                        }
                                        Some(TestState::Failed(e)) => {
                                            ui.label(RichText::new(e).color(theme::ERROR));
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        });
                    }
                });
            }
        });
    login
}

fn limits_text(l: &ModelLimits) -> String {
    let k = |n: u32| {
        if n >= 1000 {
            format!("{}k", n / 1000)
        } else {
            n.to_string()
        }
    };
    match (l.context, l.max_output) {
        (Some(c), Some(o)) => format!("ctx {} \u{b7} out {}", k(c), k(o)),
        (Some(c), None) => format!("ctx {}", k(c)),
        (None, Some(o)) => format!("out {}", k(o)),
        _ => String::new(),
    }
}

/// Collapsible live install log. Returns true when Cancel was pressed.
fn install_log(ui: &mut egui::Ui, tr: &I18n, id: &str, st: &InstallState) -> bool {
    let mut cancel = false;
    let running = matches!(st, InstallState::Running { .. });
    ui.horizontal(|ui| match st {
        InstallState::Running { .. } => {
            ui.spinner();
            ui.label(tr.t("rteams.providers.installing"));
            if ui.small_button(tr.t("common.actions.cancel")).clicked() {
                cancel = true;
            }
        }
        InstallState::Done { ok: true, .. } => {
            ui.label(RichText::new(tr.t("rteams.providers.installOk")).color(theme::SUCCESS));
        }
        InstallState::Done { ok: false, .. } => {
            ui.label(RichText::new(tr.t("rteams.providers.installFailed")).color(theme::ERROR));
        }
    });
    let lines = st.lines();
    if !lines.is_empty() {
        let tail = &lines[lines.len().saturating_sub(200)..];
        egui::CollapsingHeader::new(tr.t("rteams.providers.installLog"))
            .id_salt(("install-log", id))
            .default_open(running || matches!(st, InstallState::Done { ok: false, .. }))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("install-scroll", id))
                    .max_height(180.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                RichText::new(tail.join("\n"))
                                    .monospace()
                                    .color(theme::TEXT_2),
                            )
                            .selectable(true)
                            .wrap(),
                        );
                    });
            });
    }
    cancel
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
    fn test_and_install_commands() {
        assert_eq!(test_args("claude", "opus").unwrap()[0], "-p");
        assert!(test_args("claude", "opus")
            .unwrap()
            .contains(&"opus".to_string()));
        assert_eq!(test_args("codex", "gpt-5").unwrap()[0], "exec");
        assert!(test_args("kiro", "x").is_none());
        assert_eq!(
            install_command("npm install -g @openai/codex   (or: brew install codex)").as_deref(),
            Some("npm install -g @openai/codex")
        );
        assert_eq!(install_command("  ").as_deref(), None);
    }

    #[test]
    fn model_test_runs_on_a_worker() {
        let dir = std::env::temp_dir().join(format!("rteams-mt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("fake-claude");
        std::fs::write(&bin, "#!/bin/sh\necho OK\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tests = Tests::default();
        let ctx = egui::Context::default();
        start_test(&ctx, &tests, bin, "claude", "opus");
        let key = ("claude".to_string(), "opus".to_string());
        for _ in 0..200 {
            if !matches!(tests.lock().unwrap().get(&key), Some(TestState::Running)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(matches!(tests.lock().unwrap().get(&key), Some(TestState::Ok(_, r)) if r == "OK"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn verify_summary_counts_states() {
        let mi = |id: &str| ModelInfo {
            id: id.into(),
            label: id.into(),
        };
        let models = [mi("a"), mi("b"), mi("c"), mi("d")];
        let mut t = std::collections::HashMap::new();
        t.insert(
            ("x".to_string(), "a".to_string()),
            TestState::Ok(1.0, "OK".into()),
        );
        t.insert(
            ("x".to_string(), "b".to_string()),
            TestState::Failed("no".into()),
        );
        t.insert(("x".to_string(), "c".to_string()), TestState::Running);
        t.insert(("y".to_string(), "d".to_string()), TestState::Running);
        assert_eq!(verify_summary(&t, "x", &models), (1, 1, 1));
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
            login_command: Some("x login".into()),
            key_env: Some("X_API_KEY".into()),
        };
        let mut input = String::new();
        let mut edits = std::collections::HashMap::new();
        let mut extra = serde_json::Map::new();
        provider_cfg::set_limits(
            &mut extra,
            "x",
            "m",
            ModelLimits {
                context: Some(64_000),
                max_output: None,
            },
        );
        edits.insert(("x".to_string(), "m".to_string()), ("1".into(), "2".into()));
        let mut x = CardX {
            key_last4: None,
            key_input: &mut input,
            awaiting: true,
            limits: &extra,
            edits: &mut edits,
            key_error: Some("boom"),
        };
        let tr = crate::view::testing::tr();
        render(|ui| {
            card(
                ui,
                tr,
                &info(Some("/bin/x")),
                1,
                &Tests::default(),
                &Installs::default(),
                &mut x,
            );
            card(
                ui,
                tr,
                &info(None),
                0,
                &Tests::default(),
                &Installs::default(),
                &mut x,
            );
        });
        x.key_last4 = Some("1234".into());
        render(|ui| {
            card(
                ui,
                tr,
                &info(Some("/bin/x")),
                1,
                &Tests::default(),
                &Installs::default(),
                &mut x,
            );
        });
    }

    #[test]
    fn local_section_renders_models() {
        let local = vec![
            LocalProvider {
                id: "ollama",
                label: "Ollama",
                url: "http://127.0.0.1:11434",
                running: true,
                models: provider_cfg::parse_ollama(r#"{"models":[{"name":"llama3:8b"}]}"#),
            },
            LocalProvider {
                id: "lmstudio",
                label: "LM Studio",
                url: "http://127.0.0.1:1234",
                running: false,
                models: vec![],
            },
        ];
        render(|ui| local_section(ui, &local));
        assert_eq!(local[0].models, ["llama3:8b"]);
        assert_eq!(
            limits_text(&ModelLimits {
                context: Some(128_000),
                max_output: Some(8_000)
            }),
            "ctx 128k \u{b7} out 8k"
        );
    }
}
