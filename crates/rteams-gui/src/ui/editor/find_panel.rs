//! Find / replace bar (Cmd/Ctrl+F, Cmd/Ctrl+H). The matching logic is pure and tested.

use std::ops::Range;

use eframe::egui;
use regex::RegexBuilder;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
}

fn build_regex(query: &str, o: &FindOptions) -> Result<regex::Regex, String> {
    let mut pat = if o.regex {
        query.to_string()
    } else {
        regex::escape(query)
    };
    if o.whole_word {
        pat = format!(r"\b(?:{pat})\b");
    }
    RegexBuilder::new(&pat)
        .case_insensitive(!o.case_sensitive)
        .multi_line(true)
        .build()
        .map_err(|e| {
            e.to_string()
                .lines()
                .last()
                .unwrap_or("invalid regex")
                .to_string()
        })
}

/// All non-empty matches as byte ranges. Empty query gives no matches.
pub fn find_matches(text: &str, query: &str, o: &FindOptions) -> Result<Vec<Range<usize>>, String> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let re = build_regex(query, o)?;
    Ok(re
        .find_iter(text)
        .filter(|m| !m.is_empty())
        .map(|m| m.range())
        .collect())
}

/// Replace every match; returns the new text and the replacement count.
/// In regex mode `$1` style group references are expanded.
pub fn replace_all(
    text: &str,
    query: &str,
    replacement: &str,
    o: &FindOptions,
) -> Result<(String, usize), String> {
    if query.is_empty() {
        return Ok((text.to_string(), 0));
    }
    let re = build_regex(query, o)?;
    let mut n = 0;
    let out = re.replace_all(text, |caps: &regex::Captures| {
        n += 1;
        if o.regex {
            let mut s = String::new();
            caps.expand(replacement, &mut s);
            s
        } else {
            replacement.to_string()
        }
    });
    Ok((out.into_owned(), n))
}

/// Replace the single match at `range` (computed against `text`); returns the new text.
pub fn replace_range(
    text: &str,
    range: Range<usize>,
    query: &str,
    replacement: &str,
    o: &FindOptions,
) -> Result<(String, usize), String> {
    let re = build_regex(query, o)?;
    let slice = &text[range.clone()];
    let rep = if o.regex {
        let caps = re
            .captures_at(text, range.start)
            .filter(|c| c.get(0).map(|m| m.range()) == Some(range.clone()));
        match caps {
            Some(c) => {
                let mut s = String::new();
                c.expand(replacement, &mut s);
                s
            }
            None => replacement.to_string(),
        }
    } else {
        let _ = slice;
        replacement.to_string()
    };
    let mut out = String::with_capacity(text.len());
    out.push_str(&text[..range.start]);
    out.push_str(&rep);
    let end = out.len();
    out.push_str(&text[range.end..]);
    Ok((out, end))
}

/// Index of the match to move to, wrapping around. `from` is the cursor byte offset.
pub fn next_index(matches: &[Range<usize>], from: usize, forward: bool) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    if forward {
        Some(matches.iter().position(|m| m.start >= from).unwrap_or(0))
    } else {
        Some(
            matches
                .iter()
                .rposition(|m| m.end <= from)
                .unwrap_or(matches.len() - 1),
        )
    }
}

#[derive(Debug, Default)]
pub struct FindResponse {
    /// The text was modified (replace / replace all).
    pub changed: bool,
    /// Select this byte range in the editor and scroll to it.
    pub select: Option<Range<usize>>,
}

#[derive(Default)]
pub struct FindPanel {
    pub open: bool,
    pub show_replace: bool,
    pub query: String,
    pub replacement: String,
    pub opts: FindOptions,
    matches: Vec<Range<usize>>,
    current: Option<usize>,
    error: Option<String>,
    focus: bool,
    /// (text hash, query, opts) the match list was computed for.
    key: Option<(u64, String, FindOptions)>,
    status: String,
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

impl FindPanel {
    pub fn open(&mut self, replace: bool) {
        self.open = true;
        self.show_replace = replace;
        self.focus = true;
    }

