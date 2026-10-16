//! Paints a terminal screen with the egui painter and feeds keyboard / mouse input back to the
//! session.

use eframe::egui::{
    self, Align2, Color32, EventFilter, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
};
use rteams_services::terminal::{Color, ScreenSnapshot, TerminalSession, TerminalSettings};

use super::keys;

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub fg: Color32,
    pub bg: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

pub fn palette(theme: &str) -> Palette {
    if theme == "light" {
        Palette {
            fg: rgb(0x24, 0x29, 0x2e),
            bg: rgb(0xfa, 0xfa, 0xfa),
            cursor: rgb(0x44, 0x4d, 0x56),
            selection: Color32::from_rgba_unmultiplied(0x3b, 0x82, 0xf6, 90),
            ansi: [
                rgb(0x24, 0x29, 0x2e),
                rgb(0xcd, 0x31, 0x31),
                rgb(0x0b, 0x7a, 0x3e),
                rgb(0x94, 0x6b, 0x00),
                rgb(0x1d, 0x4e, 0xd8),
                rgb(0xa6, 0x26, 0xa4),
                rgb(0x0e, 0x74, 0x90),
                rgb(0x6a, 0x73, 0x7d),
                rgb(0x58, 0x60, 0x69),
                rgb(0xe4, 0x4c, 0x4c),
                rgb(0x16, 0xa3, 0x4a),
                rgb(0xb4, 0x83, 0x00),
                rgb(0x3b, 0x82, 0xf6),
                rgb(0xc0, 0x4d, 0xc0),
                rgb(0x06, 0xb6, 0xd4),
                rgb(0x95, 0x9d, 0xa5),
            ],
        }
    } else {
        Palette {
            fg: rgb(0xd4, 0xd4, 0xd4),
            bg: rgb(0x0c, 0x0d, 0x13),
            cursor: rgb(0xe2, 0xe8, 0xf0),
            selection: Color32::from_rgba_unmultiplied(0x81, 0x8c, 0xf8, 90),
            ansi: [
                rgb(0x1e, 0x1e, 0x1e),
                rgb(0xef, 0x44, 0x44),
                rgb(0x22, 0xc5, 0x5e),
                rgb(0xea, 0xb3, 0x08),
                rgb(0x3b, 0x82, 0xf6),
                rgb(0xc0, 0x84, 0xfc),
                rgb(0x22, 0xd3, 0xee),
                rgb(0xd4, 0xd4, 0xd4),
                rgb(0x64, 0x74, 0x8b),
                rgb(0xf8, 0x71, 0x71),
                rgb(0x4a, 0xde, 0x80),
                rgb(0xfa, 0xcc, 0x15),
                rgb(0x60, 0xa5, 0xfa),
                rgb(0xd8, 0xb4, 0xfe),
                rgb(0x67, 0xe8, 0xf9),
                rgb(0xf8, 0xfa, 0xfc),
            ],
        }
    }
}

/// Resolve a terminal color; `default` is used for `Color::Default`.
pub fn resolve(c: Color, default: Color32, pal: &Palette) -> Color32 {
    match c {
        Color::Default => default,
        Color::Rgb(r, g, b) => rgb(r, g, b),
        Color::Idx(i) if i < 16 => pal.ansi[i as usize],
        Color::Idx(i) if i < 232 => {
            let n = i - 16;
            let level = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
            rgb(level(n / 36), level((n / 6) % 6), level(n % 6))
        }
        Color::Idx(i) => {
            let v = 8 + 10 * (i - 232);
            rgb(v, v, v)
        }
    }
}

/// (row, col) cell positions of a drag selection, in the order they were made.
pub type Selection = ((u16, u16), (u16, u16));

#[derive(Default)]
pub struct WidgetState {
    pub selection: Option<Selection>,
    selecting: bool,
    wheel_remainder: f32,
    cache: Option<(u64, (u16, u16), ScreenSnapshot)>,
    pub last: ScreenSnapshot,
}

