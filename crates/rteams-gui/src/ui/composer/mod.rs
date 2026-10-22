//! Chat composer widget: multiline input (Enter sends, Shift+Enter newline), `@member` /
//! `#task` / `/command` popups, file attachments (picker, drop, pasted paths) and a
//! "attach code context" picker (file + line range -> fenced snippet + attachment).

use std::path::{Path, PathBuf};

use eframe::egui::{self, text::CCursor, text::CCursorRange};
use image::ImageEncoder;
use rteams_core::Attachment;

use crate::view::View;

pub mod logic;
use logic::{Item, Trigger};

/// Files above this size are refused (the log stores only a path, but agents read them).
const MAX_ATTACH_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ComposerEvent {
    pub send: bool,
    /// Attachments to send with the message (drained on `send`).
    pub attachments: Vec<Attachment>,
}

#[derive(Default)]
struct CodePicker {
    open: bool,
    path: String,
    start: usize,
    end: usize,
    error: Option<String>,
}

#[derive(Default)]
pub struct Composer {
    attachments: Vec<Attachment>,
    selected: usize,
    /// Cursor (chars) seen last frame, used for popup detection before the editor runs.
    cursor: usize,
    /// Where copied attachments go: `<home>/<team>/attachments`. `None` = reference in place.
    pub attach_dir: Option<PathBuf>,
    code: CodePicker,
    error: Option<String>,
    pub focus: bool,
    /// Chip thumbnails by attachment path (`None` = not an image / undecodable).
    thumbs: std::collections::HashMap<String, Option<egui::TextureHandle>>,
}

fn tt(v: &View, key: &str, fallback: &str) -> String {
    let s = v.tr.t(key);
    if s == key {
        fallback.to_string()
    } else {
        s
    }
}

