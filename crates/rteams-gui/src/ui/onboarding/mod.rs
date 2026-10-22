//! Zero-setup first-run wizard (original splash / runtime setup): welcome -> detect runtimes ->
//! pick one or the free OpenRouter gateway -> project folder -> starter team template.
//!
//! Shell contract:
//! - `Onboarding::needed(home)` once at startup (no `onboarded` flag in `config.json` and no
//!   team under the home); if true, `Onboarding::start()`.
//! - each frame `show(ctx, tr, home)` while `is_open()`;
//! - `take_draft()` yields the starter `TeamDraft` to create (the wizard also persists the
//!   `onboarded` flag in `AppConfig.extra`, and stores a pasted OpenRouter key via `ext::apikeys`).

use std::path::Path;
use std::sync::mpsc::{channel, Receiver};

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;
use rteams_services::team_import::TeamDraft;
use rteams_services::templates::{self, TeamTemplate};

use crate::icons::{self, Icon};
use crate::theme;

/// `AppConfig.extra` key set once the wizard finished or was skipped.
pub const ONBOARDED_KEY: &str = "onboarded";
/// Env var the OpenRouter key is stored under.
pub const OPENROUTER_ENV: &str = "OPENROUTER_API_KEY";
/// Runtime the free OpenRouter gateway runs through.
pub const GATEWAY_RUNTIME: &str = "opencode";