impl WidgetState {
    pub fn selected_text(&self) -> Option<String> {
        let (a, b) = self.selection?;
        if a == b {
            return None;
        }
        Some(self.last.text_range(a, b))
    }
}

pub fn font_id(settings: &TerminalSettings) -> FontId {
    FontId::monospace(settings.font_size)
}

pub fn cell_size(ctx: &egui::Context, font: &FontId) -> Vec2 {
    ctx.fonts_mut(|f| {
        Vec2::new(
            f.glyph_width(font, 'M').max(1.0),
            f.row_height(font).max(1.0),
        )
    })
}

/// Options for `paint`.
pub struct PaintOpts<'a> {
    pub pal: &'a Palette,
    pub font: &'a FontId,
    pub cell: Vec2,
    pub selection: Option<Selection>,
    pub focused: bool,
    pub bg: Option<Color32>,
}

/// Paint `snap` into `rect`. Pure drawing, no input; usable headless.
pub fn paint(painter: &egui::Painter, rect: Rect, snap: &ScreenSnapshot, o: &PaintOpts) {
    let PaintOpts {
        pal,
        font,
        cell,
        selection,
        focused,
        bg,
    } = *o;
    if let Some(bg) = bg {
        painter.rect_filled(rect, 0.0, bg);
    }
    let at = |r: u16, c: u16| {
        Pos2::new(
            rect.min.x + c as f32 * cell.x,
            rect.min.y + r as f32 * cell.y,
        )
    };

    // Backgrounds first, merging runs of equal color.
    for r in 0..snap.rows {
        let mut run: Option<(u16, Color32)> = None;
        for c in 0..=snap.cols {
            let color = snap.cell(r, c).and_then(|cl| {
                if cl.inverse {
                    Some(resolve(cl.fg, pal.fg, pal))
                } else if cl.bg != Color::Default {
                    Some(resolve(cl.bg, pal.bg, pal))
                } else {
                    None
                }
            });
            match (run, color) {
                (Some((_, rc)), Some(nc)) if rc == nc => {}
                _ => {
                    if let Some((start, rc)) = run.take() {
                        painter.rect_filled(
                            Rect::from_min_max(at(r, start), at(r + 1, c)),
                            0.0,
                            rc,
                        );
                    }
                    if let Some(nc) = color {
                        run = Some((c, nc));
                    }
                }
            }
        }
    }

    // Text in runs of identical style.
    for r in 0..snap.rows {
        let mut text = String::new();
        let mut start = 0u16;
        let mut style: Option<(Color32, bool)> = None;
        let flush = |text: &mut String, start: u16, style: Option<(Color32, bool)>| {
            let trimmed = text.trim_end();
            if let (Some((color, underline)), false) = (style, trimmed.is_empty()) {
                let galley = painter.layout_no_wrap(trimmed.to_string(), font.clone(), color);
                let pos = at(r, start);
                let w = galley.size().x;
                painter.galley(pos, galley, color);
                if underline {
                    let y = pos.y + cell.y - 1.5;
                    painter.line_segment(
                        [Pos2::new(pos.x, y), Pos2::new(pos.x + w, y)],
                        Stroke::new(1.0, color),
                    );
                }
            }
            text.clear();
        };
        for c in 0..snap.cols {
            let Some(cl) = snap.cell(r, c) else { continue };
            if cl.continuation {
                continue;
            }
            let mut fg = if cl.inverse {
                resolve(cl.bg, pal.bg, pal)
            } else {
                resolve(cl.fg, pal.fg, pal)
            };
            if let (true, false, Color::Idx(i)) = (cl.bold, cl.inverse, cl.fg) {
                if i < 8 {
                    fg = pal.ansi[i as usize + 8];
                }
            }
            if cl.dim {
                fg = fg.gamma_multiply(0.6);
            }
            if cl.text.is_empty() {
                if style.is_some() {
                    text.push(' ');
                }
                continue;
            }
            let st = Some((fg, cl.underline));
            if st != style || cl.wide {
                flush(&mut text, start, style);
                style = st;
                start = c;
            }
            text.push_str(&cl.text);
            if cl.wide {
                flush(&mut text, start, style);
                style = None;
            }
        }
        flush(&mut text, start, style);
    }

    // Selection.
    if let Some((a, b)) = selection.filter(|(a, b)| a != b) {
        let (s, e) = if a <= b { (a, b) } else { (b, a) };
        for r in s.0..=e.0.min(snap.rows.saturating_sub(1)) {
            let c0 = if r == s.0 { s.1 } else { 0 };
            let c1 = if r == e.0 {
                e.1
            } else {
                snap.cols.saturating_sub(1)
            };
            painter.rect_filled(
                Rect::from_min_max(at(r, c0), at(r + 1, c1 + 1)),
                0.0,
                pal.selection,
            );
        }
    }

    // Cursor.
    if snap.cursor_visible && snap.cursor.0 < snap.rows && snap.cursor.1 < snap.cols {
        let (r, c) = snap.cursor;
        let cr = Rect::from_min_size(at(r, c), cell);
        if focused {
            painter.rect_filled(cr, 0.0, pal.cursor);
            if let Some(cl) = snap.cell(r, c).filter(|cl| !cl.text.is_empty()) {
                painter.text(cr.min, Align2::LEFT_TOP, &cl.text, font.clone(), pal.bg);
            }
        } else {
            painter.rect_stroke(cr, 0.0, Stroke::new(1.0, pal.cursor), StrokeKind::Inside);
        }
    }
}