impl Composer {
    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// Add a file (copied into `attach_dir` when set). Retries a copy that races a writer.
    pub fn attach_path(&mut self, src: &Path) -> Result<(), String> {
        let meta = std::fs::metadata(src).map_err(|e| e.to_string())?;
        if !meta.is_file() {
            return Err("not a file".into());
        }
        if meta.len() > MAX_ATTACH_BYTES {
            return Err(format!("{} is larger than 25 MB", src.display()));
        }
        let name = src
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy().into_owned());
        let path = match &self.attach_dir {
            Some(dir) => {
                let id = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos());
                let dest_dir = dir.join(format!("{id:x}"));
                std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
                let dest = dest_dir.join(&name);
                let mut last = None;
                for _ in 0..3 {
                    match std::fs::copy(src, &dest) {
                        Ok(_) => {
                            last = None;
                            break;
                        }
                        Err(e) => {
                            last = Some(e.to_string());
                            std::thread::sleep(std::time::Duration::from_millis(80));
                        }
                    }
                }
                if let Some(e) = last {
                    return Err(e);
                }
                dest
            }
            None => src.to_path_buf(),
        };
        self.push(Attachment {
            name,
            path: path.to_string_lossy().into_owned(),
        });
        Ok(())
    }

    fn attach_bytes(&mut self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if bytes.len() as u64 > MAX_ATTACH_BYTES {
            return Err(format!("{name} is larger than 25 MB"));
        }
        let dir = self
            .attach_dir
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("rteams-attachments"));
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dest_dir = dir.join(format!("{id:x}"));
        std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
        let dest = dest_dir.join(name);
        std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
        self.push(Attachment {
            name: name.to_string(),
            path: dest.to_string_lossy().into_owned(),
        });
        Ok(())
    }

    /// Attach a clipboard screenshot as `pasted-<ts>.png`.
    fn attach_pasted_image(&mut self, img: &egui::ColorImage) -> Result<(), String> {
        let rgba: Vec<u8> = img
            .pixels
            .iter()
            .flat_map(|c| c.to_srgba_unmultiplied())
            .collect();
        let png = encode_png(img.size[0] as u32, img.size[1] as u32, &rgba)?;
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        self.attach_bytes(&format!("pasted-{ms}.png"), &png)
    }

    fn push(&mut self, a: Attachment) {
        if !self.attachments.iter().any(|x| x.path == a.path) {
            self.attachments.push(a);
        }
    }

    fn report(&mut self, r: Result<(), String>) {
        if let Err(e) = r {
            self.error = Some(e);
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, v: &View, text: &mut String) -> ComposerEvent {
        let mut ev = ComposerEvent::default();
        let id = ui.make_persistent_id("composer-input");
        let focused = ui.memory(|m| m.has_focus(id));

        // Popup state from the cursor seen last frame; keys must be consumed before the editor.
        let trigger = if focused {
            logic::detect(text, self.cursor)
        } else {
            None
        };
        let describe = |k: &str, f: &str| tt(v, k, f);
        let items: Vec<Item> = trigger
            .as_ref()
            .map(|t| logic::candidates(t, v.members(), &v.state.tasks, &describe))
            .unwrap_or_default();
        if self.selected >= items.len() {
            self.selected = 0;
        }
        let mut chosen: Option<usize> = None;
        let mut dismissed = false;
        if focused && !items.is_empty() {
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown) {
                    self.selected = (self.selected + 1) % items.len();
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) {
                    self.selected = (self.selected + items.len() - 1) % items.len();
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Tab)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                {
                    chosen = Some(self.selected);
                }
                if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                    dismissed = true;
                }
            });
        } else if focused
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
        {
            ev.send = !text.trim().is_empty() || !self.attachments.is_empty();
        }
        if let (Some(i), Some(t)) = (chosen, trigger.as_ref()) {
            let (new, cur) = logic::apply(text, t, &items[i].insert);
            *text = new;
            self.cursor = cur;
            if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                st.cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(cur))));
                st.store(ui.ctx(), id);
            }
            self.selected = 0;
        }

        // Pasted text that is an existing file path attaches the file.
        let pasted: Vec<String> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Paste(s) => Some(s.clone()),
                    _ => None,
                })
                .collect()
        });
        if focused {
            for p in pasted {
                let p = p.trim().trim_matches('\'');
                if !p.is_empty()
                    && !p.contains('\n')
                    && Path::new(p).is_absolute()
                    && Path::new(p).is_file()
                {
                    ui.input_mut(|i| i.events.retain(|e| !matches!(e, egui::Event::Paste(_))));
                    let r = self.attach_path(Path::new(p));
                    self.report(r);
                }
            }
        }

        // A pasted clipboard image (egui-winit emits it when the clipboard has no text).
        if focused {
            let imgs: Vec<std::sync::Arc<egui::ColorImage>> = ui.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::PasteImage(img) => Some(img.clone()),
                        _ => None,
                    })
                    .collect()
            });
            for img in imgs {
                let r = self.attach_pasted_image(&img);
                self.report(r);
            }
        }

        let frame = egui::Frame::NONE;
        let group = frame.show(ui, |ui| {
            self.chips(ui);
            let out = egui::TextEdit::multiline(text)
                .id(id)
                .hint_text(tt(
                    v,
                    "team.messageComposer.input.placeholder",
                    "Message the team",
                ))
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .frame(egui::Frame::NONE)
                .show(ui);
            if let Some(r) = out.cursor_range {
                self.cursor = r.primary.index.0;
            }
            if std::mem::take(&mut self.focus) {
                out.response.request_focus();
            }
            ui.horizontal(|ui| {
                if ui
                    .button(tt(v, "rteams.composer.attach", "Attach file"))
                    .clicked()
                {
                    let mut d = rfd::FileDialog::new();
                    if let Some(p) = v.team().and_then(|t| t.project_path.as_ref()) {
                        d = d.set_directory(p);
                    }
                    if let Some(files) = d.pick_files() {
                        for f in files {
                            let r = self.attach_path(&f);
                            self.report(r);
                        }
                    }
                }
                if ui
                    .button(tt(v, "rteams.composer.code", "Attach code context"))
                    .clicked()
                {
                    self.code.open = true;
                    if self.code.end == 0 {
                        self.code.start = 1;
                        self.code.end = 20;
                    }
                }
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
            });
            out.response.rect
        });
        let edit_rect = group.inner;

        if let (Some(t), false) = (trigger.as_ref(), items.is_empty()) {
            if !dismissed {
                self.popup(ui, edit_rect, t, &items, text, id);
            }
        }
        self.code_picker(ui, v, text);

        // Drag-drop onto the composer.
        let dropped: Vec<PathBuf> = ui.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() && ui.rect_contains_pointer(group.response.rect.union(edit_rect)) {
            for f in dropped {
                let r = self.attach_path(&f);
                self.report(r);
            }
        }

        if ev.send {
            ev.attachments = std::mem::take(&mut self.attachments);
            self.error = None;
        }
        ev
    }

    fn chips(&mut self, ui: &mut egui::Ui) {
        if self.attachments.is_empty() {
            return;
        }
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            for (i, a) in self.attachments.iter().enumerate() {
                egui::Frame::NONE
                    .fill(ui.visuals().faint_bg_color)
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(6, 2))
                    .show(ui, |ui| {
                        if rteams_services::attachments::is_image(a) {
                            let tex = self.thumbs.entry(a.path.clone()).or_insert_with(|| {
                                rteams_services::attachments::thumbnail(Path::new(&a.path), 64).map(
                                    |(w, h, px)| {
                                        let ci = egui::ColorImage::from_rgba_unmultiplied(
                                            [w as usize, h as usize],
                                            &px,
                                        );
                                        ui.ctx().load_texture(
                                            format!("composer-thumb-{}", a.path),
                                            ci,
                                            egui::TextureOptions::LINEAR,
                                        )
                                    },
                                )
                            });
                            if let Some(t) = tex {
                                ui.add(egui::Image::new(&*t).max_height(32.0));
                            }
                        }
                        ui.label(&a.name).on_hover_text(&a.path);
                        if ui.small_button("x").clicked() {
                            remove = Some(i);
                        }
                    });
            }
        });
        if let Some(i) = remove {
            self.attachments.remove(i);
        }
    }

    fn popup(
        &mut self,
        ui: &mut egui::Ui,
        anchor: egui::Rect,
        t: &Trigger,
        items: &[Item],
        text: &mut String,
        id: egui::Id,
    ) {
        let mut pick = None;
        egui::Area::new(ui.make_persistent_id("composer-popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor.left_top() - egui::vec2(0.0, 4.0 + 24.0 * items.len() as f32))
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(240.0);
                    for (i, it) in items.iter().enumerate() {
                        let r = ui.selectable_label(
                            i == self.selected,
                            format!("{}  {}", it.label, it.detail),
                        );
                        if r.clicked() {
                            pick = Some(i);
                        }
                    }
                });
            });
        if let Some(i) = pick {
            let (new, cur) = logic::apply(text, t, &items[i].insert);
            *text = new;
            self.cursor = cur;
            if let Some(mut st) = egui::TextEdit::load_state(ui.ctx(), id) {
                st.cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(cur))));
                st.store(ui.ctx(), id);
            }
            ui.memory_mut(|m| m.request_focus(id));
        }
    }

    fn code_picker(&mut self, ui: &mut egui::Ui, v: &View, text: &mut String) {
        if !self.code.open {
            return;
        }
        let root = v.team().and_then(|t| t.project_path.clone());
        let mut open = true;
        let mut insert = false;
        egui::Window::new(tt(v, "rteams.composer.code", "Attach code context"))
            .open(&mut open)
            .collapsible(false)
            .show(ui.ctx(), |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.code.path).hint_text("path/to/file"),
                    );
                    if ui
                        .button(tt(v, "rteams.composer.browse", "Browse"))
                        .clicked()
                    {
                        let mut d = rfd::FileDialog::new();
                        if let Some(r) = &root {
                            d = d.set_directory(r);
                        }
                        if let Some(f) = d.pick_file() {
                            self.code.path = f.to_string_lossy().into_owned();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tt(v, "rteams.composer.lines", "Lines"));
                    ui.add(egui::DragValue::new(&mut self.code.start).range(1..=1_000_000));
                    ui.label("-");
                    ui.add(egui::DragValue::new(&mut self.code.end).range(1..=1_000_000));
                });
                if let Some(e) = &self.code.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                if ui
                    .button(tt(v, "rteams.composer.insert", "Insert"))
                    .clicked()
                {
                    insert = true;
                }
            });
        if insert {
            match attach_code(
                &self.code.path,
                root.as_deref(),
                self.code.start,
                self.code.end,
            ) {
                Ok((snip, att)) => {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&snip);
                    self.push(att);
                    self.code.error = None;
                    open = false;
                }
                Err(e) => self.code.error = Some(e),
            }
        }
        self.code.open = open;
    }
}

