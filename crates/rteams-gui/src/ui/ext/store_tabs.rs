//! The Extensions screen: header (title, `Add Custom`, `Open Dashboard`, `Refresh catalog`), a
//! runtime status banner, the "running sessions won't pick up changes" warning and the store
//! tabs `MCP Servers | Plugins | Skills | API Keys`. Also the small helpers the panels share
//! (translation fallbacks, background tasks, key/value editors).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText};
use rteams_services::ext::cli::{self, RuntimeState};
use rteams_services::ext::mcp_config::ClientPaths;
use rteams_services::i18n::I18n;

use super::apikeys_panel::ApiKeysPanel;
use super::mcp_panel::McpPanel;
use super::plugins_panel::PluginsPanel;
use super::skills_panel::SkillsPanel;
use crate::theme;
use crate::ui::{FeatureCtx, Screen, ShellRequest, TabId};

// ---------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------

/// Translation with an English fallback for keys a locale lacks.
pub fn tx(tr: &I18n, key: &str, fallback: &str) -> String {
    if tr.has(key) {
        tr.t(key)
    } else {
        fallback.to_string()
    }
}

/// `tx` with `{{name}}` placeholders.
pub fn txa(tr: &I18n, key: &str, fallback: &str, args: &[(&str, &str)]) -> String {
    if tr.has(key) {
        tr.t_args(key, args)
    } else {
        let mut s = fallback.to_string();
        for (k, v) in args {
            s = s.replace(&format!("{{{{{k}}}}}"), v);
        }
        s
    }
}

/// A value computed on a worker thread and picked up by the UI thread.
pub struct Task<T> {
    slot: Arc<Mutex<Option<T>>>,
    running: Arc<AtomicBool>,
}

impl<T> Default for Task<T> {
    fn default() -> Self {
        Task {
            slot: Arc::new(Mutex::new(None)),
            running: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<T: Send + 'static> Task<T> {
    /// Run `f` on a thread; ignored while a previous run is still going.
    pub fn start(&self, ctx: &egui::Context, f: impl FnOnce() -> T + Send + 'static) {
        if self.running.swap(true, Ordering::SeqCst) {
            return;
        }
        let (slot, running, ctx) = (self.slot.clone(), self.running.clone(), ctx.clone());
        std::thread::spawn(move || {
            let v = f();
            if let Ok(mut s) = slot.lock() {
                *s = Some(v);
            }
            running.store(false, Ordering::SeqCst);
            ctx.request_repaint();
        });
    }

    /// The finished value, once.
    pub fn take(&self) -> Option<T> {
        self.slot.lock().ok().and_then(|mut s| s.take())
    }

    pub fn busy(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

/// What every panel gets each frame.
pub struct ExtCtx<'a> {
    pub home: &'a Path,
    pub paths: &'a ClientPaths,
    /// Never touch the network or the `claude` binary (tests, `--offline`).
    pub offline: bool,
    pub tr: &'a I18n,
    pub req: &'a mut Vec<ShellRequest>,
}

impl ExtCtx<'_> {
    pub fn toast(&mut self, msg: impl Into<String>) {
        self.req.push(ShellRequest::Toast(msg.into()));
    }

    pub fn project(&self) -> Option<&Path> {
        self.paths.project.as_deref()
    }
}

/// Flat bordered card.
pub fn card<R>(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui) -> R) -> egui::Response {
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            body(ui);
        })
        .response
}

/// Small status dot drawn with the painter (no font dependency).
pub fn dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), egui::Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

