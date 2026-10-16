//! A small markdown renderer: headings, emphasis, code blocks (highlighted), lists, links,
//! block quotes, tables, rules. The block / inline parsers are pure and tested.

use eframe::egui::{self, Color32, RichText};

use super::highlight;

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Strike(Vec<Inline>),
    Code(String),
    Link { text: String, url: String },
    Image { alt: String, url: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub depth: usize,
    /// `Some(n)` for ordered items.
    pub number: Option<u64>,
    /// `Some(done)` for task items.
    pub checked: Option<bool>,
    pub content: Vec<Inline>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Heading(u8, Vec<Inline>),
    Paragraph(Vec<Inline>),
    Code {
        lang: String,
        text: String,
    },
    List(Vec<ListItem>),
    Quote(Vec<Block>),
    Table {
        header: Vec<Vec<Inline>>,
        rows: Vec<Vec<Vec<Inline>>>,
    },
    Rule,
}

// ----------------------------------------------------------------------------
// Inline parsing

pub fn parse_inline(s: &str) -> Vec<Inline> {
    let chars: Vec<char> = s.chars().collect();
    parse_inline_chars(&chars)
}

fn find_seq(c: &[char], from: usize, pat: &[char]) -> Option<usize> {
    if pat.is_empty() || c.len() < pat.len() {
        return None;
    }
    (from..=c.len() - pat.len()).find(|&i| c[i..i + pat.len()] == *pat)
}

fn parse_inline_chars(c: &[char]) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    macro_rules! flush {
        () => {
            if !buf.is_empty() {
                out.push(Inline::Text(std::mem::take(&mut buf)));
            }
        };
    }
    while i < c.len() {
        let ch = c[i];
        match ch {
            '\\' if i + 1 < c.len() && c[i + 1].is_ascii_punctuation() => {
                buf.push(c[i + 1]);
                i += 2;
            }
            '`' => {
                let ticks = c[i..].iter().take_while(|x| **x == '`').count();
                let pat = vec!['`'; ticks];
                if let Some(end) = find_seq(c, i + ticks, &pat) {
                    flush!();
                    let code: String = c[i + ticks..end].iter().collect();
                    out.push(Inline::Code(code.trim().to_string()));
                    i = end + ticks;
                } else {
                    buf.push(ch);
                    i += 1;
                }
            }
            '*' | '_' | '~' => {
                let double = i + 1 < c.len() && c[i + 1] == ch;
                let n = if double { 2 } else { 1 };
                // `_` inside words is not emphasis
                let word_inside = ch == '_'
                    && i > 0
                    && c[i - 1].is_alphanumeric()
                    && c.get(i + n).is_some_and(|x| x.is_alphanumeric());
                let pat = vec![ch; n];
                let mut end = find_seq(c, i + n, &pat);
                if double && ch != '~' {
                    while let Some(e) = end {
                        if c.get(e + 2) == Some(&ch) {
                            end = Some(e + 1);
                        } else {
                            break;
                        }
                    }
                }
                match end {
                    Some(end)
                        if !word_inside
                            && end > i + n
                            && !c[i + n].is_whitespace()
                            && !c[end - 1].is_whitespace()
                            && (ch != '~' || double) =>
                    {
                        flush!();
                        let inner = parse_inline_chars(&c[i + n..end]);
                        out.push(match (ch, double) {
                            ('~', _) => Inline::Strike(inner),
                            (_, true) => Inline::Bold(inner),
                            _ => Inline::Italic(inner),
                        });
                        i = end + n;
                    }
                    _ => {
                        buf.push(ch);
                        i += 1;
                    }
                }
            }
            '!' | '[' => {
                let is_img = ch == '!';
                let open = if is_img { i + 1 } else { i };
                if c.get(open) == Some(&'[') {
                    if let Some(close) = c[open..].iter().position(|x| *x == ']').map(|p| p + open)
                    {
                        if c.get(close + 1) == Some(&'(') {
                            if let Some(pe) = c[close + 2..]
                                .iter()
                                .position(|x| *x == ')')
                                .map(|p| p + close + 2)
                            {
                                flush!();
                                let text: String = c[open + 1..close].iter().collect();
                                let url: String = c[close + 2..pe].iter().collect();
                                let url = url.split_whitespace().next().unwrap_or("").to_string();
                                out.push(if is_img {
                                    Inline::Image { alt: text, url }
                                } else {
                                    Inline::Link { text, url }
                                });
                                i = pe + 1;
                                continue;
                            }
                        }
                    }
                }
                buf.push(ch);
                i += 1;
            }
            '<' => {
                // autolink <https://...>
                if let Some(end) = c[i..].iter().position(|x| *x == '>').map(|p| p + i) {
                    let inner: String = c[i + 1..end].iter().collect();
                    if inner.starts_with("http://") || inner.starts_with("https://") {
                        flush!();
                        out.push(Inline::Link {
                            text: inner.clone(),
                            url: inner,
                        });
                        i = end + 1;
                        continue;
                    }
                }
                buf.push(ch);
                i += 1;
            }
            'h' if c[i..].starts_with(&['h', 't', 't', 'p']) => {
                let rest: String = c[i..].iter().collect();
                if rest.starts_with("http://") || rest.starts_with("https://") {
                    let len = rest
                        .chars()
                        .take_while(|x| !x.is_whitespace() && *x != ')' && *x != '>')
                        .count();
                    let mut url: String = rest.chars().take(len).collect();
                    while url.ends_with(['.', ',', ';', ':', '!', '?']) {
                        url.pop();
                    }
                    if url.len() > 8 {
                        flush!();
                        let n = url.chars().count();
                        out.push(Inline::Link {
                            text: url.clone(),
                            url,
                        });
                        i += n;
                        continue;
                    }
                }
                buf.push(ch);
                i += 1;
            }
            _ => {
                buf.push(ch);
                i += 1;
            }
        }
    }
    flush!();
    out
}

