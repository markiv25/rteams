//! Command palette (Cmd/Ctrl+K) and global search (Cmd/Ctrl+Shift+K). The shell builds the
//! candidate list; this widget filters (fuzzy on the label, substring on the haystack), ranks,
//! groups and handles the keyboard, and reports the chosen index.

use eframe::egui::{self, RichText};

use crate::icons::{self, Icon};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Actions, tasks, members and content.
    Palette,
    /// Content only: tasks, comments, messages, logs.
    Search,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Action,
    Task,
    Member,
    Message,
    Comment,
    Log,
}

impl Group {
    pub fn key(self) -> &'static str {
        match self {
            Group::Action => "rteams.shell.palette.actions",
            Group::Task => "rteams.shell.palette.tasks",
            Group::Member => "rteams.shell.palette.members",
            Group::Message => "rteams.shell.palette.messages",
            Group::Comment => "rteams.shell.palette.comments",
            Group::Log => "rteams.shell.palette.logs",
        }
    }

    /// Content groups are only searched once the query has two characters.
    pub fn is_content(self) -> bool {
        matches!(self, Group::Message | Group::Comment | Group::Log)
    }
}

#[derive(Debug, Clone)]
pub struct Item {
    pub group: Group,
    pub icon: Icon,
    pub label: String,
    pub detail: String,
    /// Extra text matched by substring only (message bodies, log lines).
    pub haystack: String,
}

#[derive(Default)]
pub struct Palette {
    open: bool,
    search: bool,
    query: String,
    sel: usize,
    focus: bool,
}

/// Subsequence score of `q` in `text` (case-insensitive): consecutive and word-start matches
/// score higher; `None` when `q` is not a subsequence.
pub fn fuzzy(q: &str, text: &str) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut pos = 0;
    let mut prev: Option<usize> = None;
    for c in q.to_lowercase().chars() {
        let i = (pos..t.len()).find(|&i| t[i] == c)?;
        score += 1;
        if prev == Some(i.wrapping_sub(1)) {
            score += 4;
        }
        if i == 0 || !t[i - 1].is_alphanumeric() {
            score += 3;
        }
        score -= (i - pos).min(5) as i32 / 2;
        prev = Some(i);
        pos = i + 1;
    }
    if t.iter().collect::<String>().starts_with(&q.to_lowercase()) {
        score += 10;
    }
    Some(score)
}

/// Indices of `items` matching `query`, best first, at most `per_group` per group.
pub fn filter(items: &[Item], query: &str, mode: Mode, per_group: usize) -> Vec<usize> {
    let q = query.trim();
    let mut scored: Vec<(usize, i32)> = Vec::new();
    for (i, it) in items.iter().enumerate() {
        if mode == Mode::Search && matches!(it.group, Group::Action | Group::Member) {
            continue;
        }
        if q.is_empty() {
            if it.group == Group::Action && mode == Mode::Palette {
                scored.push((i, 0));
            }
            continue;
        }
        if it.group.is_content() && q.chars().count() < 2 {
            continue;
        }
        let ql = q.to_lowercase();
        let s = if it.group.is_content() {
            it.haystack.to_lowercase().contains(&ql).then_some(1)
        } else {
            fuzzy(q, &it.label).or_else(|| {
                (it.haystack.to_lowercase().contains(&ql) || it.detail.to_lowercase().contains(&ql))
                    .then_some(-5)
            })
        };
        if let Some(s) = s {
            scored.push((i, s));
        }
    }
    // Stable sort: groups keep their declared order, best score first inside a group.
    scored.sort_by_key(|&(i, s)| (items[i].group as u8, -s));
    let mut counts = [0usize; 6];
    scored
        .into_iter()
        .filter(|&(i, _)| {
            let c = &mut counts[items[i].group as usize];
            *c += 1;
            *c <= per_group
        })
        .map(|(i, _)| i)
        .collect()
}

impl Palette {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn open(&mut self, search: bool) {
        self.open = true;
        self.search = search;
        self.query.clear();
        self.sel = 0;
        self.focus = true;
    }

    #[cfg(test)]
    pub fn close(&mut self) {
        self.open = false;
    }

    #[cfg(test)]
    pub fn set_query(&mut self, q: &str) {
        self.query = q.to_string();
    }

