//! Tab model, tab bar, banners and the editor body (adapter over the code widget).

use std::path::Path;

use eframe::egui;
use rteams_services::docpreview::{self, DocPreview, Limits};
use rteams_services::editor::conflict::BaseStamp;
use rteams_services::editor::files::{Encoding, FileContent};
use rteams_services::i18n::I18n;

use super::code_view::{CodeView, CodeViewOptions};
use super::{image_preview, md_preview};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    Text,
    Binary,
    Image,
    /// docx / pptx / xlsx: read-only preview.
    Document,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MdMode {
    #[default]
    Off,
    Split,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Banner {
    ChangedOnDisk,
    Deleted,
    DraftRecovered,
    SaveFailed(String),
}

pub struct Tab {
    /// Relative to the project root, `/` separated.
    pub rel: String,
    pub text: String,
    pub base: BaseStamp,
    pub dirty: bool,
    pub kind: TabKind,
    pub size: u64,
    pub encoding: Encoding,
    pub large: bool,
    pub truncated: bool,
    /// 1-based (line, col).
    pub cursor: (usize, usize),
    pub scroll: f32,
    pub banner: Option<Banner>,
    /// One-shot request to move the caret to (line, col) and scroll there.
    pub goto: Option<(usize, usize)>,
    pub view: CodeView,
    pub md: MdMode,
    pub doc: Option<Result<DocPreview, String>>,
}

pub fn is_image(rel: &str) -> bool {
    let ext = rel
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp"
    )
}

pub fn is_document(rel: &str) -> bool {
    let p = Path::new(rel);
    docpreview::is_previewable(p) && !rel.to_ascii_lowercase().ends_with(".csv")
}

impl Tab {
    pub fn from_content(rel: &str, c: FileContent) -> Tab {
        let kind = if is_image(rel) {
            TabKind::Image
        } else if is_document(rel) {
            TabKind::Document
        } else if c.is_binary {
            TabKind::Binary
        } else {
            TabKind::Text
        };
        Tab {
            rel: rel.to_string(),
            base: BaseStamp::from(&c),
            text: c.text,
            dirty: false,
            kind,
            size: c.size,
            encoding: c.encoding,
            large: c.large,
            truncated: c.truncated,
            cursor: (1, 1),
            scroll: 0.0,
            banner: None,
            goto: None,
            view: CodeView::default(),
            md: MdMode::Off,
            doc: None,
        }
    }

    pub fn name(&self) -> &str {
        self.rel.rsplit('/').next().unwrap_or(&self.rel)
    }

    pub fn read_only(&self) -> bool {
        self.truncated || self.kind != TabKind::Text
    }

    pub fn language(&self) -> &'static str {
        language_of(&self.rel)
    }
}

/// Language label shown in the status bar and passed to the highlighter.
pub fn language_of(rel: &str) -> &'static str {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    if name == "Dockerfile" {
        return "Dockerfile";
    }
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "rs" => "Rust",
        "js" | "mjs" | "cjs" => "JavaScript",
        "ts" => "TypeScript",
        "tsx" => "TSX",
        "jsx" => "JSX",
        "py" => "Python",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "cc" | "hpp" => "C++",
        "rb" => "Ruby",
        "sh" | "bash" | "zsh" => "Shell",
        "html" | "htm" => "HTML",
        "css" => "CSS",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "md" | "markdown" => "Markdown",
        "sql" => "SQL",
        "swift" => "Swift",
        "kt" => "Kotlin",
        "xml" => "XML",
        _ => "Plain Text",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabAction {
    Activate(usize),
    Close(usize),
}

/// The tab strip. Middle-click closes; the dirty dot turns into a close button on hover.
pub fn show_bar(ui: &mut egui::Ui, tabs: &[Tab], active: usize, tr: &I18n) -> Vec<TabAction> {
    let mut acts = Vec::new();
    egui::ScrollArea::horizontal()
        .id_salt("editor-tabs")
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, t) in tabs.iter().enumerate() {
                    let sel = i == active;
                    let r = ui
                        .scope(|ui| {
                            egui::Frame::new()
                                .fill(if sel { theme::SURFACE } else { theme::SIDEBAR })
                                .inner_margin(egui::Margin::symmetric(8, 4))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let l = ui.add(
                                            egui::Label::new(egui::RichText::new(t.name()).color(
                                                if sel { theme::TEXT } else { theme::TEXT_2 },
                                            ))
                                            .sense(egui::Sense::click())
                                            .truncate(),
                                        );
                                        let close = if t.dirty {
                                            ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new("•").color(theme::WARNING),
                                                )
                                                .frame(false),
                                            )
                                            .on_hover_text(tr.t("team.editor.unsavedChanges"))
                                        } else {
                                            ui.add(
                                                egui::Button::new(
                                                    egui::RichText::new("🗙")
                                                        .small()
                                                        .color(theme::MUTED),
                                                )
                                                .frame(false),
                                            )
                                            .on_hover_text(tr.t("team.editor.actions.closeTab"))
                                        };
                                        (l, close)
                                    })
                                    .inner
                                })
                                .inner
                        })
                        .inner;
                    let (label, close) = r;
                    if close.clicked() || label.middle_clicked() {
                        acts.push(TabAction::Close(i));
                    } else if label.clicked() {
                        acts.push(TabAction::Activate(i));
                    }
                }
            });
        });
    acts
}

/// What the body reported for this frame.
#[derive(Debug, Default, Clone, Copy)]
pub struct BodyResponse {
    pub changed: bool,
    pub cursor: Option<(usize, usize)>,
}

