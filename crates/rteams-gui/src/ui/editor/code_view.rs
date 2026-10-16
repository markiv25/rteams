//! Code text widget: line-number gutter, syntect highlighting, current-line highlight,
//! find/replace and go-to-line bars, tab/auto-indent/toggle-comment editing helpers.
//!
//! Usage: keep one [`CodeView`] per open buffer and call [`CodeView::show`] every frame.
//! Cmd/Ctrl+F, Cmd/Ctrl+H, Cmd/Ctrl+G and Cmd/Ctrl+/ are handled internally while the
//! editor (or its find / go-to bar) has focus. Reported `cursor_line` / `cursor_col` are
//! 1-based.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

use eframe::egui::{
    self, text::CCursor, text::CCursorRange, text_edit::TextEditState, Align, Color32, FontId,
    Rect, Shape, TextBuffer, Vec2,
};

use super::find_panel::FindPanel;
use super::goto_line::GotoLine;
use super::highlight;

#[derive(Debug, Clone, Copy)]
pub struct CodeViewOptions {
    pub read_only: bool,
    pub word_wrap: bool,
    pub show_line_numbers: bool,
    pub font_size: f32,
    /// Force large-file mode (no highlighting, no wrapping). Also auto-enabled by size.
    pub large_file: bool,
}