// ----------------------------------------------------------------------------
// Block parsing

fn split_row(line: &str) -> Vec<String> {
    let t = line.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_table_sep(line: &str) -> bool {
    let t = line.trim();
    t.contains('-')
        && t.contains('|') | t.contains(':')
        && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn is_rule(line: &str) -> bool {
    let t: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    t.len() >= 3
        && (t.chars().all(|c| c == '-')
            || t.chars().all(|c| c == '*')
            || t.chars().all(|c| c == '_'))
}

fn heading(line: &str) -> Option<(u8, &str)> {
    let hashes = line.chars().take_while(|c| *c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &line[hashes..];
        if rest.is_empty() || rest.starts_with(' ') {
            return Some((hashes as u8, rest.trim().trim_end_matches('#').trim_end()));
        }
    }
    None
}

fn list_marker(line: &str) -> Option<(usize, Option<u64>, &str)> {
    let indent = line.len() - line.trim_start().len();
    let spaces = line[..indent]
        .chars()
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum::<usize>();
    let t = &line[indent..];
    for m in ["- ", "* ", "+ "] {
        if let Some(r) = t.strip_prefix(m) {
            return Some((spaces / 2, None, r));
        }
    }
    let digits = t.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && digits < 10 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((spaces / 2, t[..digits].parse().ok(), r));
        }
    }
    None
}

pub fn parse_blocks(md: &str) -> Vec<Block> {
    let lines: Vec<&str> = md.lines().collect();
    parse_lines(&lines)
}