fn cell_at(rect: Rect, cell: Vec2, snap: &ScreenSnapshot, p: Pos2) -> (u16, u16) {
    let c = ((p.x - rect.min.x) / cell.x).floor().max(0.0) as u16;
    let r = ((p.y - rect.min.y) / cell.y).floor().max(0.0) as u16;
    (
        r.min(snap.rows.saturating_sub(1)),
        c.min(snap.cols.saturating_sub(1)),
    )
}

/// Draw the live terminal for `session` filling the available space, handle input.
/// Returns the response (use `.has_focus()` / `.clicked()`).
pub fn show(
    ui: &mut egui::Ui,
    id_salt: usize,
    session: &mut TerminalSession,
    state: &mut WidgetState,
    settings: &TerminalSettings,
    bg: Option<Color32>,
) -> egui::Response {
    let font = font_id(settings);
    let cell = cell_size(ui.ctx(), &font);
    let size = ui.available_size().max(Vec2::splat(20.0));
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let id = ui.make_persistent_id(("terminal_widget", id_salt));
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    let focused = resp.has_focus();
    let pal = palette(&settings.theme);

    let cols = ((rect.width() / cell.x).floor() as u16).max(2);
    let rows = ((rect.height() / cell.y).floor() as u16).max(1);
    session.resize(rows, cols);

    let key = (session.generation(), (rows, cols));
    let stale = state
        .cache
        .as_ref()
        .is_none_or(|(g, sz, _)| (*g, *sz) != key);
    if stale {
        let snap = session.screen();
        state.cache = Some((key.0, key.1, snap));
    }
    let snap = state
        .cache
        .as_ref()
        .map(|c| c.2.clone())
        .unwrap_or_default();
    state.last = snap.clone();

    if focused {
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                id,
                EventFilter {
                    tab: true,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    escape: true,
                },
            )
        });
        let events = ui.input(|i| i.events.clone());
        let t = keys::translate(&events, snap.app_cursor, snap.bracketed_paste);
        if t.copy {
            match state.selected_text() {
                Some(text) => ui.ctx().copy_text(text),
                None if !cfg!(target_os = "macos") => session.write(&[3]),
                None => {}
            }
        }
        if !t.bytes.is_empty() {
            state.selection = None;
            session.write(&t.bytes);
        }
    }

    // Wheel: scrollback, or cursor keys in alt-screen programs.
    if resp.hovered() {
        let dy = ui.input(|i| i.smooth_scroll_delta.y);
        if dy != 0.0 {
            state.wheel_remainder += dy / cell.y;
            let lines = state.wheel_remainder.trunc();
            state.wheel_remainder -= lines;
            if lines != 0.0 {
                if snap.alt_screen {
                    session.write(&keys::wheel_arrows(
                        lines > 0.0,
                        lines.abs() as usize,
                        snap.app_cursor,
                    ));
                } else {
                    session.scroll(lines as isize);
                }
            }
        }
    }

    // Selection by dragging.
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() || resp.clicked() {
            let c = cell_at(rect, cell, &snap, p);
            state.selection = Some((c, c));
            state.selecting = true;
        } else if resp.dragged() && state.selecting {
            let c = cell_at(rect, cell, &snap, p);
            if let Some((a, _)) = state.selection {
                state.selection = Some((a, c));
            }
        }
    }
    if resp.drag_stopped() {
        state.selecting = false;
    }

    let painter = ui.painter_at(rect);
    let opts = PaintOpts {
        pal: &pal,
        font: &font,
        cell,
        selection: state.selection,
        focused,
        bg,
    };
    paint(&painter, rect, &snap, &opts);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::terminal::session::Cell;

    pub fn canned(text: &str, rows: u16, cols: u16) -> ScreenSnapshot {
        let mut cells = vec![Cell::default(); rows as usize * cols as usize];
        for (r, line) in text.lines().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                let cl = &mut cells[r * cols as usize + c];
                cl.text = ch.to_string();
                if r == 0 {
                    cl.fg = Color::Idx(2);
                    cl.bold = true;
                }
                if r == 1 {
                    cl.bg = Color::Rgb(10, 20, 30);
                    cl.underline = true;
                }
            }
        }
        ScreenSnapshot {
            rows,
            cols,
            cells,
            cursor: (2, 1),
            cursor_visible: true,
            ..Default::default()
        }
    }

    #[test]
    fn palette_resolution() {
        let p = palette("dark");
        assert_eq!(resolve(Color::Default, p.fg, &p), p.fg);
        assert_eq!(resolve(Color::Idx(1), p.fg, &p), p.ansi[1]);
        assert_eq!(resolve(Color::Idx(16), p.fg, &p), rgb(0, 0, 0));
        assert_eq!(resolve(Color::Idx(231), p.fg, &p), rgb(255, 255, 255));
        assert_eq!(resolve(Color::Idx(232), p.fg, &p), rgb(8, 8, 8));
        assert_eq!(resolve(Color::Rgb(1, 2, 3), p.fg, &p), rgb(1, 2, 3));
    }

    #[test]
    fn headless_render_canned_screen() {
        let snap = canned("hello\nworld wide\nabc", 5, 20);
        let settings = TerminalSettings::default();
        let font = font_id(&settings);
        let pal = palette("dark");
        let mut sel = Some(((0, 1), (1, 3)));
        crate::view::testing::render(|ui| {
            let cell = cell_size(ui.ctx(), &font);
            let (rect, _) = ui.allocate_exact_size(Vec2::new(300.0, 100.0), Sense::hover());
            let on = PaintOpts {
                pal: &pal,
                font: &font,
                cell,
                selection: sel,
                focused: true,
                bg: Some(pal.bg),
            };
            let off = PaintOpts {
                pal: &pal,
                font: &font,
                cell,
                selection: None,
                focused: false,
                bg: None,
            };
            paint(&ui.painter_at(rect), rect, &snap, &on);
            paint(&ui.painter_at(rect), rect, &snap, &off);
            sel = sel.take();
        });
        assert_eq!(snap.text_range((0, 1), (1, 3)), "ello\nworl");
    }

    #[test]
    fn live_widget_renders() {
        let args = vec!["-c".to_string(), "printf hi".to_string()];
        let mut s = TerminalSession::spawn_command(
            &std::env::temp_dir(),
            "/bin/sh",
            &args,
            rteams_services::terminal::TermSize::default(),
        )
        .unwrap();
        let mut st = WidgetState::default();
        let settings = TerminalSettings::default();
        crate::view::testing::render(|ui| {
            show(ui, 0, &mut s, &mut st, &settings, None);
        });
        assert!(st.last.rows >= 1);
    }
}