/// Translate with an English fallback for keys not in the catalog.
pub(crate) fn tx(tr: &I18n, key: &str, default: &str) -> String {
    let s = tr.t(key);
    if s == key {
        default.to_string()
    } else {
        s
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Welcome,
    Runtime,
    Folder,
    Template,
}

impl Step {
    const ALL: [Step; 4] = [Step::Welcome, Step::Runtime, Step::Folder, Step::Template];
    fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRow {
    pub id: String,
    pub label: String,
    pub installed: bool,
    pub version: Option<String>,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    Runtime(String),
    /// Free model through the OpenRouter gateway.
    Free,
}

pub struct Onboarding {
    open: bool,
    pub step: Step,
    rows: Option<Vec<RuntimeRow>>,
    rx: Option<Receiver<Vec<RuntimeRow>>>,
    pub choice: Option<Choice>,
    pub api_key: String,
    pub project: String,
    pub team_name: String,
    templates: Vec<TeamTemplate>,
    pub template: usize,
    pub error: Option<String>,
    draft: Option<TeamDraft>,
}

impl Default for Onboarding {
    fn default() -> Self {
        Onboarding {
            open: false,
            step: Step::Welcome,
            rows: None,
            rx: None,
            choice: None,
            api_key: String::new(),
            project: String::new(),
            team_name: "my-team".into(),
            templates: templates::all(),
            template: 0,
            error: None,
            draft: None,
        }
    }
}

fn has_teams(home: &Path) -> bool {
    std::fs::read_dir(home)
        .is_ok_and(|rd| rd.flatten().any(|e| e.path().join("events.jsonl").exists()))
}

impl Onboarding {
    /// First run: not onboarded yet and no team exists under `home`.
    pub fn needed(home: &Path) -> bool {
        let cfg = AppConfig::load(home);
        let done = cfg
            .extra
            .get(ONBOARDED_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        !done && !has_teams(home)
    }

    pub fn start(&mut self) {
        self.open = true;
        self.step = Step::Welcome;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The starter team to create, once the wizard finished (taken once).
    pub fn take_draft(&mut self) -> Option<TeamDraft> {
        self.draft.take()
    }

    /// Persist the flag so the wizard never shows again.
    pub fn mark_done(home: &Path) {
        let mut cfg = AppConfig::load(home);
        cfg.extra.insert(ONBOARDED_KEY.into(), true.into());
        let _ = cfg.save(home);
    }

    fn detect_in_background(&mut self, ctx: &egui::Context) {
        if self.rows.is_some() || self.rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.rx = Some(rx);
        std::thread::spawn(move || {
            let rows = crate::runtimes::all()
                .iter()
                .map(|r| {
                    let d = r.detect();
                    RuntimeRow {
                        id: r.id().into(),
                        label: r.label().into(),
                        installed: d.path.is_some(),
                        version: d.version,
                        hint: r.install_hint().into(),
                    }
                })
                .collect();
            let _ = tx.send(rows);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(rows) = rx.try_recv() {
                if self.choice.is_none() {
                    if let Some(r) = rows.iter().find(|r| r.installed) {
                        self.choice = Some(Choice::Runtime(r.id.clone()));
                    }
                }
                self.rows = Some(rows);
                self.rx = None;
            }
        }
    }

    #[cfg(test)]
    pub fn set_detected(&mut self, rows: Vec<RuntimeRow>) {
        self.rows = Some(rows);
    }

    /// Runtime id the starter team's members get.
    pub fn runtime_id(&self) -> String {
        match &self.choice {
            Some(Choice::Runtime(id)) => id.clone(),
            Some(Choice::Free) => GATEWAY_RUNTIME.into(),
            None => "claude".into(),
        }
    }

    pub fn can_next(&self) -> bool {
        match self.step {
            Step::Welcome => true,
            Step::Runtime => match &self.choice {
                Some(Choice::Runtime(_)) => true,
                Some(Choice::Free) => !self.api_key.trim().is_empty(),
                None => false,
            },
            Step::Folder => !self.project.trim().is_empty(),
            Step::Template => crate::teams::valid_name(&self.team_name),
        }
    }

    pub fn draft(&self) -> TeamDraft {
        let project = Some(self.project.trim().to_string()).filter(|p| !p.is_empty());
        match self.templates.get(self.template) {
            Some(t) => t.to_draft(&self.team_name, project, &self.runtime_id()),
            None => TeamDraft {
                team_name: self.team_name.trim().into(),
                project_path: project,
                ..Default::default()
            },
        }
    }

    /// Advance; leaving the runtime step saves a pasted OpenRouter key, the last step finishes.
    pub fn next(&mut self, home: &Path) {
        if !self.can_next() {
            return;
        }
        self.error = None;
        if self.step == Step::Runtime && self.choice == Some(Choice::Free) {
            let r = rteams_services::ext::apikeys::Store::new(home).add(
                "OpenRouter",
                OPENROUTER_ENV,
                self.api_key.trim(),
                None,
            );
            if let Err(e) = r {
                self.error = Some(e);
                return;
            }
            self.api_key.clear();
        }
        match Step::ALL.get(self.step.index() + 1) {
            Some(s) => self.step = *s,
            None => self.finish(home),
        }
    }

    pub fn back(&mut self) {
        if let Some(i) = self.step.index().checked_sub(1) {
            self.step = Step::ALL[i];
        }
    }

    fn finish(&mut self, home: &Path) {
        if let Err(e) = rteams_services::projects::folder::ensure(self.project.trim()) {
            self.error = Some(e);
            return;
        }
        self.draft = Some(self.draft());
        Self::mark_done(home);
        self.open = false;
    }

    pub fn skip(&mut self, home: &Path) {
        Self::mark_done(home);
        self.open = false;
    }

    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n, home: &Path) {
        if !self.open {
            return;
        }
        if self.step == Step::Runtime {
            self.detect_in_background(ctx);
        }
        self.poll();
        if self.rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        let mut go_next = false;
        let mut go_back = false;
        let mut skip = false;
        egui::Modal::new(egui::Id::unique("onboarding"))
            .backdrop_color(egui::Color32::from_black_alpha(190))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(14))
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(ctx, |ui| {
                ui.set_width(540.0f32.min((ctx.content_rect().width() - 80.0).max(120.0)));
                ui.spacing_mut().item_spacing.y = 10.0;
                self.dots(ui);
                match self.step {
                    Step::Welcome => welcome(ui, tr),
                    Step::Runtime => self.runtime_step(ui, tr),
                    Step::Folder => self.folder_step(ui, tr),
                    Step::Template => self.template_step(ui, tr),
                }
                if let Some(e) = &self.error {
                    ui.colored_label(theme::ERROR, e);
                }
                ui.add_space(8.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let last = self.step == Step::Template;
                    let label = if last {
                        tx(tr, "rteams.onboarding.create", "Create team")
                    } else {
                        tx(tr, "rteams.onboarding.next", "Continue")
                    };
                    let b = egui::Button::new(
                        RichText::new(label).strong().color(egui::Color32::WHITE),
                    )
                    .fill(theme::ACCENT);
                    if ui.add_enabled(self.can_next(), b).clicked() {
                        go_next = true;
                    }
                    if self.step != Step::Welcome
                        && theme::ghost(ui, &tx(tr, "rteams.onboarding.back", "Back")).clicked()
                    {
                        go_back = true;
                    }
                    if theme::ghost(ui, &tx(tr, "rteams.onboarding.skip", "Skip setup")).clicked() {
                        skip = true;
                    }
                });
            });
        if go_next {
            self.next(home);
        }
        if go_back {
            self.back();
        }
        if skip {
            self.skip(home);
        }
    }

    fn dots(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (i, _) in Step::ALL.iter().enumerate() {
                let on = i <= self.step.index();
                let (r, _) = ui.allocate_exact_size(egui::vec2(28.0, 4.0), egui::Sense::hover());
                ui.painter().rect_filled(
                    r,
                    egui::CornerRadius::same(2),
                    if on {
                        theme::ACCENT
                    } else {
                        theme::hairline(0.12)
                    },
                );
            }
        });
    }

    fn runtime_step(&mut self, ui: &mut egui::Ui, tr: &I18n) {
        title(
            ui,
            Icon::Cpu,
            &tx(
                tr,
                "rteams.onboarding.runtime.title",
                "Choose how agents run",
            ),
        );
        hint(
            ui,
            &tx(
                tr,
                "rteams.onboarding.runtime.hint",
                "Pick an installed agent CLI, or use a free model through the OpenRouter gateway.",
            ),
        );
        match self.rows.clone() {
            None => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    hint(
                        ui,
                        &tx(
                            tr,
                            "rteams.onboarding.detecting",
                            "Detecting installed runtimes...",
                        ),
                    );
                });
            }
            Some(rows) => {
                for r in &rows {
                    let sel = self.choice == Some(Choice::Runtime(r.id.clone()));
                    let sub = if r.installed {
                        r.version.clone().unwrap_or_else(|| "installed".into())
                    } else {
                        r.hint.clone()
                    };
                    if option_row(ui, &r.label, &sub, sel, r.installed).clicked() && r.installed {
                        self.choice = Some(Choice::Runtime(r.id.clone()));
                    }
                }
            }
        }
        let free = self.choice == Some(Choice::Free);
        if option_row(
            ui,
            &tx(
                tr,
                "rteams.onboarding.free.title",
                "Free model via OpenRouter",
            ),
            &tx(
                tr,
                "rteams.onboarding.free.hint",
                "No CLI needed: paste a free OpenRouter API key.",
            ),
            free,
            true,
        )
        .clicked()
        {
            self.choice = Some(Choice::Free);
        }
        if free {
            ui.add(
                egui::TextEdit::singleline(&mut self.api_key)
                    .password(true)
                    .hint_text("sk-or-...")
                    .desired_width(f32::INFINITY),
            );
        }
    }

    fn folder_step(&mut self, ui: &mut egui::Ui, tr: &I18n) {
        title(
            ui,
            Icon::Folder,
            &tx(
                tr,
                "rteams.onboarding.folder.title",
                "Pick a project folder",
            ),
        );
        hint(
            ui,
            &tx(
                tr,
                "rteams.onboarding.folder.hint",
                "Agents work inside this folder.",
            ),
        );
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.project)
                    .hint_text("/path/to/project")
                    .desired_width(ui.available_width() - 110.0),
            );
            if ui
                .button(tx(tr, "rteams.onboarding.browse", "Browse..."))
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new().pick_folder() {
                    self.project = p.to_string_lossy().into_owned();
                }
            }
        });
    }

    fn template_step(&mut self, ui: &mut egui::Ui, tr: &I18n) {
        title(
            ui,
            Icon::Users,
            &tx(
                tr,
                "rteams.onboarding.template.title",
                "Create your first team",
            ),
        );
        ui.label(RichText::new(tr.t("team.create.fields.teamName")).color(theme::TEXT_2));
        ui.add(egui::TextEdit::singleline(&mut self.team_name).desired_width(f32::INFINITY));
        if !crate::teams::valid_name(&self.team_name) {
            ui.label(
                RichText::new(tr.t("team.create.validation.nameMustContainLetterOrDigit"))
                    .size(11.5)
                    .color(theme::ERROR),
            );
        }
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                for (i, t) in self.templates.iter().enumerate() {
                    let sub = tr.t(&t.description_key());
                    if option_row(ui, &tr.t(&t.name_key()), &sub, self.template == i, true)
                        .clicked()
                    {
                        self.template = i;
                    }
                }
            });
    }
}

