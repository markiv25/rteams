//! Search box (debounced) and the result list with highlighted snippets.

use std::time::{Duration, Instant};

use eframe::egui::{self, text::LayoutJob, Color32, RichText, TextFormat};
use rteams_services::i18n::I18n;
use rteams_services::sessions::search::{SearchHit, MAX_RESULTS};

use super::{fmt_date, tt};
use crate::theme;

const DEBOUNCE: Duration = Duration::from_millis(400);

#[derive(Default)]
pub struct SearchState {
    pub query: String,
    pub results: Vec<SearchHit>,
    pub searching: bool,
    /// Query the current `results` belong to.
    pub ran_query: String,
    edited_at: Option<Instant>,
}

impl SearchState {
    pub fn active(&self) -> bool {
        !self.query.trim().is_empty()
    }

    pub fn clear(&mut self) {
        self.query.clear();
        self.results.clear();
        self.ran_query.clear();
        self.searching = false;
        self.edited_at = None;
    }

    pub fn capped(&self) -> bool {
        self.results.len() >= MAX_RESULTS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchAction {
    None,
    Run(String),
    Clear,
}

/// The input row.
pub fn show_bar(ui: &mut egui::Ui, tr: &I18n, st: &mut SearchState) -> SearchAction {
    let mut action = SearchAction::None;
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(&mut st.query)
                .hint_text(tt(
                    tr,
                    "common.sessions.search.placeholder",
                    "Search sessions...",
                ))
                .desired_width(280.0),
        );
        if r.changed() {
            st.edited_at = Some(Instant::now());
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let q = st.query.trim().to_string();
        let due = st.edited_at.is_some_and(|t| t.elapsed() >= DEBOUNCE);
        if (enter || due) && q.chars().count() >= 2 && q != st.ran_query {
            st.edited_at = None;
            action = SearchAction::Run(q);
        } else if st.edited_at.is_some() {
            ui.ctx().request_repaint_after(DEBOUNCE);
        }
        if st.searching {
            ui.spinner();
        }
        if st.active()
            && theme::ghost(
                ui,
                &tt(tr, "common.sessions.search.clear", "Clear session search"),
            )
            .clicked()
        {
            action = SearchAction::Clear;
        }
    });
    action
}

fn highlighted(h: &SearchHit, ui: &egui::Ui) -> LayoutJob {
    let chars: Vec<char> = h.snippet.chars().collect();
    let a = h.match_start.min(chars.len());
    let b = (h.match_start + h.match_len).min(chars.len());
    let s = |r: std::ops::Range<usize>| chars[r].iter().collect::<String>();
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), theme::TEXT_2);
    let hot = TextFormat {
        background: theme::tint(theme::WARNING, 0.35),
        ..TextFormat::simple(font, Color32::WHITE)
    };
    job.append(&s(0..a), 0.0, plain.clone());
    job.append(&s(a..b), 0.0, hot);
    job.append(&s(b..chars.len()), 0.0, plain);
    job.wrap.max_width = ui.available_width();
    job
}

/// Result list; returns the index of the clicked hit.
pub fn show_results(ui: &mut egui::Ui, tr: &I18n, st: &SearchState) -> Option<usize> {
    let mut clicked = None;
    if st.searching && st.results.is_empty() {
        ui.label(
            RichText::new(tt(tr, "common.search.searching", "Searching...")).color(theme::MUTED),
        );
        return None;
    }
    if st.results.is_empty() {
        ui.label(
            RichText::new(tt(tr, "common.search.noResults", "No results")).color(theme::MUTED),
        );
        return None;
    }
    ui.label(
        RichText::new(format!(
            "{}{}",
            st.results.len(),
            if st.capped() { "+" } else { "" }
        ))
        .small()
        .color(theme::MUTED),
    );
    egui::ScrollArea::vertical()
        .id_salt("session_search_results")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, h) in st.results.iter().enumerate() {
                let r = egui::Frame::new()
                    .fill(theme::CARD)
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            theme::pill(
                                ui,
                                h.role,
                                if h.role == "user" {
                                    theme::ACCENT
                                } else {
                                    theme::SUCCESS
                                },
                            );
                            ui.label(RichText::new(&h.session_id).small().color(theme::MUTED));
                            if let Some(t) = h.ts_ms {
                                ui.label(RichText::new(fmt_date(t)).small().color(theme::MUTED));
                            }
                        });
                        ui.add(egui::Label::new(highlighted(h, ui)));
                    })
                    .response
                    .interact(egui::Sense::click());
                if r.clicked() {
                    clicked = Some(i);
                }
                ui.add_space(4.0);
            }
        });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn renders_results_and_bar() {
        let tr = I18n::default();
        let mut st = SearchState {
            query: "login".into(),
            ..SearchState::default()
        };
        st.results.push(SearchHit {
            session_id: "s1".into(),
            path: PathBuf::from("/x/s1.jsonl").to_string_lossy().into_owned(),
            cwd: "/x".into(),
            source: rteams_services::sessions::paths::Source::Claude,
            role: "user",
            snippet: "fix the login bug".into(),
            match_start: 8,
            match_len: 5,
            ts_ms: Some(1_700_000_000_000),
        });
        crate::view::testing::render(|ui| {
            let _ = show_bar(ui, &tr, &mut st);
            assert_eq!(show_results(ui, &tr, &st), None);
        });
        st.clear();
        assert!(!st.active());
    }
}
