//! Syntax highlighting (syntect, pure-Rust regex engine) and language detection.

use std::path::Path;
use std::sync::OnceLock;

use eframe::egui::{self, text::LayoutJob, Color32, FontId, TextFormat};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

/// Above this size (bytes) or line count highlighting is skipped ("large file mode").
pub const LARGE_FILE_BYTES: usize = 1024 * 1024;
pub const LARGE_FILE_LINES: usize = 20_000;

/// True when `text` is too large to highlight.
pub fn is_large(text: &str) -> bool {
    text.len() > LARGE_FILE_BYTES || text.bytes().filter(|b| *b == b'\n').count() > LARGE_FILE_LINES
}

struct Syntaxes {
    ps: SyntaxSet,
    ts: ThemeSet,
}

fn syntaxes() -> &'static Syntaxes {
    static S: OnceLock<Syntaxes> = OnceLock::new();
    S.get_or_init(|| Syntaxes {
        ps: SyntaxSet::load_defaults_newlines(),
        ts: ThemeSet::load_defaults(),
    })
}

/// Language id (a file extension syntect understands, or a plain name) for a path.
/// Returns `"txt"` when unknown.
pub fn language_for_path(path: &Path) -> &'static str {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match name.as_str() {
        "makefile" | "gnumakefile" => return "makefile",
        "dockerfile" => return "sh",
        "cargo.lock" | "cargo.toml" => return "toml",
        ".gitignore" | ".env" | ".bashrc" | ".zshrc" | ".profile" => return "sh",
        _ => {}
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "rs",
        "ts" | "mts" | "cts" => "ts",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" => "js",
        "jsx" => "jsx",
        "json" | "jsonc" | "json5" => "json",
        "toml" => "toml",
        "yml" | "yaml" => "yaml",
        "md" | "markdown" | "mdx" => "md",
        "py" | "pyw" => "py",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kt",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "cs" => "cs",
        "rb" => "rb",
        "php" => "php",
        "swift" => "swift",
        "sh" | "bash" | "zsh" => "sh",
        "html" | "htm" | "vue" | "svelte" => "html",
        "css" | "scss" | "less" => "css",
        "xml" | "svg" | "plist" => "xml",
        "sql" => "sql",
        "lua" => "lua",
        "txt" | "log" => "txt",
        _ => "txt",
    }
}

/// Single-line comment prefix for a language id (used by toggle-comment).
pub fn comment_prefix(lang: &str) -> Option<&'static str> {
    match lang {
        "rs" | "ts" | "tsx" | "js" | "jsx" | "go" | "java" | "kt" | "c" | "cpp" | "cs" | "php"
        | "swift" | "json" => Some("//"),
        "py" | "sh" | "toml" | "yaml" | "rb" | "makefile" => Some("#"),
        "sql" | "lua" => Some("--"),
        _ => None,
    }
}

/// Map our language id to the extension used for syntect lookup (TypeScript and friends fall
/// back to JavaScript because the default syntax pack has no TS grammar).
fn syntect_ext(lang: &str) -> &str {
    match lang {
        "ts" | "tsx" | "jsx" | "kt" | "swift" | "cs" => match lang {
            "kt" | "swift" | "cs" => "java",
            _ => "js",
        },
        "toml" => "yaml",
        "vue" | "svelte" => "html",
        other => other,
    }
}

/// A highlighted token run: byte range into the text and colour.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub color: Color32,
    pub italic: bool,
    pub bold: bool,
}

/// Highlight `text`; returns `None` when the language is unknown.
pub fn spans(text: &str, lang: &str, dark: bool) -> Option<Vec<Span>> {
    let s = syntaxes();
    let syntax = s.ps.find_syntax_by_extension(syntect_ext(lang))?;
    let theme = &s.ts.themes[if dark {
        "base16-ocean.dark"
    } else {
        "InspiredGitHub"
    }];
    let mut h = HighlightLines::new(syntax, theme);
    let base = text.as_ptr() as usize;
    let mut out = Vec::new();
    for line in LinesWithEndings::from(text) {
        for (style, piece) in h.highlight_line(line, &s.ps).ok()? {
            let start = piece.as_ptr() as usize - base;
            let c = style.foreground;
            out.push(Span {
                start,
                end: start + piece.len(),
                color: Color32::from_rgb(c.r, c.g, c.b),
                italic: style.font_style.contains(FontStyle::ITALIC),
                bold: style.font_style.contains(FontStyle::BOLD),
            });
        }
    }
    Some(out)
}

/// Build a `LayoutJob` (no wrapping set) for `text`; plain when `lang` is unknown or `plain`.
pub fn layout_job(text: &str, lang: &str, font: &FontId, dark: bool, plain: bool) -> LayoutJob {
    let default_color = if dark {
        Color32::from_gray(210)
    } else {
        Color32::from_gray(40)
    };
    let spans = if plain { None } else { spans(text, lang, dark) };
    let Some(spans) = spans else {
        return LayoutJob::simple(text.to_owned(), font.clone(), default_color, f32::INFINITY);
    };
    let mut job = LayoutJob {
        text: text.to_owned(),
        ..Default::default()
    };
    for sp in spans {
        job.sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: egui::text::ByteIndex(sp.start)..egui::text::ByteIndex(sp.end),
            format: TextFormat {
                font_id: font.clone(),
                color: sp.color,
                italics: sp.italic,
                ..Default::default()
            },
        });
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_languages() {
        let l = |p: &str| language_for_path(Path::new(p));
        assert_eq!(l("src/main.rs"), "rs");
        assert_eq!(l("a/b.TS"), "ts");
        assert_eq!(l("x.tsx"), "tsx");
        assert_eq!(l("package.json"), "json");
        assert_eq!(l("Cargo.toml"), "toml");
        assert_eq!(l("README.md"), "md");
        assert_eq!(l("Makefile"), "makefile");
        assert_eq!(l("noext"), "txt");
        assert_eq!(l("weird.zzz"), "txt");
    }

    fn colors(text: &str, lang: &str) -> std::collections::HashSet<[u8; 4]> {
        spans(text, lang, true)
            .unwrap()
            .iter()
            .map(|s| s.color.to_array())
            .collect()
    }

    #[test]
    fn highlights_samples() {
        let rust = "fn main() {\n    let x = \"hi\"; // c\n}\n";
        assert!(colors(rust, "rs").len() > 2);
        let ts = "const a: number = 1;\nfunction f() { return 'x'; }\n";
        assert!(colors(ts, "ts").len() > 2);
        let json = "{\"a\": [1, 2, true], \"b\": \"s\"}\n";
        assert!(colors(json, "json").len() > 2);
    }

    #[test]
    fn spans_cover_text_contiguously() {
        let text = "fn a() {}\nlet é = 1;\n";
        let sp = spans(text, "rs", true).unwrap();
        let mut at = 0;
        for s in &sp {
            assert_eq!(s.start, at);
            at = s.end;
        }
        assert_eq!(at, text.len());
    }

    #[test]
    fn unknown_language_is_plain() {
        assert!(spans("x", "nope", true).is_none());
        let job = layout_job("x", "nope", &FontId::monospace(12.0), true, false);
        assert_eq!(job.sections.len(), 1);
    }

    #[test]
    fn large_detection() {
        assert!(!is_large("a\nb"));
        assert!(is_large(&"x".repeat(LARGE_FILE_BYTES + 1)));
        assert!(is_large(&"\n".repeat(LARGE_FILE_LINES + 1)));
    }
}