fn parse_lines(lines: &[&str]) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        // fenced code
        if let Some(fence) = ["```", "~~~"].iter().find(|f| trimmed.starts_with(**f)) {
            let lang = trimmed.trim_start_matches(*fence).trim().to_string();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with(*fence) {
                body.push(lines[i]);
                i += 1;
            }
            i += 1; // closing fence (or EOF)
            blocks.push(Block::Code {
                lang,
                text: body.join("\n"),
            });
            continue;
        }
        if let Some((lvl, t)) = heading(trimmed) {
            blocks.push(Block::Heading(lvl, parse_inline(t)));
            i += 1;
            continue;
        }
        if is_rule(line) && list_marker(line).is_none()
            || (is_rule(line) && trimmed.starts_with("---"))
        {
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if trimmed.starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let l = lines[i].trim_start().trim_start_matches('>');
                inner.push(l.strip_prefix(' ').unwrap_or(l));
                i += 1;
            }
            blocks.push(Block::Quote(parse_lines(&inner)));
            continue;
        }
        if line.contains('|') && i + 1 < lines.len() && is_table_sep(lines[i + 1]) {
            let header: Vec<Vec<Inline>> =
                split_row(line).iter().map(|c| parse_inline(c)).collect();
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() && lines[i].contains('|') && !lines[i].trim().is_empty() {
                rows.push(
                    split_row(lines[i])
                        .iter()
                        .map(|c| parse_inline(c))
                        .collect(),
                );
                i += 1;
            }
            blocks.push(Block::Table { header, rows });
            continue;
        }
        if list_marker(line).is_some() {
            let mut items: Vec<ListItem> = Vec::new();
            while i < lines.len() {
                if let Some((depth, number, rest)) = list_marker(lines[i]) {
                    let (checked, rest) = if let Some(r) = rest.strip_prefix("[ ] ") {
                        (Some(false), r)
                    } else if let Some(r) = rest
                        .strip_prefix("[x] ")
                        .or_else(|| rest.strip_prefix("[X] "))
                    {
                        (Some(true), r)
                    } else {
                        (None, rest)
                    };
                    items.push(ListItem {
                        depth,
                        number,
                        checked,
                        content: parse_inline(rest),
                    });
                    i += 1;
                } else if !lines[i].trim().is_empty()
                    && lines[i].starts_with("  ")
                    && heading(lines[i].trim_start()).is_none()
                {
                    // continuation line
                    if let Some(last) = items.last_mut() {
                        last.content.push(Inline::Text(" ".into()));
                        last.content.extend(parse_inline(lines[i].trim()));
                    }
                    i += 1;
                } else {
                    break;
                }
            }
            blocks.push(Block::List(items));
            continue;
        }
        // paragraph: until blank line or another block start
        let mut para = vec![line.trim()];
        i += 1;
        while i < lines.len() {
            let l = lines[i];
            let t = l.trim_start();
            if l.trim().is_empty()
                || t.starts_with("```")
                || t.starts_with("~~~")
                || t.starts_with('>')
                || heading(t).is_some()
                || list_marker(l).is_some()
                || is_rule(l)
            {
                break;
            }
            para.push(l.trim());
            i += 1;
        }
        blocks.push(Block::Paragraph(parse_inline(&para.join(" "))));
    }
    blocks
}

// ----------------------------------------------------------------------------
// Rendering