/// Editable list of `(key, value)` rows with add / remove. `secret` masks the values.
pub fn kv_editor(
    ui: &mut egui::Ui,
    salt: &str,
    rows: &mut Vec<(String, String)>,
    key_hint: &str,
    value_hint: &str,
    secret: bool,
) {
    let mut remove = None;
    for (i, (k, v)) in rows.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.push_id((salt, i), |ui| {
                ui.add(
                    egui::TextEdit::singleline(k)
                        .hint_text(key_hint)
                        .desired_width(170.0),
                );
                ui.add(
                    egui::TextEdit::singleline(v)
                        .hint_text(value_hint)
                        .password(secret)
                        .desired_width(220.0),
                );
                if theme::ghost(ui, "x").clicked() {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        rows.remove(i);
    }
    if theme::ghost(ui, "+ Add").clicked() {
        rows.push((String::new(), String::new()));
    }
}

/// Non-empty rows as an ordered map.
pub fn rows_to_map(rows: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    rows.iter()
        .filter(|(k, _)| !k.trim().is_empty())
        .map(|(k, v)| (k.trim().to_string(), v.clone()))
        .collect()
}

pub fn notice_line(ui: &mut egui::Ui, n: &Option<(bool, String)>) {
    if let Some((ok, msg)) = n {
        ui.colored_label(if *ok { theme::SUCCESS } else { theme::ERROR }, msg);
    }
}

pub fn big_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ---------------------------------------------------------------------------------------------
// The screen
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreTab {
    #[default]
    Mcp,
    Plugins,
    Skills,
    ApiKeys,
}

pub struct ExtensionsScreen {
    pub tab: StoreTab,
    runtime: Option<RuntimeState>,
    runtime_task: Task<RuntimeState>,
    pub mcp: McpPanel,
    pub plugins: PluginsPanel,
    pub skills: SkillsPanel,
    pub keys: ApiKeysPanel,
    /// Fixed config locations (tests); `None` = the real home plus the open team's project.
    paths_override: Option<ClientPaths>,
    offline: bool,
}

impl Default for ExtensionsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl ExtensionsScreen {
    pub fn new() -> Self {
        ExtensionsScreen {
            tab: StoreTab::Mcp,
            runtime: None,
            runtime_task: Task::default(),
            mcp: McpPanel::default(),
            plugins: PluginsPanel::default(),
            skills: SkillsPanel::default(),
            keys: ApiKeysPanel::default(),
            paths_override: None,
            offline: false,
        }
    }

    /// Screen that reads and writes only under `paths`, never uses the network or the real
    /// `claude`, and scans skills only under `skills_home` / `project`.
    #[cfg(test)]
    pub fn sandboxed(paths: ClientPaths) -> Self {
        let mut s = Self::new();
        s.skills = SkillsPanel::with_roots(Some(paths.user_home.clone()), paths.project.clone());
        s.paths_override = Some(paths);
        s.offline = true;
        s
    }

    fn paths(&self, c: &FeatureCtx) -> ClientPaths {
        if let Some(p) = &self.paths_override {
            return p.clone();
        }
        let project: Option<PathBuf> = c
            .view
            .and_then(|v| v.team())
            .and_then(|t| t.project_path.as_deref())
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from);
        ClientPaths::real(project)
    }

    fn refresh_runtime(&self, ctx: &egui::Context) {
        if !self.offline {
            self.runtime_task.start(ctx, cli::probe_claude);
        }
    }

    fn banner(&self, ui: &mut egui::Ui, tr: &I18n) {
        let (color, title, desc) = match &self.runtime {
            None => (
                theme::INFO,
                tx(
                    tr,
                    "extensions.store.runtime.checkingAvailabilityTitle",
                    "Checking extensions runtime availability",
                ),
                tx(
                    tr,
                    "extensions.store.runtime.checkingAvailabilityDescription",
                    "Extensions need the configured runtime to manage plugins, MCP servers, skills, and provider connections.",
                ),
            ),
            Some(RuntimeState::Missing) => (
                theme::WARNING,
                tx(
                    tr,
                    "extensions.store.runtime.notAvailableTitle",
                    "The configured runtime is not available",
                ),
                tx(
                    tr,
                    "extensions.store.runtime.notAvailableDescription",
                    "Extensions are disabled until the runtime is installed. Open the Dashboard to install it and retry.",
                ),
            ),
            Some(RuntimeState::FailedToStart(e)) => (
                theme::ERROR,
                tx(
                    tr,
                    "extensions.store.runtime.failedToStartTitle",
                    "The configured runtime was found but failed to start",
                ),
                e.clone(),
            ),
            Some(RuntimeState::Ready { version, .. }) => (
                theme::SUCCESS,
                txa(
                    tr,
                    "extensions.store.runtime.readyTitle",
                    "{{runtime}} is ready",
                    &[("runtime", "Claude Code")],
                ),
                txa(
                    tr,
                    "extensions.store.runtime.readyDescription",
                    "Plugins can be installed from this page{{versionSuffix}}.",
                    &[(
                        "versionSuffix",
                        &version
                            .as_ref()
                            .map(|v| format!(" ({v})"))
                            .unwrap_or_default(),
                    )],
                ),
            ),
        };
        egui::Frame::new()
            .fill(theme::tint(color, 0.08))
            .stroke(egui::Stroke::new(1.0, theme::tint(color, 0.3)))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    dot(ui, color);
                    ui.label(RichText::new(title).strong());
                    ui.label(RichText::new(desc).small().color(theme::TEXT_2));
                });
            });
    }

    /// True when plugin actions can run (the runtime is ready or we cannot tell yet).
    pub fn runtime_ready(&self) -> bool {
        matches!(self.runtime, None | Some(RuntimeState::Ready { .. }))
    }
}

