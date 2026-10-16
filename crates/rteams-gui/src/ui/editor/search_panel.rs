//! Search in Files (Cmd/Ctrl+Shift+F), shown in the sidebar. Searches run on a worker thread.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use rteams_services::editor::search::{self, SearchOpts, SearchResults};
use rteams_services::i18n::I18n;

use crate::theme;

#[derive(Default)]
pub struct SearchPanel {
    pub open: bool,
    pub opts: SearchOpts,
    pub results: Option<SearchResults>,
    pub show_filters: bool,
    rx: Option<Receiver<SearchResults>>,
    focus: bool,
}

/// Open this file at (line, col).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jump {
    pub rel: String,
    pub line: usize,
    pub col: usize,
}

impl SearchPanel {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.focus = self.open;
    }

    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    pub fn start(&mut self, root: PathBuf, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let opts = self.opts.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(search::search_content(&root, &opts));
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    /// Collect a finished search (call every frame).
    pub fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(r) = rx.try_recv() {
                self.results = Some(r);
                self.rx = None;
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, root: &std::path::Path, tr: &I18n) -> Option<Jump> {
        let mut jump = None;
        let mut run = false;
        ui.horizontal(|ui| {
            ui.strong(tr.t("team.editor.searchInFiles.title"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .small_button("🗙")
                    .on_hover_text(tr.t("team.editor.searchInFiles.closeSearchShortcut"))
                    .clicked()
                {
                    self.open = false;
                }
            });
        });
        let r = ui.add(
            egui::TextEdit::singleline(&mut self.opts.query)
                .hint_text(tr.t("team.editor.searchInFiles.searchPlaceholder"))
                .desired_width(f32::INFINITY),
        );
        if self.focus {
            r.request_focus();
            self.focus = false;
        }
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            run = true;
        }
        ui.horizontal_wrapped(|ui| {
            run |= ui
                .toggle_value(
                    &mut self.opts.case_sensitive,
                    tr.t("team.editor.searchInFiles.matchCaseToggle"),
                )
                .on_hover_text(tr.t("team.editor.searchInFiles.matchCase"))
                .changed();
            run |= ui
                .toggle_value(&mut self.opts.whole_word, "W")
                .on_hover_text("Whole word")
                .changed();
            run |= ui
                .toggle_value(&mut self.opts.regex, ".*")
                .on_hover_text("Regex")
                .changed();
            ui.toggle_value(&mut self.show_filters, "…")
                .on_hover_text("Files to include / exclude");
        });
        if self.show_filters {
            ui.add(
                egui::TextEdit::singleline(&mut self.opts.include)
                    .hint_text("include: *.rs, src/**")
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.opts.exclude)
                    .hint_text("exclude: target/**")
                    .desired_width(f32::INFINITY),
            );
        }
        if run && !self.opts.query.is_empty() {
            self.start(root.to_path_buf(), ui.ctx());
        }
        ui.separator();
        if self.running() {
            ui.spinner();
        }
        if let Some(res) = &self.results {
            if let Some(e) = &res.error {
                ui.colored_label(theme::ERROR, e);
            } else if res.matches.is_empty() {
                ui.label(
                    egui::RichText::new(tr.t("team.editor.searchInFiles.noResults"))
                        .color(theme::MUTED),
                );
            } else {
                let mut s = tr.t_args(
                    "team.editor.searchInFiles.resultsSummary",
                    &[
                        ("count", &res.matches.len().to_string()),
                        ("fileCount", &res.file_count.to_string()),
                    ],
                );
                if res.truncated {
                    s.push(' ');
                    s.push_str(&tr.t("team.editor.searchInFiles.truncated"));
                }
                ui.label(egui::RichText::new(s).color(theme::TEXT_2).small());
            }
            egui::ScrollArea::vertical()
                .id_salt("search-results")
                .show(ui, |ui| {
                    let mut last = "";
                    for m in &res.matches {
                        if m.rel != last {
                            ui.label(egui::RichText::new(&m.rel).strong().small());
                            last = &m.rel;
                        }
                        let r = ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("{:>4}  {}", m.line, m.text))
                                    .monospace()
                                    .small()
                                    .color(theme::TEXT_2),
                            )
                            .truncate()
                            .sense(egui::Sense::click()),
                        );
                        if r.clicked() {
                            jump = Some(Jump {
                                rel: m.rel.clone(),
                                line: m.line,
                                col: m.col,
                            });
                        }
                    }
                });
        }
        jump
    }
}
