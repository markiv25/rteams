//! Go-to-line bar (Cmd/Ctrl+G). Accepts `N`, `+N`, `-N` (relative) and `N%`.

use eframe::egui;

/// Resolve user input to a 1-based line number clamped to `1..=total`.
pub fn parse_goto(input: &str, current: usize, total: usize) -> Option<usize> {
    let s = input.trim();
    if s.is_empty() || total == 0 {
        return None;
    }
    let line = if let Some(p) = s.strip_suffix('%') {
        let pct: f64 = p.trim().parse().ok()?;
        if !(0.0..=100.0).contains(&pct) {
            return None;
        }
        ((pct / 100.0) * total as f64).ceil() as i64
    } else if let Some(n) = s.strip_prefix('+') {
        current as i64 + n.trim().parse::<i64>().ok()?
    } else if let Some(n) = s.strip_prefix('-') {
        current as i64 - n.trim().parse::<i64>().ok()?
    } else {
        s.parse::<i64>().ok()?
    };
    Some(line.clamp(1, total as i64) as usize)
}

#[derive(Default)]
pub struct GotoLine {
    pub open: bool,
    input: String,
    focus: bool,
}

impl GotoLine {
    pub fn open(&mut self) {
        self.open = true;
        self.focus = true;
        self.input.clear();
    }

    /// Consume Cmd/Ctrl+G; returns true if the bar was opened.
    pub fn handle_shortcut(&mut self, ctx: &egui::Context) -> bool {
        let hit = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::G));
        if hit {
            self.open();
        }
        hit
    }

    /// Draw the bar; returns the 1-based line to jump to when confirmed.
    pub fn show(&mut self, ui: &mut egui::Ui, current: usize, total: usize) -> Option<usize> {
        if !self.open {
            return None;
        }
        let mut jump = None;
        ui.horizontal(|ui| {
            ui.label("Go to line");
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .desired_width(100.0)
                    .hint_text(format!("1-{total}")),
            );
            if self.focus {
                r.request_focus();
                self.focus = false;
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if enter {
                jump = parse_goto(&self.input, current, total);
                self.open = false;
            } else if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.open = false;
            }
            if ui.small_button("x").clicked() {
                self.open = false;
            }
        });
        jump
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_forms() {
        assert_eq!(parse_goto("5", 1, 100), Some(5));
        assert_eq!(parse_goto("+10", 20, 100), Some(30));
        assert_eq!(parse_goto("-5", 20, 100), Some(15));
        assert_eq!(parse_goto("50%", 1, 200), Some(100));
        assert_eq!(parse_goto("999", 1, 100), Some(100));
        assert_eq!(parse_goto("0", 1, 100), Some(1));
        assert_eq!(parse_goto("-500", 3, 100), Some(1));
        assert_eq!(parse_goto("abc", 1, 100), None);
        assert_eq!(parse_goto("", 1, 100), None);
        assert_eq!(parse_goto("120%", 1, 100), None);
    }

    #[test]
    fn renders() {
        let mut g = GotoLine::default();
        g.open();
        crate::view::testing::render(|ui| {
            g.show(ui, 1, 10);
        });
    }
}