/// Text editing: the shared code widget (line numbers, highlighting, find, go to line).
pub fn edit_text(ui: &mut egui::Ui, t: &mut Tab, wrap: bool) -> BodyResponse {
    if let Some((l, _)) = t.goto.take() {
        t.view.goto_line(&t.text, l);
    }
    let lang = super::highlight::language_for_path(Path::new(&t.rel));
    let opts = CodeViewOptions {
        read_only: t.read_only(),
        word_wrap: wrap && !t.large,
        large_file: t.large,
        ..Default::default()
    };
    let r = t.view.show(ui, &mut t.text, lang, &opts);
    BodyResponse {
        changed: r.changed,
        cursor: Some((r.cursor_line, r.cursor_col)),
    }
}

/// Markdown body: editor, split with preview, or preview only.
pub fn edit_markdown(ui: &mut egui::Ui, t: &mut Tab, wrap: bool, tr: &I18n) -> BodyResponse {
    ui.horizontal(|ui| {
        let split = t.md == MdMode::Split;
        let full = t.md == MdMode::Full;
        if ui
            .selectable_label(split, tr.t("team.editor.shortcuts.actions.splitPreview"))
            .clicked()
        {
            t.md = if split { MdMode::Off } else { MdMode::Split };
        }
        if ui
            .selectable_label(full, tr.t("team.editor.shortcuts.actions.fullPreview"))
            .clicked()
        {
            t.md = if full { MdMode::Off } else { MdMode::Full };
        }
    });
    match t.md {
        MdMode::Off => edit_text(ui, t, wrap),
        MdMode::Full => {
            egui::ScrollArea::vertical()
                .id_salt("md-full")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    md_preview::show(ui, &t.text);
                });
            BodyResponse::default()
        }
        MdMode::Split => {
            let mut out = BodyResponse::default();
            ui.columns(2, |cols| {
                out = edit_text(&mut cols[0], t, wrap);
                egui::ScrollArea::vertical()
                    .id_salt("md-split")
                    .auto_shrink([false, false])
                    .show(&mut cols[1], |ui| {
                        md_preview::show(ui, &t.text);
                    });
            });
            out
        }
    }
}

pub fn banner_text(b: &Banner, tr: &I18n) -> String {
    match b {
        Banner::ChangedOnDisk => tr.t("team.editor.externalChange.changed"),
        Banner::Deleted => tr.t("team.editor.externalChange.deleted"),
        Banner::DraftRecovered => tr.t("team.editor.draftRecovered"),
        Banner::SaveFailed(e) => tr.t_args("team.editor.saveFailed", &[("error", e)]),
    }
}

/// What the user chose on a banner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerAction {
    Reload,
    Keep,
    Retry,
    Dismiss,
}

pub fn show_banner(ui: &mut egui::Ui, b: &Banner, tr: &I18n) -> Option<BannerAction> {
    let mut act = None;
    egui::Frame::new()
        .fill(theme::RAISED)
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let color = if matches!(b, Banner::SaveFailed(_) | Banner::Deleted) {
                    theme::ERROR_FG
                } else {
                    theme::WARNING
                };
                ui.colored_label(color, banner_text(b, tr));
                match b {
                    Banner::ChangedOnDisk => {
                        if ui.button(tr.t("team.editor.actions.reload")).clicked() {
                            act = Some(BannerAction::Reload);
                        }
                        if ui.button(tr.t("team.editor.actions.keepMine")).clicked() {
                            act = Some(BannerAction::Keep);
                        }
                    }
                    Banner::Deleted => {
                        if ui.button(tr.t("team.editor.actions.keep")).clicked() {
                            act = Some(BannerAction::Keep);
                        }
                    }
                    Banner::SaveFailed(_) => {
                        if ui.button(tr.t("team.editor.actions.retry")).clicked() {
                            act = Some(BannerAction::Retry);
                        }
                    }
                    Banner::DraftRecovered => {}
                }
                if ui.small_button("🗙").clicked() && act.is_none() {
                    act = Some(BannerAction::Dismiss);
                }
            });
        });
    act
}

pub fn human_size(n: u64) -> String {
    const U: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let (mut v, mut i) = (n as f64, 0);
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Non-text bodies (binary placeholder, image, office document).
pub fn show_non_text(ui: &mut egui::Ui, root: &Path, t: &mut Tab, tr: &I18n) {
    let abs = root.join(&t.rel);
    match t.kind {
        TabKind::Binary => {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(tr.t_args(
                        "team.editor.binaryPlaceholder.file",
                        &[("size", &human_size(t.size))],
                    ))
                    .color(theme::MUTED),
                );
                if ui
                    .button(tr.t("team.editor.imagePreview.openSystemViewer"))
                    .clicked()
                {
                    let _ = open::that(&abs);
                }
            });
        }
        TabKind::Image => image_preview::show(ui, &abs),
        TabKind::Document => {
            let doc = t
                .doc
                .get_or_insert_with(|| docpreview::preview_file(&abs, &Limits::default()));
            ui.label(
                egui::RichText::new(tr.t("team.editor.documentPreview.readOnly"))
                    .small()
                    .color(theme::MUTED),
            );
            match doc {
                Ok(d) => image_preview::show_document(ui, d),
                Err(_) => {
                    ui.label(tr.t("team.editor.documentPreview.unavailable"));
                }
            }
            if ui
                .button(tr.t("team.editor.imagePreview.openSystemViewer"))
                .clicked()
            {
                let _ = open::that(&abs);
            }
        }
        TabKind::Text => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(language_of("src/a.rs"), "Rust");
        assert_eq!(language_of("Dockerfile"), "Dockerfile");
        assert_eq!(language_of("x"), "Plain Text");
        assert_eq!(human_size(2048), "2.0 KiB");
        assert!(is_image("a/b.PNG"));
    }
}
