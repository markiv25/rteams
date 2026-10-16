//! Rich text for message bodies: `#<task id>` becomes a link that opens the task, `@name` a
//! highlighted member mention that opens the member. The tokenizer is pure and tested; the
//! renderer only decides, per token, whether the target exists.

use eframe::egui::{self, Color32, RichText, Sense, Ui};
use rteams_core::Task;

use crate::theme;
use crate::view::{Intent, Intents, View};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Text(String),
    /// `#abc123`, without the `#`.
    TaskRef(String),
    /// `@ada`, without the `@`.
    Mention(String),
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Split `text` into plain runs, `#id` references and `@name` mentions. A marker only counts at
/// the start of the text or after a non-word character (so `a@b.com` and `x#1` stay plain), and
/// needs at least one name character after it. Concatenating the tokens (with markers restored)
/// gives back the input.
pub fn tokenize(text: &str) -> Vec<Token> {
    let mut out: Vec<Token> = Vec::new();
    let mut plain = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let boundary = i == 0 || !is_name_char(chars[i - 1]);
        if (c == '#' || c == '@') && boundary {
            let mut j = i + 1;
            while j < chars.len() && is_name_char(chars[j]) {
                j += 1;
            }
            // A trailing '-' is punctuation ("see #12-"), not part of the name.
            while j > i + 1 && chars[j - 1] == '-' {
                j -= 1;
            }
            if j > i + 1 {
                if !plain.is_empty() {
                    out.push(Token::Text(std::mem::take(&mut plain)));
                }
                let name: String = chars[i + 1..j].iter().collect();
                out.push(if c == '#' {
                    Token::TaskRef(name)
                } else {
                    Token::Mention(name)
                });
                i = j;
                continue;
            }
        }
        plain.push(c);
        i += 1;
    }
    if !plain.is_empty() {
        out.push(Token::Text(plain));
    }
    out
}

/// The full id of the task `r` refers to: an exact id, else the only id starting with `r`.
pub fn resolve_task(tasks: &[Task], r: &str) -> Option<String> {
    if let Some(t) = tasks.iter().find(|t| t.id == r) {
        return Some(t.id.clone());
    }
    let mut it = tasks.iter().filter(|t| t.id.starts_with(r));
    match (it.next(), it.next()) {
        (Some(t), None) => Some(t.id.clone()),
        _ => None,
    }
}

/// Look of a run of text.
#[derive(Debug, Clone, Copy)]
pub struct TextLook {
    pub size: f32,
    pub color: Color32,
    pub strong: bool,
}

impl Default for TextLook {
    fn default() -> Self {
        TextLook {
            size: 14.0,
            color: theme::TEXT,
            strong: false,
        }
    }
}

/// Draw `text` wrapped, with task references and mentions as links.
pub fn rich_text(ui: &mut Ui, text: &str, v: &View, out: &mut Intents) {
    rich_text_with(ui, text, v, out, TextLook::default());
}

pub fn rich_text_with(ui: &mut Ui, text: &str, v: &View, out: &mut Intents, look: TextLook) {
    let style = |s: &str| {
        let mut rt = RichText::new(s).size(look.size).color(look.color);
        if look.strong {
            rt = rt.strong();
        }
        rt
    };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for tok in tokenize(text) {
            match tok {
                Token::Text(s) => {
                    ui.add(egui::Label::new(style(&s)).wrap());
                }
                Token::TaskRef(r) => match resolve_task(&v.state.tasks, &r) {
                    Some(id) => {
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(format!("#{r}"))
                                        .size(look.size)
                                        .color(theme::INFO)
                                        .underline(),
                                )
                                .sense(Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            out.push(Intent::OpenTask(id));
                        }
                    }
                    None => {
                        ui.add(egui::Label::new(style(&format!("#{r}"))).wrap());
                    }
                },
                Token::Mention(n) => {
                    let known = v.members().iter().any(|m| m.name == n && !m.removed);
                    if known {
                        let c = v.color(&n);
                        let resp = ui
                            .add(
                                egui::Label::new(
                                    RichText::new(format!("@{n}"))
                                        .size(look.size)
                                        .color(c)
                                        .background_color(theme::tint(c, 0.15)),
                                )
                                .sense(Sense::click()),
                            )
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.clicked() {
                            out.push(Intent::OpenMember(n));
                        }
                    } else {
                        ui.add(egui::Label::new(style(&format!("@{n}"))).wrap());
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render};

    fn t(s: &str) -> Token {
        Token::Text(s.into())
    }
    fn r(s: &str) -> Token {
        Token::TaskRef(s.into())
    }
    fn m(s: &str) -> Token {
        Token::Mention(s.into())
    }

    #[test]
    fn plain_text() {
        assert_eq!(tokenize("hello world"), vec![t("hello world")]);
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn refs_and_mentions() {
        assert_eq!(
            tokenize("see #12 and ask @ada."),
            vec![t("see "), r("12"), t(" and ask "), m("ada"), t(".")]
        );
        assert_eq!(tokenize("#ab12cd34"), vec![r("ab12cd34")]);
        assert_eq!(tokenize("@bob: hi"), vec![m("bob"), t(": hi")]);
    }

    #[test]
    fn markers_need_a_boundary_and_a_name() {
        assert_eq!(tokenize("a@b.com"), vec![t("a@b.com")]);
        assert_eq!(tokenize("x#1"), vec![t("x#1")]);
        assert_eq!(tokenize("# @ #"), vec![t("# @ #")]);
        assert_eq!(tokenize("(#7)"), vec![t("("), r("7"), t(")")]);
        assert_eq!(tokenize("#12-"), vec![r("12"), t("-")]);
    }

    #[test]
    fn unicode_is_kept() {
        assert_eq!(
            tokenize("héllo @zoë ✔"),
            vec![t("héllo "), m("zoë"), t(" ✔")]
        );
    }

    #[test]
    fn resolves_exact_and_unique_prefix() {
        let (home, data) = fixture("links-resolve");
        let tasks = &data.state().tasks;
        let first = tasks[0].id.clone();
        assert_eq!(resolve_task(tasks, &first), Some(first.clone()));
        assert_eq!(resolve_task(tasks, "nope"), None);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn renders_without_panicking() {
        let (home, data) = fixture("links-render");
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut out = Intents::default();
        let id = v.state.tasks[0].id.clone();
        let text = format!("see #{id} and #zzz, ping @ada @ghost\nsecond line #");
        render(|ui| rich_text(ui, &text, &v, &mut out));
        render(|ui| rich_text(ui, "", &v, &mut out));
        assert!(out.0.is_empty());
        let _ = std::fs::remove_dir_all(home);
    }
}