fn render_inlines(ui: &mut egui::Ui, inl: &[Inline]) {
    fn walk(
        ui: &mut egui::Ui,
        inl: &[Inline],
        bold: bool,
        italic: bool,
        strike: bool,
        size: Option<f32>,
    ) {
        for x in inl {
            let style = |mut t: RichText| {
                if bold {
                    t = t.strong();
                }
                if italic {
                    t = t.italics();
                }
                if strike {
                    t = t.strikethrough();
                }
                if let Some(s) = size {
                    t = t.size(s);
                }
                t
            };
            match x {
                Inline::Text(s) => {
                    ui.label(style(RichText::new(s)));
                }
                Inline::Bold(v) => walk(ui, v, true, italic, strike, size),
                Inline::Italic(v) => walk(ui, v, bold, true, strike, size),
                Inline::Strike(v) => walk(ui, v, bold, italic, true, size),
                Inline::Code(s) => {
                    ui.label(
                        style(RichText::new(s).monospace())
                            .background_color(ui.visuals().code_bg_color),
                    );
                }
                Inline::Link { text, url } => {
                    ui.hyperlink_to(text, url);
                }
                Inline::Image { alt, url } => {
                    if url.starts_with("http://") || url.starts_with("https://") {
                        ui.hyperlink_to(format!("[image: {alt}]"), url);
                    } else {
                        ui.weak(format!("[image: {alt}]"));
                    }
                }
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        walk(ui, inl, false, false, false, None);
    });
}

fn heading_size(level: u8) -> f32 {
    match level {
        1 => 26.0,
        2 => 22.0,
        3 => 18.0,
        4 => 16.0,
        _ => 14.0,
    }
}

fn plain_text(inl: &[Inline]) -> String {
    inl.iter()
        .map(|x| match x {
            Inline::Text(s) | Inline::Code(s) => s.clone(),
            Inline::Bold(v) | Inline::Italic(v) | Inline::Strike(v) => plain_text(v),
            Inline::Link { text, .. } => text.clone(),
            Inline::Image { alt, .. } => alt.clone(),
        })
        .collect()
}

fn show_code(ui: &mut egui::Ui, lang: &str, text: &str) {
    let dark = ui.visuals().dark_mode;
    let font = egui::FontId::monospace(12.5);
    let lang_id = match lang {
        "rust" => "rs",
        "typescript" => "ts",
        "javascript" => "js",
        "python" => "py",
        "bash" | "shell" | "zsh" => "sh",
        "yml" => "yaml",
        other => other,
    };
    let job = highlight::layout_job(text, lang_id, &font, dark, text.len() > 200_000);
    egui::Frame::new()
        .fill(ui.visuals().code_bg_color)
        .corner_radius(4.0)
        .inner_margin(8.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::ScrollArea::horizontal().show(ui, |ui| {
                ui.add(egui::Label::new(job).selectable(true).extend());
            });
        });
}

fn show_blocks(ui: &mut egui::Ui, blocks: &[Block]) {
    for b in blocks {
        match b {
            Block::Heading(l, inl) => {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(plain_text(inl))
                        .size(heading_size(*l))
                        .strong(),
                );
                if *l <= 2 {
                    ui.separator();
                }
            }
            Block::Paragraph(inl) => render_inlines(ui, inl),
            Block::Code { lang, text } => show_code(ui, lang, text),
            Block::Rule => {
                ui.separator();
            }
            Block::List(items) => {
                for it in items {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(it.depth as f32 * 16.0 + 4.0);
                        let marker = match (it.number, it.checked) {
                            (_, Some(true)) => "[x]".to_string(),
                            (_, Some(false)) => "[ ]".to_string(),
                            (Some(n), _) => format!("{n}."),
                            _ => "\u{2022}".to_string(),
                        };
                        ui.label(marker);
                        render_inlines(ui, &it.content);
                    });
                }
            }
            Block::Quote(inner) => {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(3.0, 0.0), egui::Sense::hover());
                    let start = r.min;
                    ui.vertical(|ui| {
                        ui.visuals_mut().override_text_color = Some(ui.visuals().weak_text_color());
                        show_blocks(ui, inner);
                    });
                    let end_y = ui.min_rect().max.y;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(start, egui::pos2(start.x + 3.0, end_y)),
                        1.0,
                        Color32::from_gray(120),
                    );
                });
            }
            Block::Table { header, rows } => {
                let cols = header
                    .len()
                    .max(rows.iter().map(Vec::len).max().unwrap_or(0));
                egui::Grid::new(ui.next_auto_id())
                    .striped(true)
                    .spacing([14.0, 4.0])
                    .show(ui, |ui| {
                        for c in 0..cols {
                            let t = header.get(c).map(|h| plain_text(h)).unwrap_or_default();
                            ui.label(RichText::new(t).strong());
                        }
                        ui.end_row();
                        for r in rows {
                            for c in 0..cols {
                                match r.get(c) {
                                    Some(inl) => render_inlines(ui, inl),
                                    None => {
                                        ui.label("");
                                    }
                                }
                            }
                            ui.end_row();
                        }
                    });
            }
        }
        ui.add_space(4.0);
    }
}