impl Screen for ExtensionsScreen {
    fn id(&self) -> TabId {
        TabId::Extensions
    }

    fn title_key(&self) -> &'static str {
        "extensions.store.title"
    }

    fn show(&mut self, ui: &mut egui::Ui, c: &mut FeatureCtx) {
        let tr = c.tr;
        let paths = self.paths(c);
        if let Some(r) = self.runtime_task.take() {
            self.runtime = Some(r);
        }
        if self.runtime.is_none() && !self.runtime_task.busy() {
            if self.offline {
                self.runtime = Some(RuntimeState::Missing);
            } else {
                self.refresh_runtime(ui.ctx());
            }
        }

        ui.horizontal_wrapped(|ui| {
            ui.heading(tx(tr, "extensions.store.title", "Extensions"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(tx(
                        tr,
                        "extensions.store.actions.refreshCatalog",
                        "Refresh catalog",
                    ))
                    .clicked()
                {
                    self.mcp.invalidate_catalog();
                    self.plugins.invalidate_catalog();
                    self.runtime = None;
                    self.refresh_runtime(ui.ctx());
                }
                if ui
                    .button(tx(
                        tr,
                        "extensions.store.actions.openDashboard",
                        "Open Dashboard",
                    ))
                    .clicked()
                {
                    c.req.push(ShellRequest::OpenTab(TabId::Dashboard));
                }
                if theme::solid(
                    ui,
                    &tx(tr, "extensions.store.actions.addCustom", "Add Custom"),
                    theme::ACCENT,
                )
                .clicked()
                {
                    self.tab = StoreTab::Mcp;
                    self.mcp.open_custom();
                }
            });
        });
        self.banner(ui, tr);
        ui.label(
            RichText::new(tx(
                tr,
                "extensions.store.sessionsRestartWarning",
                "Running sessions won't pick up extension changes until restarted.",
            ))
            .small()
            .color(theme::WARNING),
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            for (t, key, fb) in [
                (StoreTab::Mcp, "mcpServers", "MCP Servers"),
                (StoreTab::Plugins, "plugins", "Plugins"),
                (StoreTab::Skills, "skills", "Skills"),
                (StoreTab::ApiKeys, "apiKeys", "API Keys"),
            ] {
                ui.selectable_value(
                    &mut self.tab,
                    t,
                    RichText::new(tx(tr, &format!("extensions.store.tabs.{key}.label"), fb))
                        .strong(),
                );
            }
        });
        let (dkey, dfb) = match self.tab {
            StoreTab::Mcp => (
                "mcpServers",
                "Connections to outside tools and apps. They let the runtime read data or do actions beyond this app.",
            ),
            StoreTab::Plugins => (
                "plugins",
                "Small add-ons for the runtime.",
            ),
            StoreTab::Skills => (
                "skills",
                "Ready-made instructions for common jobs. They help the runtime handle repeatable tasks more consistently.",
            ),
            StoreTab::ApiKeys => (
                "apiKeys",
                "Secret keys for online services. Add them here so plugins, servers, and integrations can connect and work.",
            ),
        };
        ui.label(
            RichText::new(tx(
                tr,
                &format!("extensions.store.tabs.{dkey}.description"),
                dfb,
            ))
            .small()
            .color(theme::MUTED),
        );
        theme::rule(ui);

        let ready = self.runtime_ready();
        let offline = self.offline;
        match self.tab {
            StoreTab::Skills => self.skills.show(ui, c),
            tab => {
                let mut x = ExtCtx {
                    home: c.home,
                    paths: &paths,
                    offline,
                    tr,
                    req: &mut *c.req,
                };
                match tab {
                    StoreTab::Mcp => self.mcp.show(ui, &mut x),
                    StoreTab::Plugins => self.plugins.show(ui, &mut x, ready, &self.runtime),
                    _ => self.keys.show(ui, &mut x),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::{testing::render, Intents};
    use rteams_services::ext::mcp_config::{self, Runtime, Scope, ServerSpec};

    fn paths(tag: &str) -> (tempfile::TempDir, ClientPaths) {
        let _ = tag;
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("h");
        let proj = d.path().join("p");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&proj).unwrap();
        (d, ClientPaths::new(home, Some(proj)))
    }

    fn draw(screen: &mut ExtensionsScreen, home: &Path) -> Vec<ShellRequest> {
        let tr = I18n::default();
        let mut reqs = Vec::new();
        render(|ui| {
            let mut out = Intents::default();
            let mut req = Vec::new();
            let mut c = FeatureCtx {
                home,
                team: None,
                view: None,
                out: &mut out,
                tr: &tr,
                req: &mut req,
            };
            screen.show(ui, &mut c);
            reqs.extend(req);
        });
        reqs
    }

    #[test]
    fn renders_every_tab_empty_and_populated() {
        let (d, p) = paths("tabs");
        let home = d.path().join("rteams-home");
        std::fs::create_dir_all(&home).unwrap();
        let mut screen = ExtensionsScreen::sandboxed(p.clone());
        assert_eq!(screen.id(), TabId::Extensions);
        for tab in [
            StoreTab::Mcp,
            StoreTab::Plugins,
            StoreTab::Skills,
            StoreTab::ApiKeys,
        ] {
            screen.tab = tab;
            draw(&mut screen, &home);
        }
        // Populate: an installed MCP server in each scope, an API key.
        mcp_config::upsert(
            &p,
            Runtime::Claude,
            Scope::User,
            "fs",
            &ServerSpec::stdio("npx", &["-y", "x"]),
        )
        .unwrap();
        mcp_config::upsert(
            &p,
            Runtime::Claude,
            Scope::Project,
            "web",
            &ServerSpec::http("https://x.test/mcp"),
        )
        .unwrap();
        rteams_services::ext::apikeys::Store::new(&home)
            .add("OpenAI", "OPENAI_API_KEY", "sk-1234567890", None)
            .unwrap();
        screen.mcp.invalidate_installed();
        screen.keys.invalidate();
        for tab in [
            StoreTab::Mcp,
            StoreTab::Plugins,
            StoreTab::Skills,
            StoreTab::ApiKeys,
        ] {
            screen.tab = tab;
            draw(&mut screen, &home);
        }
    }

    #[test]
    fn header_buttons_have_no_side_effects_until_clicked() {
        let (d, p) = paths("hdr");
        let mut screen = ExtensionsScreen::sandboxed(p);
        let reqs = draw(&mut screen, d.path());
        assert!(reqs.is_empty());
    }

    #[test]
    fn helpers() {
        let tr = I18n::english();
        assert_eq!(tx(&tr, "extensions.store.title", "x"), "Extensions");
        assert_eq!(tx(&tr, "no.such.key", "fallback"), "fallback");
        assert_eq!(txa(&tr, "no.such", "a {{b}} c", &[("b", "X")]), "a X c");
        assert_eq!(big_count(1500), "1.5k");
        assert_eq!(big_count(12), "12");
        let rows = vec![
            ("A".to_string(), "1".to_string()),
            (" ".to_string(), "x".to_string()),
        ];
        assert_eq!(rows_to_map(&rows).len(), 1);
    }
}
