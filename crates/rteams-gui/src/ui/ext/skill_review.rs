//! Review dialog: shows what a create / save / import will change (new, updated, removed files
//! with diffs and a script-risk summary). Nothing is written until the user confirms.

use eframe::egui::{self, RichText};
use rteams_services::ext_skills::plan::{ChangeKind, Plan};
use rteams_services::ext_skills::review::{self, Finding, Risk};
use rteams_services::i18n::I18n;

use crate::theme;

use super::skills_panel::{t, ta};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    None,
    Apply,
    Cancel,
    /// Return to the editor / import dialog.
    Back,
}

pub struct ReviewDialog {
    pub plan: Plan,
    /// What is being reviewed, e.g. "Importing this skill".
    pub label: String,
    pub findings: Vec<Finding>,
    /// Text of the confirm button.
    pub confirm_text: String,
    pub error: Option<String>,
}

impl ReviewDialog {
    pub fn new(plan: Plan, label: impl Into<String>, confirm_text: impl Into<String>) -> Self {
        let files = plan
            .changes
            .iter()
            .filter(|c| matches!(c.kind, ChangeKind::Added | ChangeKind::Updated))
            .filter_map(|c| c.new.clone().map(|n| (c.rel.clone(), n)))
            .collect();
        let findings = review::review_files(&files);
        Self {
            plan,
            label: label.into(),
            findings,
            confirm_text: confirm_text.into(),
            error: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n) -> ReviewAction {
        let mut action = ReviewAction::None;
        egui::Window::new(t(tr, "extensions.skillReview.title", "Review skill changes"))
            .collapsible(false)
            .default_width(680.0)
            .default_height(520.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(ta(
                        tr,
                        "extensions.skillReview.description",
                        "{{reviewLabel}} previews the filesystem changes first. Nothing is written until you confirm below.",
                        &[("reviewLabel", &self.label)],
                    ))
                    .color(theme::TEXT_2),
                );
                ui.add_space(6.0);
                self.summary(ui, tr);
                if let Some(e) = &self.error {
                    ui.colored_label(theme::ERROR, e);
                }
                self.risk_box(ui);
                ui.add_space(4.0);
                egui::ScrollArea::vertical().max_height((ui.available_height() - 50.0).max(60.0)).auto_shrink([false, false]).show(ui, |ui| {
                    if self.plan.change_count() == 0 {
                        ui.label(RichText::new(t(tr, "extensions.skillReview.noChanges", "No file changes detected yet.")).color(theme::MUTED));
                    }
                    for c in self.plan.changes.iter().filter(|c| c.kind != ChangeKind::Unchanged) {
                        file_block(ui, c, tr);
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if theme::ghost(ui, &t(tr, "extensions.skillImport.actions.cancel", "Cancel")).clicked() {
                        action = ReviewAction::Cancel;
                    }
                    if theme::ghost(ui, &t(tr, "extensions.skillImport.actions.backToImport", "Back")).clicked() {
                        action = ReviewAction::Back;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let enabled = !self.plan.is_noop();
                        if ui
                            .add_enabled(enabled, egui::Button::new(RichText::new(&self.confirm_text).strong().color(egui::Color32::WHITE)).fill(theme::ACCENT))
                            .clicked()
                        {
                            action = ReviewAction::Apply;
                        }
                    });
                });
            });
        action
    }

    fn summary(&self, ui: &mut egui::Ui, tr: &I18n) {
        let c = self.plan.counts();
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(ta(
                    tr,
                    "extensions.skillReview.summary.fileChanges",
                    "{{count}} file changes",
                    &[("count", &self.plan.change_count().to_string())],
                ))
                .strong(),
            );
            if c.added > 0 {
                theme::pill(
                    ui,
                    ta(
                        tr,
                        "extensions.skillReview.summary.new",
                        "{{count}} new",
                        &[("count", &c.added.to_string())],
                    ),
                    theme::SUCCESS,
                );
            }
            if c.updated > 0 {
                theme::pill(
                    ui,
                    ta(
                        tr,
                        "extensions.skillReview.summary.updated",
                        "{{count}} updated",
                        &[("count", &c.updated.to_string())],
                    ),
                    theme::INFO,
                );
            }
            if c.removed > 0 {
                theme::pill(
                    ui,
                    ta(
                        tr,
                        "extensions.skillReview.summary.removed",
                        "{{count}} removed",
                        &[("count", &c.removed.to_string())],
                    ),
                    theme::ERROR,
                );
            }
            if c.binary > 0 {
                theme::pill(
                    ui,
                    ta(
                        tr,
                        "extensions.skillReview.summary.binary",
                        "{{count}} binary",
                        &[("count", &c.binary.to_string())],
                    ),
                    theme::MUTED,
                );
            }
        });
    }

    fn risk_box(&self, ui: &mut egui::Ui) {
        let Some(max) = review::max_risk(&self.findings) else {
            return;
        };
        if max == Risk::Info && self.findings.iter().all(|f| f.line == 0) {
            ui.label(
                RichText::new(format!(
                    "This skill includes {} script file(s).",
                    self.findings.len()
                ))
                .color(theme::WARNING),
            );
            return;
        }
        let color = if max == Risk::High {
            theme::ERROR
        } else {
            theme::WARNING
        };
        egui::CollapsingHeader::new(
            RichText::new(format!("Script review: {} finding(s)", self.findings.len()))
                .color(color),
        )
        .id_salt("skill_review_risks")
        .default_open(max == Risk::High)
        .show(ui, |ui| {
            for f in self.findings.iter().take(40) {
                let loc = if f.line == 0 {
                    f.file.clone()
                } else {
                    format!("{}:{}", f.file, f.line)
                };
                ui.label(RichText::new(format!("{loc}  {}", f.reason)).small());
                if !f.snippet.is_empty() {
                    ui.label(
                        RichText::new(&f.snippet)
                            .monospace()
                            .small()
                            .color(theme::MUTED),
                    );
                }
            }
        });
    }
}