impl Default for CodeViewOptions {
    fn default() -> Self {
        Self {
            read_only: false,
            word_wrap: false,
            show_line_numbers: true,
            font_size: 13.0,
            large_file: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CodeViewResponse {
    /// Text was modified this frame (typing, replace, comment toggle...).
    pub changed: bool,
    /// 1-based caret line.
    pub cursor_line: usize,
    /// 1-based caret column (in characters).
    pub cursor_col: usize,
}

const TAB_WIDTH: usize = 4;

// ----------------------------------------------------------------------------
// Pure text helpers (byte offsets everywhere).

fn char_to_byte(text: &str, ci: usize) -> usize {
    text.char_indices().nth(ci).map_or(text.len(), |(b, _)| b)
}

fn byte_to_char(text: &str, b: usize) -> usize {
    text[..b.min(text.len())].chars().count()
}

/// 1-based (line, column) of a char index.
pub fn line_col(text: &str, char_idx: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in text.chars().enumerate() {
        if i >= char_idx {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

fn line_start(text: &str, b: usize) -> usize {
    text[..b].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, b: usize) -> usize {
    text[b..].find('\n').map_or(text.len(), |i| b + i)
}

/// Result of an edit: new text plus selection (anchor, head) as byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub text: String,
    pub anchor: usize,
    pub head: usize,
}

/// What Enter inserts at `cursor`: newline + the current line's indentation, one level deeper
/// after an opening bracket (and a split `{|}` becomes a three-line block).
/// Returns (inserted text, caret offset inside it).
pub fn enter_insertion(text: &str, cursor: usize, lang: &str) -> (String, usize) {
    let ls = line_start(text, cursor);
    let before = &text[ls..cursor];
    let indent: String = before
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    let prev = before.trim_end().chars().last();
    let next = text[cursor..].chars().next();
    let deeper = matches!(prev, Some('{' | '(' | '['))
        || (matches!(lang, "py" | "yaml") && prev == Some(':'));
    let unit = " ".repeat(TAB_WIDTH);
    if deeper {
        let inner = format!("\n{indent}{unit}");
        let closes = matches!(
            (prev, next),
            (Some('{'), Some('}')) | (Some('('), Some(')')) | (Some('['), Some(']'))
        );
        if closes {
            let s = format!("{inner}\n{indent}");
            return (s, inner.len());
        }
        let n = inner.len();
        return (inner, n);
    }
    let s = format!("\n{indent}");
    let n = s.len();
    (s, n)
}

/// Tab / Shift+Tab. Without a multi-line selection Tab inserts spaces up to the next tab stop
/// and Shift+Tab removes up to one level of indentation from the line.
pub fn tab_edit(text: &str, a: usize, b: usize, dedent: bool) -> Edit {
    let (s, e) = (a.min(b), a.max(b));
    let multi = text[s..e].contains('\n');
    if !dedent && !multi {
        let col = text[line_start(text, s)..s].chars().count();
        let n = TAB_WIDTH - col % TAB_WIDTH;
        let mut t = String::with_capacity(text.len() + n);
        t.push_str(&text[..s]);
        t.push_str(&" ".repeat(n));
        t.push_str(&text[e..]);
        return Edit {
            text: t,
            anchor: s + n,
            head: s + n,
        };
    }
    // line-wise
    let first = line_start(text, s);
    let last_end = line_end(text, e);
    let mut out = String::with_capacity(text.len() + 16);
    out.push_str(&text[..first]);
    let (mut new_s, mut new_e) = (s as isize, e as isize);
    let mut pos = first;
    for (i, line) in text[first..last_end].split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            pos += 1;
        }
        let delta: isize = if dedent {
            let strip = line
                .chars()
                .take(TAB_WIDTH)
                .take_while(|c| *c == ' ')
                .count()
                .max(usize::from(line.starts_with('\t')));
            out.push_str(&line[strip..]);
            -(strip as isize)
        } else if line.is_empty() {
            0
        } else {
            out.push_str(&" ".repeat(TAB_WIDTH));
            out.push_str(line);
            TAB_WIDTH as isize
        };
        if (pos as isize) < s as isize || i == 0 {
            new_s += if dedent {
                delta.max(-((s - first) as isize))
            } else {
                delta
            };
        }
        if pos as isize <= e as isize && (pos < e || i == 0) {
            new_e += delta;
        }
        pos += line.len();
    }
    out.push_str(&text[last_end..]);
    let clamp = |v: isize| (v.max(0) as usize).min(out.len());
    Edit {
        anchor: clamp(new_s),
        head: clamp(new_e),
        text: out,
    }
}

/// Toggle line comments on every line touched by the selection.
pub fn toggle_comment(text: &str, a: usize, b: usize, prefix: &str) -> Edit {
    let (s, e) = (a.min(b), a.max(b));
    let first = line_start(text, s);
    // a selection ending right after a newline does not include the next line
    let eff_end = if e > s && text[..e].ends_with('\n') {
        e - 1
    } else {
        e
    };
    let last_end = line_end(text, eff_end);
    let region = &text[first..last_end];
    let all_commented = region
        .split('\n')
        .filter(|l| !l.trim().is_empty())
        .all(|l| l.trim_start().starts_with(prefix));
    let mut out = String::from(&text[..first]);
    let mut delta_total: isize = 0;
    let mut delta_first: isize = 0;
    for (i, line) in region.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let indent_len = line.len() - line.trim_start().len();
        let (indent, rest) = line.split_at(indent_len);
        let d: isize = if rest.is_empty() {
            out.push_str(line);
            0
        } else if all_commented {
            let stripped = rest.strip_prefix(prefix).unwrap_or(rest);
            let stripped2 = stripped.strip_prefix(' ').unwrap_or(stripped);
            out.push_str(indent);
            out.push_str(stripped2);
            -((rest.len() - stripped2.len()) as isize)
        } else {
            out.push_str(indent);
            out.push_str(prefix);
            out.push(' ');
            out.push_str(rest);
            (prefix.len() + 1) as isize
        };
        if i == 0 {
            delta_first = d;
        }
        delta_total += d;
    }
    out.push_str(&text[last_end..]);
    let clamp = |v: isize| (v.max(0) as usize).min(out.len());
    Edit {
        anchor: clamp(s as isize + delta_first),
        head: clamp(e as isize + delta_total),
        text: out,
    }
}

/// Matching bracket for the one at (or just before) byte `idx`. Returns (bracket, partner).
pub fn match_bracket(text: &str, idx: usize) -> Option<(usize, usize)> {
    const LIMIT: usize = 20_000;
    let bytes = text.as_bytes();
    let cand = [idx, idx.wrapping_sub(1)];
    for &i in &cand {
        let Some(&c) = bytes.get(i) else { continue };
        let (open, close, fwd) = match c {
            b'(' => (b'(', b')', true),
            b'[' => (b'[', b']', true),
            b'{' => (b'{', b'}', true),
            b')' => (b'(', b')', false),
            b']' => (b'[', b']', false),
            b'}' => (b'{', b'}', false),
            _ => continue,
        };
        let mut depth = 0i32;
        if fwd {
            for (j, &x) in bytes.iter().enumerate().skip(i).take(LIMIT) {
                if x == open {
                    depth += 1;
                } else if x == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((i, j));
                    }
                }
            }
        } else {
            for j in (i.saturating_sub(LIMIT)..=i).rev() {
                let x = bytes[j];
                if x == close {
                    depth += 1;
                } else if x == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((i, j));
                    }
                }
            }
        }
    }
    None
}