/// Encode RGBA8 pixels as PNG.
fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Resolve `path` (relative paths are under the project root), read the line range and build
/// the fenced snippet plus its attachment (named `rel/path:start-end`, pointing at the file).
pub fn attach_code(
    path: &str,
    root: Option<&str>,
    start: usize,
    end: usize,
) -> Result<(String, Attachment), String> {
    let p = Path::new(path.trim());
    let full = if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(root.ok_or("no project path")?).join(p)
    };
    if let Some(r) = root {
        // Keep the picker inside the project: no `..` escapes for relative input.
        let canon = full.canonicalize().map_err(|e| e.to_string())?;
        let rc = Path::new(r).canonicalize().map_err(|e| e.to_string())?;
        if !canon.starts_with(&rc) {
            return Err("file is outside the project".into());
        }
    }
    let meta = std::fs::metadata(&full).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_ATTACH_BYTES {
        return Err("not a readable file".into());
    }
    let content = std::fs::read_to_string(&full).map_err(|e| e.to_string())?;
    let (s, e, code) = logic::slice_lines(&content, start, end).ok_or("file is empty")?;
    let rel = root.and_then(|r| full.strip_prefix(r).ok()).map_or_else(
        || full.to_string_lossy().into_owned(),
        |r| r.to_string_lossy().into_owned(),
    );
    let range = if s == e {
        s.to_string()
    } else {
        format!("{s}-{e}")
    };
    Ok((
        logic::snippet(&rel, s, e, &code),
        Attachment {
            name: format!("{rel}:{range}"),
            path: full.to_string_lossy().into_owned(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::{fixture, render, tr};

    #[test]
    fn renders_headless() {
        let (home, data) = fixture("composer-render");
        let v = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: tr(),
        };
        let mut c = Composer::default();
        let mut text = String::from("hi @a");
        render(|ui| {
            let ev = c.show(ui, &v, &mut text);
            assert!(!ev.send);
        });
        let mut empty = String::new();
        c.code.open = true;
        render(|ui| {
            c.show(ui, &v, &mut empty);
        });
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn attach_copies_and_dedups() {
        let dir = std::env::temp_dir().join(format!("composer-att-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.txt");
        std::fs::write(&src, "x").unwrap();
        let mut c = Composer {
            attach_dir: Some(dir.join("att")),
            ..Default::default()
        };
        c.attach_path(&src).unwrap();
        assert_eq!(c.attachments().len(), 1);
        assert!(Path::new(&c.attachments()[0].path).starts_with(dir.join("att")));
        assert!(c.attach_path(&dir.join("nope")).is_err());
        c.attach_bytes("img.png", b"png").unwrap();
        assert_eq!(c.attachments().len(), 2);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn pasted_image_becomes_png_attachment() {
        let dir = std::env::temp_dir().join(format!("composer-paste-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut c = Composer {
            attach_dir: Some(dir.join("att")),
            ..Default::default()
        };
        let img = egui::ColorImage::filled([3, 2], egui::Color32::from_rgb(10, 20, 30));
        c.attach_pasted_image(&img).unwrap();
        let a = &c.attachments()[0];
        assert!(a.name.starts_with("pasted-") && a.name.ends_with(".png"));
        assert!(rteams_services::attachments::is_image(a));
        let (w, h, px) = rteams_services::attachments::thumbnail(Path::new(&a.path), 64).unwrap();
        assert_eq!((w, h), (3, 2));
        assert_eq!(&px[..4], &[10, 20, 30, 255]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn code_context_snippet() {
        let dir = std::env::temp_dir().join(format!("composer-code-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/m.rs"), "a\nb\nc\n").unwrap();
        let root = dir.to_string_lossy().into_owned();
        let (snip, att) = attach_code("src/m.rs", Some(&root), 2, 3).unwrap();
        assert!(snip.contains("`src/m.rs:2-3`") && snip.contains("b\nc"));
        assert_eq!(att.name, "src/m.rs:2-3");
        assert!(attach_code("../x", Some(&root), 1, 1).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