/// Render markdown into `ui` (wraps itself in a vertical scroll area).
pub fn show(ui: &mut egui::Ui, md: &str) {
    let blocks = parse_blocks(md);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .id_salt("md_preview")
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width().clamp(100.0, 900.0));
            show_blocks(ui, &blocks);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> Inline {
        Inline::Text(s.into())
    }

    #[test]
    fn inline_forms() {
        assert_eq!(
            parse_inline("a **b** c"),
            vec![t("a "), Inline::Bold(vec![t("b")]), t(" c")]
        );
        assert_eq!(parse_inline("*i*"), vec![Inline::Italic(vec![t("i")])]);
        assert_eq!(parse_inline("~~x~~"), vec![Inline::Strike(vec![t("x")])]);
        assert_eq!(parse_inline("`a*b`"), vec![Inline::Code("a*b".into())]);
        assert_eq!(
            parse_inline("[go](http://x.io) now"),
            vec![
                Inline::Link {
                    text: "go".into(),
                    url: "http://x.io".into()
                },
                t(" now")
            ]
        );
        assert_eq!(parse_inline("snake_case_name"), vec![t("snake_case_name")]);
        assert_eq!(parse_inline("2 * 3 * 4"), vec![t("2 * 3 * 4")]);
        assert_eq!(parse_inline(r"\*lit\*"), vec![t("*lit*")]);
        assert_eq!(
            parse_inline("see https://a.b/c."),
            vec![
                t("see "),
                Inline::Link {
                    text: "https://a.b/c".into(),
                    url: "https://a.b/c".into()
                },
                t(".")
            ]
        );
    }

    #[test]
    fn nested_emphasis() {
        assert_eq!(
            parse_inline("**a *b***"),
            vec![Inline::Bold(vec![t("a "), Inline::Italic(vec![t("b")])])]
        );
    }

    #[test]
    fn block_structure() {
        let md = "# Title\n\nhello\nworld\n\n```rust\nfn a() {}\n```\n\n- one\n- [x] two\n  - nested\n\n1. a\n2. b\n\n> quote\n> more\n\n---\n\n| h1 | h2 |\n|---|---|\n| a | b |\n";
        let b = parse_blocks(md);
        assert!(matches!(&b[0], Block::Heading(1, _)));
        assert_eq!(b[1], Block::Paragraph(vec![t("hello world")]));
        assert_eq!(
            b[2],
            Block::Code {
                lang: "rust".into(),
                text: "fn a() {}".into()
            }
        );
        match &b[3] {
            Block::List(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[1].checked, Some(true));
                assert_eq!(items[2].depth, 1);
            }
            o => panic!("{o:?}"),
        }
        match &b[4] {
            Block::List(items) => assert_eq!(items[1].number, Some(2)),
            o => panic!("{o:?}"),
        }
        assert!(matches!(&b[5], Block::Quote(q) if q.len() == 1));
        assert_eq!(b[6], Block::Rule);
        match &b[7] {
            Block::Table { header, rows } => {
                assert_eq!(header.len(), 2);
                assert_eq!(rows.len(), 1);
            }
            o => panic!("{o:?}"),
        }
        assert_eq!(b.len(), 8);
    }

    #[test]
    fn unterminated_fence_and_empty() {
        let b = parse_blocks("```\ncode");
        assert_eq!(
            b,
            vec![Block::Code {
                lang: String::new(),
                text: "code".into()
            }]
        );
        assert!(parse_blocks("").is_empty());
    }

    #[test]
    fn renders_headless() {
        let md = "# H\n\ntext **b** `c` [l](https://x.y)\n\n```ts\nconst a = 1;\n```\n\n- a\n- b\n\n> q\n\n| a | b |\n|-|-|\n| 1 | 2 |\n";
        crate::view::testing::render(|ui| show(ui, md));
    }
}