// ----------------------------------------------------------------------------

struct Cached {
    key: (u64, String, bool, u32),
    job: egui::text::LayoutJob,
}

fn hash_str(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub struct CodeView {
    id: egui::Id,
    pub find: FindPanel,
    pub goto: GotoLine,
    cache: Option<Cached>,
    /// Selection to apply next frame: (anchor, head) as byte offsets.
    pending_select: Option<(usize, usize)>,
    pending_scroll: bool,
    pending_focus: bool,
    had_focus: bool,
    cursor_char: usize,
}

impl Default for CodeView {
    fn default() -> Self {
        Self {
            id: egui::Id::unique(("code_view", NEXT_ID.fetch_add(1, Ordering::Relaxed))),
            find: FindPanel::default(),
            goto: GotoLine::default(),
            cache: None,
            pending_select: None,
            pending_scroll: false,
            pending_focus: false,
            had_focus: false,
            cursor_char: 0,
        }
    }
}

impl CodeView {
    /// Select a byte range next frame (and scroll to it).
    pub fn select_bytes(&mut self, range: Range<usize>) {
        self.pending_select = Some((range.start, range.end));
        self.pending_scroll = true;
    }

    /// Move the caret to the start of a 1-based line next frame and focus the editor.
    pub fn goto_line(&mut self, text: &str, line: usize) {
        let mut b = 0;
        for _ in 1..line.max(1) {
            match text[b..].find('\n') {
                Some(i) => b += i + 1,
                None => break,
            }
        }
        self.pending_select = Some((b, b));
        self.pending_scroll = true;
        self.pending_focus = true;
    }

    fn apply_edit(&mut self, ctx: &egui::Context, text: &mut String, edit: Edit) {
        let a = byte_to_char(&edit.text, edit.anchor);
        let h = byte_to_char(&edit.text, edit.head);
        *text = edit.text;
        self.set_selection_chars(ctx, a, h);
    }

    fn set_selection_chars(&mut self, ctx: &egui::Context, a: usize, h: usize) {
        let mut st = TextEditState::load(ctx, self.id).unwrap_or_default();
        st.cursor.set_char_range(Some(CCursorRange {
            primary: CCursor::new(h),
            secondary: CCursor::new(a),
            h_pos: None,
        }));
        st.store(ctx, self.id);
        self.cursor_char = h;
    }

    /// Handle Tab / Enter / Cmd+/ before the TextEdit sees them.
    fn pre_edit(&mut self, ctx: &egui::Context, text: &mut String, lang: &str) -> bool {
        if !ctx.memory(|m| m.has_focus(self.id)) {
            return false;
        }
        let Some(range) = TextEditState::load(ctx, self.id).and_then(|s| s.cursor.char_range())
        else {
            return false;
        };
        let (a, b) = (
            char_to_byte(text, range.secondary.index.0),
            char_to_byte(text, range.primary.index.0),
        );
        let (tab, shift_tab, enter, comment) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
                i.consume_key(egui::Modifiers::SHIFT, egui::Key::Tab),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash),
            )
        });
        if tab || shift_tab {
            let e = tab_edit(text, a, b, shift_tab);
            self.apply_edit(ctx, text, e);
            return true;
        }
        if enter {
            let (s, e) = (a.min(b), a.max(b));
            let mut t = String::from(&text[..s]);
            let (ins, caret) = enter_insertion(text, s, lang);
            t.push_str(&ins);
            let pos = s + caret;
            t.push_str(&text[e..]);
            self.apply_edit(
                ctx,
                text,
                Edit {
                    text: t,
                    anchor: pos,
                    head: pos,
                },
            );
            return true;
        }
        if comment {
            if let Some(prefix) = highlight::comment_prefix(lang) {
                let e = toggle_comment(text, a, b, prefix);
                self.apply_edit(ctx, text, e);
                return true;
            }
        }
        false
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        text: &mut String,
        lang: &str,
        opts: &CodeViewOptions,
    ) -> CodeViewResponse {
        let ctx = ui.ctx().clone();
        let mut changed = false;
        let large = opts.large_file || highlight::is_large(text);
        let read_only = opts.read_only;

        // --- shortcuts (only while this editor or its bars are active)
        if self.had_focus || self.find.open || self.goto.open {
            let was_open = self.find.open;
            if self.find.handle_shortcuts(&ctx) && !was_open {
                if let Some(r) =
                    TextEditState::load(&ctx, self.id).and_then(|s| s.cursor.char_range())
                {
                    let (a, b) = (r.secondary.index.0, r.primary.index.0);
                    let sel: String = text
                        .chars()
                        .skip(a.min(b))
                        .take(a.max(b) - a.min(b))
                        .collect();
                    if !sel.is_empty() && !sel.contains('\n') {
                        self.find.query = sel;
                    }
                }
            }
            self.goto.handle_shortcut(&ctx);
        }
        let (cur_line, _) = line_col(text, self.cursor_char);
        let total_lines = text.bytes().filter(|b| *b == b'\n').count() + 1;

        // --- bars
        if self.find.open {
            let cursor_byte = char_to_byte(text, self.cursor_char);
            egui::Frame::new().inner_margin(4.0).show(ui, |ui| {
                let r = self.find.show(ui, text, cursor_byte, read_only);
                changed |= r.changed;
                if let Some(sel) = r.select {
                    self.select_bytes(sel);
                }
            });
        }
        if self.goto.open {
            egui::Frame::new().inner_margin(4.0).show(ui, |ui| {
                if let Some(line) = self.goto.show(ui, cur_line, total_lines) {
                    self.goto_line(text, line);
                }
            });
        }

        // --- keyboard edits
        if !read_only && self.pre_edit(&ctx, text, lang) {
            changed = true;
        }
        if let Some((a, b)) = self.pending_select.take() {
            let a = a.min(text.len());
            let b = b.min(text.len());
            let (ca, cb) = (byte_to_char(text, a), byte_to_char(text, b));
            self.set_selection_chars(&ctx, ca, cb);
            if self.pending_focus {
                ctx.memory_mut(|m| m.request_focus(self.id));
                self.pending_focus = false;
            }
        }

        // --- layout inputs
        let font = FontId::monospace(opts.font_size);
        let dark = ui.visuals().dark_mode;
        let wrap = opts.word_wrap && !large;
        let line_h = ui.fonts_mut(|f| f.row_height(&font));
        let digits = total_lines.to_string().len().max(3);
        let glyph_w = ui.fonts_mut(|f| f.glyph_width(&font, '0'));
        let gutter_w = if opts.show_line_numbers {
            digits as f32 * glyph_w + 16.0
        } else {
            0.0
        };
        let avail = ui.available_size();
        let rows = ((avail.y / line_h).ceil() as usize).max(1);

        let mut cache = self.cache.take();
        let lang_s = lang.to_string();
        let mut layouter = |ui: &egui::Ui, buf: &dyn TextBuffer, wrap_width: f32| {
            let s = buf.as_str();
            let key = (hash_str(s), lang_s.clone(), dark, opts.font_size.to_bits());
            let mut job = if large {
                highlight::layout_job(s, lang, &font, dark, true)
            } else {
                match &cache {
                    Some(c) if c.key == key => c.job.clone(),
                    _ => {
                        let job = highlight::layout_job(s, lang, &font, dark, false);
                        cache = Some(Cached {
                            key,
                            job: job.clone(),
                        });
                        job
                    }
                }
            };
            job.wrap.max_width = if wrap { wrap_width } else { f32::INFINITY };
            ui.fonts_mut(|f| f.layout_job(job))
        };

        let id = self.id;
        let mut ro_buf: &str = "";
        let out = egui::ScrollArea::both()
            .id_salt((id, "scroll"))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let viewport = ui.clip_rect();
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.add_space(gutter_w);
                    let width = if wrap {
                        (avail.x - gutter_w - 8.0).max(50.0)
                    } else {
                        f32::INFINITY
                    };
                    let bg_idx = ui.painter().add(Shape::Noop);
                    let buf: &mut dyn TextBuffer = if read_only {
                        ro_buf = text.as_str();
                        &mut ro_buf
                    } else {
                        &mut *text
                    };
                    let te = egui::TextEdit::multiline(buf)
                        .id(id)
                        .font(font.clone())
                        .frame(egui::Frame::NONE)
                        .desired_width(width)
                        .desired_rows(rows)
                        .lock_focus(true)
                        .layouter(&mut layouter)
                        .min_size(Vec2::new(0.0, avail.y))
                        .show(ui);
                    (te, bg_idx, viewport)
                })
                .inner
            })
            .inner;
        self.cache = cache;
        let (te, bg_idx, viewport) = out;
        let galley = te.galley.clone();
        let gpos = te.galley_pos;
        if te.response.response.changed() {
            changed = true;
        }
        self.had_focus = ui.memory(|m| m.has_focus(id));
        if let Some(r) = te.cursor_range {
            self.cursor_char = r.primary.index.0;
        }
        let cursor_char = self.cursor_char.min(text.chars().count());
        let (line, col) = line_col(text, cursor_char);

        // --- backgrounds: current line, find matches, bracket pair
        let mut shapes = Vec::new();
        let caret_rect = galley.pos_from_cursor(CCursor::new(cursor_char));
        if self.had_focus || !read_only {
            let y = gpos.y + caret_rect.min.y;
            let r =
                Rect::from_x_y_ranges(viewport.x_range(), y..=y + caret_rect.height().max(line_h));
            let c = if dark {
                Color32::from_white_alpha(10)
            } else {
                Color32::from_black_alpha(10)
            };
            shapes.push(Shape::rect_filled(r, 0.0, c));
        }
        let span_rect = |a: usize, b: usize, text: &str| -> Option<Rect> {
            let (ca, cb) = (byte_to_char(text, a), byte_to_char(text, b));
            let ra = galley.pos_from_cursor(CCursor::new(ca));
            let rb = galley.pos_from_cursor(CCursor::new(cb));
            if (ra.min.y - rb.min.y).abs() > 1.0 {
                return None;
            }
            Some(
                Rect::from_min_max(ra.min, egui::pos2(rb.min.x, ra.max.y))
                    .translate(gpos.to_vec2()),
            )
        };
        if self.find.open {
            let cur = self.find.current_range();
            let hl = Color32::from_rgba_unmultiplied(230, 190, 40, 60);
            let hl_cur = Color32::from_rgba_unmultiplied(240, 140, 30, 140);
            for m in self.find.matches().iter().take(2000) {
                let y0 = gpos.y;
                let _ = y0;
                if let Some(r) = span_rect(m.start, m.end, text) {
                    if r.max.y < viewport.min.y || r.min.y > viewport.max.y {
                        continue;
                    }
                    let c = if cur.as_ref() == Some(m) { hl_cur } else { hl };
                    shapes.push(Shape::rect_filled(r, 2.0, c));
                }
            }
        }
        if !large && text.len() < 400_000 {
            let cb = char_to_byte(text, cursor_char);
            if let Some((x, y)) = match_bracket(text, cb) {
                let c = Color32::from_rgba_unmultiplied(120, 160, 255, 70);
                for i in [x, y] {
                    if let Some(r) = span_rect(i, i + 1, text) {
                        shapes.push(Shape::rect_filled(r, 2.0, c));
                    }
                }
            }
        }
        ui.painter().set(bg_idx, Shape::Vec(shapes));

        // --- scroll to pending selection
        if self.pending_scroll {
            self.pending_scroll = false;
            let r = caret_rect.translate(gpos.to_vec2());
            ui.scroll_to_rect(r.expand2(Vec2::new(0.0, line_h * 3.0)), Some(Align::Center));
        }

        // --- gutter
        if opts.show_line_numbers {
            let painter = ui.painter();
            let bg = ui.visuals().extreme_bg_color;
            let gutter = Rect::from_min_max(
                egui::pos2(viewport.min.x, viewport.min.y),
                egui::pos2(viewport.min.x + gutter_w, viewport.max.y),
            );
            painter.rect_filled(gutter, 0.0, bg);
            let dim = ui.visuals().weak_text_color();
            let strong = ui.visuals().text_color();
            let mut n = 0usize;
            let mut at_line_start = true;
            for row in &galley.rows {
                if at_line_start {
                    n += 1;
                }
                let y = gpos.y + row.pos.y;
                let first = at_line_start;
                at_line_start = row.ends_with_newline;
                if !first || y + row.size.y < viewport.min.y || y > viewport.max.y {
                    continue;
                }
                painter.text(
                    egui::pos2(gutter.min.x + gutter_w - 8.0, y),
                    egui::Align2::RIGHT_TOP,
                    n.to_string(),
                    font.clone(),
                    if n == line { strong } else { dim },
                );
            }
            painter.vline(
                gutter.max.x,
                gutter.y_range(),
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
            );
        }

        CodeViewResponse {
            changed,
            cursor_line: line,
            cursor_col: col,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts() {
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
        assert_eq!(line_col("é\nx", 2), (2, 1));
    }

    #[test]
    fn enter_keeps_indent() {
        let (s, n) = enter_insertion("    foo", 7, "rs");
        assert_eq!(s, "\n    ");
        assert_eq!(n, s.len());
        let (s, _) = enter_insertion("fn a() {", 8, "rs");
        assert_eq!(s, "\n    ");
        let (s, n) = enter_insertion("x{}", 2, "rs");
        assert_eq!(s, "\n    \n");
        assert_eq!(n, 5);
        let (s, _) = enter_insertion("if x:", 5, "py");
        assert_eq!(s, "\n    ");
        let (s, _) = enter_insertion("if x:", 5, "rs");
        assert_eq!(s, "\n");
    }

    #[test]
    fn tab_inserts_to_stop() {
        let e = tab_edit("ab", 2, 2, false);
        assert_eq!(e.text, "ab  ");
        assert_eq!(e.head, 4);
        let e = tab_edit("", 0, 0, false);
        assert_eq!(e.text, "    ");
    }

    #[test]
    fn tab_indents_and_dedents_selection() {
        let t = "a\nb\nc";
        let e = tab_edit(t, 0, 3, false);
        assert_eq!(e.text, "    a\n    b\nc");
        let d = tab_edit(&e.text, 0, e.head, true);
        assert_eq!(d.text, "a\nb\nc");
        let d = tab_edit("  x", 3, 3, true);
        assert_eq!(d.text, "x");
        assert_eq!(d.head, 1);
    }

    #[test]
    fn comment_toggle() {
        let e = toggle_comment("a\n  b\n", 0, 5, "//");
        assert_eq!(e.text, "// a\n  // b\n");
        let back = toggle_comment(&e.text, 0, e.head, "//");
        assert_eq!(back.text, "a\n  b\n");
        // blank lines are skipped
        let e = toggle_comment("a\n\nb", 0, 4, "#");
        assert_eq!(e.text, "# a\n\n# b");
    }

    #[test]
    fn brackets() {
        let t = "f(a[1], {x})";
        assert_eq!(match_bracket(t, 1), Some((1, 11)));
        assert_eq!(match_bracket(t, 12), Some((11, 1)));
        assert_eq!(match_bracket(t, 0), None);
        assert_eq!(match_bracket("(", 0), None);
    }

    #[test]
    fn renders_headless() {
        let mut cv = CodeView::default();
        let mut text = "fn main() {\n    println!(\"hi\");\n}\n".repeat(30);
        let mut wrap = false;
        crate::view::testing::render(|ui| {
            let o = CodeViewOptions {
                word_wrap: wrap,
                ..Default::default()
            };
            let r = cv.show(ui, &mut text, "rs", &o);
            assert!(r.cursor_line >= 1 && r.cursor_col >= 1);
            wrap = !wrap;
        });
        cv.find.open(true);
        cv.find.query = "main".into();
        cv.goto_line(&text, 5);
        crate::view::testing::render(|ui| {
            cv.show(ui, &mut text, "rs", &CodeViewOptions::default());
        });
        crate::view::testing::render(|ui| {
            let o = CodeViewOptions {
                read_only: true,
                large_file: true,
                show_line_numbers: false,
                ..Default::default()
            };
            cv.show(ui, &mut text, "rs", &o);
        });
    }
}
