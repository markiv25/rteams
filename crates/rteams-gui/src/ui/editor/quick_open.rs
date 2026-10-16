//! Quick Open (Cmd/Ctrl+P): fuzzy file-name picker.

use eframe::egui;
use rteams_services::editor::search::{self, QuickHit};
use rteams_services::i18n::I18n;

use crate::theme;

#[derive(Debug, Default)]
pub struct QuickOpen {
    pub open: bool,
    pub query: String,
    pub hits: Vec<QuickHit>,
    pub selected: usize,
    last_query: Option<String>,
    focus: bool,
}

impl QuickOpen {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.query.clear();
            self.selected = 0;
            self.last_query = None;
            self.focus = true;
        }
    }

    /// Returns the chosen file (relative path). `files` is the cached list, `None` while loading.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        files: Option<&[String]>,
        tr: &I18n,
    ) -> Option<String> {
        if !self.open {
            return None;
        }
        let mut chosen = None;
        let (up, down, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if esc {
            self.open = false;
            return None;
        }
        if self.last_query.as_deref() != Some(self.query.as_str()) {
            if let Some(f) = files {
                self.hits = search::quick_open(f, &self.query, 50);
                self.last_query = Some(self.query.clone());
                self.selected = 0;
            }
        }
        if down && self.selected + 1 < self.hits.len() {
            self.selected += 1;
        }
        if up {
            self.selected = self.selected.saturating_sub(1);
        }
        if enter {
            chosen = self.hits.get(self.selected).map(|h| h.rel.clone());
        }
        egui::Window::new(tr.t("team.editor.quickOpen.title"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .collapsible(false)
            .resizable(false)
            .fixed_size([
                480.0_f32.min(ctx.content_rect().width() - 20.0).max(100.0),
                0.0,
            ])
            .show(ctx, |ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text(tr.t("team.editor.quickOpen.searchPlaceholder"))
                        .desired_width(f32::INFINITY),
                );
                if self.focus {
                    r.request_focus();
                    self.focus = false;
                }
                ui.add_space(4.0);
                if files.is_none() {
                    ui.label(
                        egui::RichText::new(tr.t("team.editor.quickOpen.loading"))
                            .color(theme::MUTED),
                    );
                } else if self.hits.is_empty() {
                    ui.label(
                        egui::RichText::new(tr.t("team.editor.quickOpen.empty"))
                            .color(theme::MUTED),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (i, h) in self.hits.iter().enumerate() {
                            let (dir, name) = h
                                .rel
                                .rsplit_once('/')
                                .map_or(("", h.rel.as_str()), |(d, n)| (d, n));
                            let resp = ui.selectable_label(
                                i == self.selected,
                                egui::RichText::new(format!("{name}   {dir}")),
                            );
                            if i == self.selected {
                                resp.scroll_to_me(None);
                            }
                            if resp.clicked() {
                                chosen = Some(h.rel.clone());
                            }
                        }
                    });
            });
        if chosen.is_some() {
            self.open = false;
        }
        chosen
    }
}