fn file_block(ui: &mut egui::Ui, c: &rteams_services::ext_skills::plan::FileChange, tr: &I18n) {
    let (label, color) = match c.kind {
        ChangeKind::Added => ("new", theme::SUCCESS),
        ChangeKind::Updated => ("updated", theme::INFO),
        ChangeKind::Removed => ("removed", theme::ERROR),
        ChangeKind::Unchanged => ("same", theme::MUTED),
    };
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(6)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::pill(ui, label, color);
                ui.label(RichText::new(&c.rel).monospace());
                if c.binary {
                    theme::pill(
                        ui,
                        t(tr, "extensions.skillReview.binaryBadge", "binary"),
                        theme::MUTED,
                    );
                }
                if !c.binary {
                    ui.label(
                        RichText::new(format!("+{} -{}", c.added_lines, c.removed_lines))
                            .small()
                            .color(theme::MUTED),
                    );
                }
            });
            if c.binary {
                ui.label(
                    RichText::new(t(
                        tr,
                        "extensions.skillReview.binaryPreviewHidden",
                        "Binary file preview is not shown. The file will be copied as-is.",
                    ))
                    .small()
                    .color(theme::MUTED),
                );
            } else {
                for line in c.diff.lines().take(400) {
                    let col = if line.starts_with("+++") || line.starts_with("---") {
                        theme::TEXT_2
                    } else if line.starts_with('+') {
                        theme::RESULT_FG
                    } else if line.starts_with('-') {
                        theme::ERROR_FG
                    } else if line.starts_with("@@") {
                        theme::INFO
                    } else {
                        theme::MUTED
                    };
                    ui.label(RichText::new(line).monospace().size(12.0).color(col));
                }
            }
        });
    ui.add_space(4.0);
}