    /// Draw the palette; returns the index (into `items`) the user picked.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        tr: &rteams_services::i18n::I18n,
        items: &[Item],
    ) -> Option<usize> {
        if !self.open {
            return None;
        }
        let mode = if self.search {
            Mode::Search
        } else {
            Mode::Palette
        };
        let shown = filter(items, &self.query, mode, 8);
        self.sel = self.sel.min(shown.len().saturating_sub(1));
        let (down, up, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::Enter),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if down && !shown.is_empty() {
            self.sel = (self.sel + 1) % shown.len();
        }
        if up && !shown.is_empty() {
            self.sel = (self.sel + shown.len() - 1) % shown.len();
        }
        if esc {
            self.open = false;
            return None;
        }
        let mut picked = enter.then(|| shown.get(self.sel).copied()).flatten();
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::unique("palette_scrim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, r) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(120));
                if r.clicked() {
                    self.open = false;
                }
            });
        let w = (screen.width() - 32.0).clamp(240.0, 640.0);
        let placeholder = tr.t(if self.search {
            "rteams.shell.search.placeholder"
        } else {
            "rteams.shell.palette.placeholder"
        });
        egui::Area::new(egui::Id::unique("palette"))
            .order(egui::Order::Tooltip)
            .fixed_pos(egui::pos2(screen.center().x - w / 2.0, screen.top() + 80.0))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(w - 22.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui
                                .allocate_exact_size(egui::vec2(18.0, 24.0), egui::Sense::hover());
                            icons::draw(
                                ui.painter(),
                                egui::Rect::from_center_size(r.center(), egui::vec2(16.0, 16.0)),
                                Icon::Search,
                                theme::TEXT_2,
                            );
                            let te = ui.add(
                                egui::TextEdit::singleline(&mut self.query)
                                    .hint_text(placeholder)
                                    .frame(egui::Frame::NONE)
                                    .desired_width(ui.available_width()),
                            );
                            if self.focus {
                                te.request_focus();
                                self.focus = false;
                            }
                            if te.changed() {
                                self.sel = 0;
                            }
                        });
                        theme::rule(ui);
                        egui::ScrollArea::vertical()
                            .max_height((screen.height() - 220.0).clamp(120.0, 440.0))
                            .auto_shrink([false, true])
                            .show(ui, |ui| {
                                if shown.is_empty() {
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(tr.t("rteams.shell.palette.empty"))
                                            .color(theme::MUTED),
                                    );
                                }
                                let mut last: Option<Group> = None;
                                for (row, &i) in shown.iter().enumerate() {
                                    let it = &items[i];
                                    if last != Some(it.group) {
                                        last = Some(it.group);
                                        ui.add_space(4.0);
                                        ui.label(
                                            RichText::new(tr.t(it.group.key()))
                                                .size(11.0)
                                                .color(theme::MUTED),
                                        );
                                    }
                                    let r = row_ui(ui, it, row == self.sel);
                                    if r.hovered()
                                        && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
                                    {
                                        self.sel = row;
                                    }
                                    if row == self.sel && (down || up) {
                                        r.scroll_to_me(None);
                                    }
                                    if r.clicked() {
                                        picked = Some(i);
                                    }
                                }
                            });
                    });
            });
        if picked.is_some() {
            self.open = false;
        }
        picked
    }
}

fn row_ui(ui: &mut egui::Ui, it: &Item, selected: bool) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    if selected || resp.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(7),
            if selected {
                theme::tint(theme::ACCENT, 0.18)
            } else {
                egui::Color32::from_rgb(0x25, 0x27, 0x33)
            },
        );
    }
    icons::draw(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::vec2(16.0, 16.0),
        ),
        it.icon,
        if selected { theme::TEXT } else { theme::TEXT_2 },
    );
    let font = egui::TextStyle::Body.resolve(ui.style());
    let label = ui.painter().layout_no_wrap(
        clip(&it.label, 60),
        font.clone(),
        if selected { theme::TEXT } else { theme::TEXT_2 },
    );
    let lx = rect.left() + 36.0;
    let lw = label.size().x;
    ui.painter().galley(
        egui::pos2(lx, rect.center().y - label.size().y / 2.0),
        label,
        theme::TEXT,
    );
    if !it.detail.is_empty() {
        let room = (rect.right() - (lx + lw + 20.0)).max(0.0);
        let chars = (room / 7.0) as usize;
        if chars > 4 {
            let d = ui.painter().layout_no_wrap(
                clip(&it.detail, chars),
                egui::FontId::proportional(12.0),
                theme::MUTED,
            );
            ui.painter().galley(
                egui::pos2(lx + lw + 12.0, rect.center().y - d.size().y / 2.0),
                d,
                theme::MUTED,
            );
        }
    }
    resp
}

fn clip(s: &str, n: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n {
        format!(
            "{}\u{2026}",
            one.chars().take(n.saturating_sub(1)).collect::<String>()
        )
    } else {
        one
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(group: Group, label: &str, hay: &str) -> Item {
        Item {
            group,
            icon: Icon::Search,
            label: label.into(),
            detail: String::new(),
            haystack: hay.into(),
        }
    }

    #[test]
    fn fuzzy_matches_subsequences() {
        assert!(fuzzy("nt", "New task").is_some());
        assert!(fuzzy("xyz", "New task").is_none());
        assert!(fuzzy("new", "New task") > fuzzy("nwk", "New task"));
    }

    #[test]
    fn filter_groups_and_modes() {
        let items = vec![
            item(Group::Action, "New task", ""),
            item(Group::Task, "#1 fix bug", "fix bug the parser"),
            item(Group::Message, "ada", "the parser crashed"),
        ];
        // Empty query: actions only.
        assert_eq!(filter(&items, "", Mode::Palette, 8), vec![0]);
        // One char does not search content.
        assert!(!filter(&items, "p", Mode::Palette, 8).contains(&2));
        assert_eq!(filter(&items, "parser", Mode::Palette, 8), vec![1, 2]);
        // Search mode excludes actions.
        assert_eq!(filter(&items, "new", Mode::Search, 8), Vec::<usize>::new());
    }
}