fn welcome(ui: &mut egui::Ui, tr: &I18n) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(tx(
            tr,
            "rteams.onboarding.welcome.title",
            "Welcome to rteams",
        ))
        .size(24.0)
        .strong(),
    );
    hint(
        ui,
        &tx(
            tr,
            "rteams.onboarding.welcome.body",
            "Run a team of AI agents on a shared kanban board. Setup takes about a minute: choose a runtime, a project folder and a starter team.",
        ),
    );
    ui.add_space(6.0);
}

fn title(ui: &mut egui::Ui, icon: Icon, text: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
        icons::draw(ui.painter(), r, icon, theme::ACCENT);
        ui.label(RichText::new(text).size(18.0).strong());
    });
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(theme::MUTED));
}

/// Selectable card row.
fn option_row(
    ui: &mut egui::Ui,
    label: &str,
    sub: &str,
    selected: bool,
    enabled: bool,
) -> egui::Response {
    let fg = if enabled { theme::TEXT } else { theme::MUTED };
    egui::Frame::new()
        .fill(if selected {
            theme::tint(theme::ACCENT, 0.15)
        } else {
            theme::RAISED
        })
        .stroke(egui::Stroke::new(
            1.0,
            if selected {
                theme::ACCENT
            } else {
                theme::hairline(0.1)
            },
        ))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(label).strong().color(fg));
            if !sub.is_empty() {
                ui.label(RichText::new(sub).size(11.5).color(theme::TEXT_2));
            }
        })
        .response
        .interact(egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rteams-onb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn rows() -> Vec<RuntimeRow> {
        vec![
            RuntimeRow {
                id: "claude".into(),
                label: "Claude Code".into(),
                installed: true,
                version: Some("1.0".into()),
                hint: String::new(),
            },
            RuntimeRow {
                id: "codex".into(),
                label: "Codex".into(),
                installed: false,
                version: None,
                hint: "npm i -g codex".into(),
            },
        ]
    }

    #[test]
    fn needed_only_on_first_run() {
        let h = temp("needed");
        assert!(Onboarding::needed(&h));
        std::fs::create_dir_all(h.join("t1")).unwrap();
        std::fs::write(h.join("t1/events.jsonl"), "").unwrap();
        assert!(!Onboarding::needed(&h));
        let _ = std::fs::remove_dir_all(&h);
        let h = temp("needed2");
        Onboarding::mark_done(&h);
        assert!(!Onboarding::needed(&h));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn full_flow_emits_draft_and_flag() {
        let h = temp("flow");
        let proj = h.join("proj");
        let mut o = Onboarding::default();
        o.start();
        o.set_detected(rows());
        o.next(&h);
        assert_eq!(o.step, Step::Runtime);
        assert!(!o.can_next());
        o.choice = Some(Choice::Runtime("claude".into()));
        o.next(&h);
        assert!(!o.can_next());
        o.project = proj.to_string_lossy().into_owned();
        o.next(&h);
        assert_eq!(o.step, Step::Template);
        o.team_name = "starter".into();
        o.next(&h);
        assert!(!o.is_open());
        let d = o.take_draft().unwrap();
        assert_eq!(d.team_name, "starter");
        assert!(!d.members.is_empty() && d.members.iter().all(|m| m.runtime == "claude"));
        assert!(o.take_draft().is_none());
        assert!(!Onboarding::needed(&h));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn free_gateway_saves_key_and_uses_opencode() {
        let h = temp("free");
        let mut o = Onboarding::default();
        o.start();
        o.next(&h);
        o.choice = Some(Choice::Free);
        assert!(!o.can_next());
        o.api_key = "sk-or-test1234".into();
        o.next(&h);
        assert_eq!(o.step, Step::Folder);
        assert_eq!(o.runtime_id(), GATEWAY_RUNTIME);
        let keys = rteams_services::ext::apikeys::Store::new(&h).list();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].env_key, OPENROUTER_ENV);
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn skip_sets_flag_without_draft() {
        let h = temp("skip");
        let mut o = Onboarding::default();
        o.start();
        o.skip(&h);
        assert!(!o.is_open() && o.take_draft().is_none());
        assert!(!Onboarding::needed(&h));
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn every_step_renders_at_three_sizes() {
        let h = temp("render");
        let tr = I18n::english();
        let mut o = Onboarding::default();
        o.start();
        o.set_detected(rows());
        o.choice = Some(Choice::Free);
        for s in Step::ALL {
            o.step = s;
            crate::view::testing::render(|ui| {
                o.show(ui.ctx(), &tr, &h);
            });
        }
        let _ = std::fs::remove_dir_all(&h);
    }
}
