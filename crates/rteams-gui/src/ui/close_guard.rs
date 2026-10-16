//! close_guard: one "unsaved work" prompt for every screen that can hold something the user
//! would lose on quit. Participants implement `CloseParticipant`; the shell calls `request` when
//! the window asks to close and `show` every frame until a decision comes back.

use eframe::egui::{self, Color32, RichText};

use super::{CloseBlocker, CloseParticipant};
use crate::theme;
use rteams_services::i18n::I18n;

/// Translated text for `key`, or `fallback` when the catalog has no such key.
pub(crate) fn tx(tr: &I18n, key: &str, fallback: &str) -> String {
    if tr.has(key) {
        tr.t(key)
    } else {
        fallback.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// Prompt is (still) open or nothing has been decided.
    Pending,
    /// Close the window now.
    Quit,
    /// The user cancelled; keep running.
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    SaveAll,
    QuitAnyway,
    Cancel,
}

/// Blockers of every participant, in order.
pub fn collect(parts: &[&mut dyn CloseParticipant]) -> Vec<CloseBlocker> {
    parts.iter().flat_map(|p| p.blockers()).collect()
}

/// A participant made of a fixed list of blockers (e.g. running terminals); saving is a no-op.
pub struct StaticBlockers(pub Vec<CloseBlocker>);

impl StaticBlockers {
    pub fn new(owner: &str, count: usize, noun: &str) -> Self {
        StaticBlockers(if count == 0 {
            vec![]
        } else {
            vec![CloseBlocker {
                owner: owner.into(),
                detail: format!("{count} {noun}"),
            }]
        })
    }
}

impl CloseParticipant for StaticBlockers {
    fn blockers(&self) -> Vec<CloseBlocker> {
        self.0.clone()
    }
    fn save_all(&mut self) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
pub struct CloseGuard {
    open: bool,
    blockers: Vec<CloseBlocker>,
    error: Option<String>,
}

impl CloseGuard {
    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Call when the window asks to close. Returns `Quit` when nothing blocks, otherwise opens
    /// the prompt and returns `Pending` (the shell should also send `CancelClose`).
    pub fn request(&mut self, parts: &mut [&mut dyn CloseParticipant]) -> CloseDecision {
        let blockers = collect(parts);
        if blockers.is_empty() {
            self.open = false;
            return CloseDecision::Quit;
        }
        self.blockers = blockers;
        self.error = None;
        self.open = true;
        CloseDecision::Pending
    }

    /// Pure decision logic for a button press.
    pub fn resolve(
        &mut self,
        action: CloseAction,
        parts: &mut [&mut dyn CloseParticipant],
    ) -> CloseDecision {
        match action {
            CloseAction::Cancel => {
                self.open = false;
                CloseDecision::Cancel
            }
            CloseAction::QuitAnyway => {
                self.open = false;
                CloseDecision::Quit
            }
            CloseAction::SaveAll => {
                for p in parts.iter_mut() {
                    if let Err(e) = p.save_all() {
                        self.error = Some(e);
                        return CloseDecision::Pending;
                    }
                }
                self.open = false;
                CloseDecision::Quit
            }
        }
    }

    /// Draw the prompt (when open) and return the decision.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        parts: &mut [&mut dyn CloseParticipant],
        tr: &I18n,
    ) -> CloseDecision {
        if !self.open {
            return CloseDecision::Pending;
        }
        let mut action = None;
        let width = (ctx.content_rect().width() * 0.92).clamp(1.0, 480.0);
        let modal = egui::Modal::new(egui::Id::unique("close_guard"))
            .backdrop_color(Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.label(
                    RichText::new(tx(tr, "team.editor.unsavedChanges", "Unsaved changes"))
                        .size(18.0)
                        .strong(),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(tx(tr, "closeGuard.body", "These will be lost if you quit:"))
                        .color(theme::TEXT_2),
                );
                ui.add_space(4.0);
                for b in self.blockers.iter().take(12) {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&b.owner).color(theme::MUTED));
                        ui.label(RichText::new(&b.detail).monospace().color(theme::TEXT));
                    });
                }
                if self.blockers.len() > 12 {
                    ui.label(
                        RichText::new(format!("+{} more", self.blockers.len() - 12))
                            .color(theme::MUTED),
                    );
                }
                if let Some(e) = &self.error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(theme::ERROR_FG));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if theme::solid(ui, &tx(tr, "closeGuard.saveAll", "Save all"), theme::ACCENT)
                        .clicked()
                    {
                        action = Some(CloseAction::SaveAll);
                    }
                    if ui
                        .button(tx(tr, "closeGuard.quitAnyway", "Quit anyway"))
                        .clicked()
                    {
                        action = Some(CloseAction::QuitAnyway);
                    }
                    if ui
                        .button(tx(tr, "common.actions.cancel", "Cancel"))
                        .clicked()
                    {
                        action = Some(CloseAction::Cancel);
                    }
                });
            });
        if action.is_none() && modal.should_close() {
            action = Some(CloseAction::Cancel);
        }
        match action {
            Some(a) => self.resolve(a, parts),
            None => CloseDecision::Pending,
        }
    }
}