    /// Consume Cmd/Ctrl+F and Cmd/Ctrl+H. Call only while the editor has focus (or the panel is open).
    pub fn handle_shortcuts(&mut self, ctx: &egui::Context) -> bool {
        let (f, h) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::F),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::H),
            )
        });
        if f {
            self.open(self.show_replace);
        } else if h {
            self.open(true);
        }
        f || h
    }

    /// The match last navigated to (byte range), if still valid.
    pub fn current_range(&self) -> Option<Range<usize>> {
        if !self.open {
            return None;
        }
        self.current.and_then(|c| self.matches.get(c).cloned())
    }

    /// Current match ranges (for painting highlights).
    pub fn matches(&self) -> &[Range<usize>] {
        if self.open {
            &self.matches
        } else {
            &[]
        }
    }

    fn recompute(&mut self, text: &str) {
        let key = (hash_str(text), self.query.clone(), self.opts);
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        match find_matches(text, &self.query, &self.opts) {
            Ok(m) => {
                self.matches = m;
                self.error = None;
            }
            Err(e) => {
                self.matches.clear();
                self.error = Some(e);
            }
        }
        if self.current.is_some_and(|c| c >= self.matches.len()) {
            self.current = None;
        }
    }

    pub fn step(&mut self, text: &str, cursor_byte: usize, forward: bool) -> Option<Range<usize>> {
        self.recompute(text);
        let from = match (self.current, forward) {
            (Some(c), true) => self.matches.get(c).map_or(cursor_byte, |m| m.end),
            (Some(c), false) => self.matches.get(c).map_or(cursor_byte, |m| m.start),
            (None, _) => cursor_byte,
        };
        let i = next_index(&self.matches, from, forward)?;
        self.current = Some(i);
        Some(self.matches[i].clone())
    }

    pub fn replace_current(&mut self, text: &mut String) -> FindResponse {
        self.recompute(text);
        let mut r = FindResponse::default();
        let Some(i) = self.current.filter(|i| *i < self.matches.len()) else {
            return r;
        };
        let range = self.matches[i].clone();
        if let Ok((new, end)) =
            replace_range(text, range, &self.query, &self.replacement, &self.opts)
        {
            *text = new;
            r.changed = true;
            self.key = None;
            self.current = None;
            r.select = self.step(text, end, true);
        }
        r
    }

    pub fn replace_everything(&mut self, text: &mut String) -> FindResponse {
        let mut r = FindResponse::default();
        if let Ok((new, n)) = replace_all(text, &self.query, &self.replacement, &self.opts) {
            if n > 0 {
                *text = new;
                r.changed = true;
            }
            self.status = format!("{n} replaced");
            self.key = None;
            self.current = None;
        }
        r
    }

    /// Draw the bar. `cursor_byte` is the editor caret as a byte offset.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &mut String,
        cursor_byte: usize,
        read_only: bool,
    ) -> FindResponse {
        let mut resp = FindResponse::default();
        if !self.open {
            return resp;
        }
        self.recompute(text);
        let mut enter_next = None;
        ui.horizontal_wrapped(|ui| {
            let q = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .desired_width(180.0)
                    .hint_text("Find"),
            );
            if self.focus {
                q.request_focus();
                self.focus = false;
            }
            if q.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                enter_next = Some(!ui.input(|i| i.modifiers.shift));
                q.request_focus();
            }
            if q.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.open = false;
            }
            ui.toggle_value(&mut self.opts.case_sensitive, "Aa")
                .on_hover_text("Match case");
            ui.toggle_value(&mut self.opts.whole_word, "W")
                .on_hover_text("Whole word");
            ui.toggle_value(&mut self.opts.regex, ".*")
                .on_hover_text("Regular expression");
            let label = match (&self.error, self.current) {
                (Some(e), _) => e.clone(),
                (None, Some(c)) => format!("{} of {}", c + 1, self.matches.len()),
                (None, None) if self.query.is_empty() => String::new(),
                (None, None) => format!("{} found", self.matches.len()),
            };
            ui.weak(label);
            if ui.small_button("Prev").clicked() {
                enter_next = Some(false);
            }
            if ui.small_button("Next").clicked() {
                enter_next = Some(true);
            }
            ui.toggle_value(&mut self.show_replace, "Replace");
            if ui.small_button("x").clicked() {
                self.open = false;
            }
        });
        if self.show_replace && !read_only {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.replacement)
                        .desired_width(180.0)
                        .hint_text("Replace"),
                );
                if ui.small_button("Replace").clicked() {
                    let r = self.replace_current(text);
                    resp.changed |= r.changed;
                    resp.select = r.select.or(resp.select.take());
                }
                if ui.small_button("Replace all").clicked() {
                    let r = self.replace_everything(text);
                    resp.changed |= r.changed;
                }
                if !self.status.is_empty() {
                    ui.weak(&self.status);
                }
            });
        }
        if let Some(fwd) = enter_next {
            self.status.clear();
            if let Some(r) = self.step(text, cursor_byte, fwd) {
                resp.select = Some(r);
            }
        }
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const O: FindOptions = FindOptions {
        regex: false,
        case_sensitive: false,
        whole_word: false,
    };

    #[test]
    fn plain_and_case() {
        let t = "Foo foo FOO";
        assert_eq!(find_matches(t, "foo", &O).unwrap().len(), 3);
        let cs = FindOptions {
            case_sensitive: true,
            ..O
        };
        assert_eq!(find_matches(t, "foo", &cs).unwrap(), vec![4..7]);
        assert!(find_matches(t, "", &O).unwrap().is_empty());
    }

    #[test]
    fn whole_word_and_escape() {
        let w = FindOptions {
            whole_word: true,
            ..O
        };
        assert_eq!(
            find_matches("cat concat cat.", "cat", &w).unwrap(),
            vec![0..3, 11..14]
        );
        assert_eq!(find_matches("a.b axb", "a.b", &O).unwrap(), vec![0..3]);
    }

    #[test]
    fn regex_and_error() {
        let r = FindOptions { regex: true, ..O };
        assert_eq!(
            find_matches("a1 b22", r"\d+", &r).unwrap(),
            vec![1..2, 4..6]
        );
        assert!(find_matches("x", "(", &r).is_err());
    }

    #[test]
    fn replace_all_counts_and_groups() {
        assert_eq!(
            replace_all("a a a", "a", "bb", &O).unwrap(),
            ("bb bb bb".into(), 3)
        );
        let r = FindOptions { regex: true, ..O };
        assert_eq!(
            replace_all("k=1,j=2", r"(\w)=(\d)", "$2:$1", &r).unwrap(),
            ("1:k,2:j".into(), 2)
        );
        // literal `$` is not expanded in plain mode
        assert_eq!(replace_all("a", "a", "$1", &O).unwrap().0, "$1");
    }

    #[test]
    fn replace_one_and_unicode() {
        let (t, end) = replace_range("héllo héllo", 7..13, "héllo", "X", &O).unwrap();
        assert_eq!(t, "héllo X");
        assert_eq!(end, t.len());
    }

    #[test]
    fn next_prev_wrap() {
        let m = vec![2..3, 10..11, 20..21];
        assert_eq!(next_index(&m, 0, true), Some(0));
        assert_eq!(next_index(&m, 3, true), Some(1));
        assert_eq!(next_index(&m, 22, true), Some(0));
        assert_eq!(next_index(&m, 10, false), Some(0));
        assert_eq!(next_index(&m, 1, false), Some(2));
        assert_eq!(next_index(&[], 0, true), None);
    }

    #[test]
    fn panel_flow() {
        let mut p = FindPanel {
            query: "a".into(),
            ..Default::default()
        };
        let mut text = String::from("a b a");
        assert_eq!(p.step(&text, 0, true), Some(0..1));
        assert_eq!(p.step(&text, 0, true), Some(4..5));
        assert_eq!(p.step(&text, 0, true), Some(0..1));
        p.replacement = "zz".into();
        let r = p.replace_current(&mut text);
        assert!(r.changed);
        assert_eq!(text, "zz b a");
        assert_eq!(r.select, Some(5..6));
        let r = p.replace_everything(&mut text);
        assert!(r.changed);
        assert_eq!(text, "zz b zz");
    }

    #[test]
    fn renders() {
        let mut p = FindPanel::default();
        p.open(true);
        p.query = "x".into();
        let mut t = String::from("xyz");
        crate::view::testing::render(|ui| {
            p.show(ui, &mut t, 0, false);
        });
    }
}