/// Run one headless frame (tests of all my screens).
#[cfg(test)]
pub(crate) fn headless(ctx: &egui::Context, f: impl FnMut(&mut egui::Ui)) {
    let mut o = ctx.run_ui(egui::RawInput::default(), f);
    o.textures_delta.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        dirty: Vec<&'static str>,
        fail: bool,
        saved: bool,
    }
    impl CloseParticipant for Fake {
        fn blockers(&self) -> Vec<CloseBlocker> {
            self.dirty
                .iter()
                .map(|d| CloseBlocker {
                    owner: "x".into(),
                    detail: d.to_string(),
                })
                .collect()
        }
        fn save_all(&mut self) -> Result<(), String> {
            self.saved = true;
            if self.fail {
                Err("disk full".into())
            } else {
                self.dirty.clear();
                Ok(())
            }
        }
    }
    fn fake(d: &[&'static str], fail: bool) -> Fake {
        Fake {
            dirty: d.to_vec(),
            fail,
            saved: false,
        }
    }

    #[test]
    fn no_blockers_quits_immediately() {
        let (mut a, mut g) = (fake(&[], false), CloseGuard::default());
        assert_eq!(g.request(&mut [&mut a]), CloseDecision::Quit);
        assert!(!g.is_open());
    }

    #[test]
    fn blockers_open_prompt_and_collect_all() {
        let (mut a, mut b, mut g) = (
            fake(&["a.rs"], false),
            fake(&["b.rs", "c.rs"], false),
            CloseGuard::default(),
        );
        assert_eq!(g.request(&mut [&mut a, &mut b]), CloseDecision::Pending);
        assert!(g.is_open());
        assert_eq!(g.blockers.len(), 3);
    }

    #[test]
    fn save_all_failure_keeps_prompt_open() {
        let (mut a, mut b, mut g) = (
            fake(&["a"], true),
            fake(&["b"], false),
            CloseGuard::default(),
        );
        g.request(&mut [&mut a, &mut b]);
        assert_eq!(
            g.resolve(CloseAction::SaveAll, &mut [&mut a, &mut b]),
            CloseDecision::Pending
        );
        assert!(g.is_open() && g.error.as_deref() == Some("disk full"));
        assert!(!b.saved, "stops at the first failure");
    }

    #[test]
    fn save_all_success_quit_anyway_cancel() {
        let (mut a, mut g) = (fake(&["a"], false), CloseGuard::default());
        g.request(&mut [&mut a]);
        assert_eq!(
            g.resolve(CloseAction::SaveAll, &mut [&mut a]),
            CloseDecision::Quit
        );
        assert!(a.saved && !g.is_open());
        let mut a = fake(&["a"], false);
        g.request(&mut [&mut a]);
        assert_eq!(
            g.resolve(CloseAction::QuitAnyway, &mut [&mut a]),
            CloseDecision::Quit
        );
        assert!(!a.saved);
        g.request(&mut [&mut a]);
        assert_eq!(
            g.resolve(CloseAction::Cancel, &mut [&mut a]),
            CloseDecision::Cancel
        );
    }

    #[test]
    fn static_blockers_and_headless_render() {
        let mut t = StaticBlockers::new("Terminal", 2, "running terminal(s)");
        assert_eq!(t.blockers()[0].detail, "2 running terminal(s)");
        assert!(StaticBlockers::new("T", 0, "x").blockers().is_empty());
        let mut g = CloseGuard::default();
        g.request(&mut [&mut t]);
        let ctx = egui::Context::default();
        let tr = I18n::english();
        let mut d = CloseDecision::Quit;
        crate::ui::close_guard::headless(&ctx, |ui| {
            d = g.show(ui.ctx(), &mut [&mut t], &tr);
        });
        assert_eq!(d, CloseDecision::Pending);
        assert!(g.is_open());
    }
}
