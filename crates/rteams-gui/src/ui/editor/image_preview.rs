//! Image preview (zoomable) and read-only office document preview.
//!
//! The shell must call `egui_extras::install_image_loaders(ctx)` once at startup; without
//! it images show a "no loader" error.

use std::path::Path;

use eframe::egui::{self, RichText};
use rteams_services::docpreview::{format_bytes, DocPreview, Sheet};

#[derive(Clone, Copy)]
struct Zoom {
    /// 0.0 = fit to width.
    factor: f32,
}

fn file_uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

pub fn next_zoom(current: f32, zoom_in: bool) -> f32 {
    const STEPS: [f32; 11] = [0.1, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0];
    let cur = if current <= 0.0 { 1.0 } else { current };
    if zoom_in {
        STEPS
            .iter()
            .copied()
            .find(|s| *s > cur + 1e-3)
            .unwrap_or(8.0)
    } else {
        STEPS
            .iter()
            .rev()
            .copied()
            .find(|s| *s < cur - 1e-3)
            .unwrap_or(0.1)
    }
}

/// Show an image file with zoom controls (Fit, 100%, +, -; Ctrl/Cmd + scroll).
pub fn show(ui: &mut egui::Ui, path: &Path) {
    let id = ui.scope_id().with(("img_zoom", path));
    let mut z = ui
        .data(|d| d.get_temp::<Zoom>(id))
        .unwrap_or(Zoom { factor: 0.0 });
    ui.horizontal(|ui| {
        if ui.button("-").clicked() {
            z.factor = next_zoom(z.factor, false);
        }
        if ui.button("+").clicked() {
            z.factor = next_zoom(z.factor, true);
        }
        if ui.button("Fit").clicked() {
            z.factor = 0.0;
        }
        if ui.button("100%").clicked() {
            z.factor = 1.0;
        }
        ui.weak(if z.factor <= 0.0 {
            "Fit".to_string()
        } else {
            format!("{}%", (z.factor * 100.0).round())
        });
        ui.weak(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        );
    });
    ui.separator();
    if !path.exists() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("File not found: {}", path.display()),
        );
        return;
    }
    let scroll = ui.input(|i| {
        if i.modifiers.command {
            i.smooth_scroll_delta.y
        } else {
            0.0
        }
    });
    if scroll.abs() > 0.5 {
        z.factor = next_zoom(z.factor, scroll > 0.0);
    }
    let avail_w = ui.available_width();
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let mut img = egui::Image::new(file_uri(path));
            img = if z.factor <= 0.0 {
                img.max_width(avail_w).shrink_to_fit()
            } else {
                img.fit_to_original_size(z.factor)
            };
            ui.add(img);
        });
    ui.data_mut(|d| d.insert_temp(id, z));
}

fn show_sheet(ui: &mut egui::Ui, sheet: &Sheet, idx: usize) {
    let cols = sheet.rows.iter().map(Vec::len).max().unwrap_or(0);
    egui::ScrollArea::both()
        .id_salt(("sheet_scroll", idx))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new(("sheet", idx))
                .striped(true)
                .min_col_width(40.0)
                .show(ui, |ui| {
                    ui.weak("");
                    for c in 0..cols {
                        ui.weak(col_name(c));
                    }
                    ui.end_row();
                    for (r, row) in sheet.rows.iter().enumerate() {
                        ui.weak((r + 1).to_string());
                        for c in 0..cols {
                            ui.label(row.get(c).map_or("", String::as_str));
                        }
                        ui.end_row();
                    }
                });
        });
}

/// Spreadsheet column letters: 0 -> A, 26 -> AA.
pub fn col_name(mut i: usize) -> String {
    let mut s = Vec::new();
    loop {
        s.push((b'A' + (i % 26) as u8) as char);
        if i < 26 {
            break;
        }
        i = i / 26 - 1;
    }
    s.iter().rev().collect()
}

/// Read-only view of a [`DocPreview`] (docx text, pptx slides, xlsx / csv tables).
pub fn show_document(ui: &mut egui::Ui, doc: &DocPreview) {
    match doc {
        DocPreview::Document {
            paragraphs,
            truncated,
        } => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for p in paragraphs {
                        if p.is_empty() {
                            ui.add_space(6.0);
                        } else {
                            ui.add(egui::Label::new(p.as_str()).selectable(true));
                        }
                    }
                    if *truncated {
                        ui.weak("Preview truncated.");
                    }
                });
        }
        DocPreview::Slides { slides, truncated } => {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for (i, s) in slides.iter().enumerate() {
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(RichText::new(format!("Slide {}", i + 1)).strong());
                            for l in s {
                                ui.add(egui::Label::new(l.as_str()).selectable(true));
                            }
                        });
                    }
                    if *truncated {
                        ui.weak("Preview truncated.");
                    }
                });
        }
        DocPreview::Sheets(sheets) => {
            let id = ui.scope_id().with("sheet_sel");
            let mut sel = ui
                .data(|d| d.get_temp::<usize>(id))
                .unwrap_or(0)
                .min(sheets.len().saturating_sub(1));
            if sheets.len() > 1 {
                ui.horizontal(|ui| {
                    for (i, s) in sheets.iter().enumerate() {
                        ui.selectable_value(&mut sel, i, &s.name);
                    }
                });
            }
            if let Some(s) = sheets.get(sel) {
                if s.truncated {
                    ui.weak("Only the first rows and columns are shown.");
                }
                show_sheet(ui, s, sel);
            }
            ui.data_mut(|d| d.insert_temp(id, sel));
        }
        DocPreview::TooLarge { size, limit } => {
            ui.label(format!(
                "File is too large to preview ({}). Limit: {}.",
                format_bytes(*size),
                format_bytes(*limit)
            ));
        }
        DocPreview::Unsupported { ext } => {
            ui.label(format!(
                "No preview available for .{ext} files. Open it externally."
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::docpreview::{DocPreview, Sheet};

    #[test]
    fn zoom_steps() {
        assert_eq!(next_zoom(0.0, true), 1.5);
        assert_eq!(next_zoom(1.0, false), 0.75);
        assert_eq!(next_zoom(8.0, true), 8.0);
        assert_eq!(next_zoom(0.1, false), 0.1);
    }

    #[test]
    fn column_names() {
        assert_eq!(col_name(0), "A");
        assert_eq!(col_name(25), "Z");
        assert_eq!(col_name(26), "AA");
        assert_eq!(col_name(27), "AB");
    }

    #[test]
    fn renders_documents_and_missing_image() {
        let docs = [
            DocPreview::Document {
                paragraphs: vec!["a".into(), String::new(), "b".into()],
                truncated: true,
            },
            DocPreview::Slides {
                slides: vec![vec!["t".into()]],
                truncated: false,
            },
            DocPreview::Sheets(vec![
                Sheet {
                    name: "S1".into(),
                    rows: vec![vec!["a".into(), "b".into()]],
                    truncated: true,
                },
                Sheet {
                    name: "S2".into(),
                    rows: vec![],
                    truncated: false,
                },
            ]),
            DocPreview::TooLarge {
                size: 30 << 20,
                limit: 25 << 20,
            },
            DocPreview::Unsupported { ext: "pdf".into() },
        ];
        for d in &docs {
            crate::view::testing::render(|ui| show_document(ui, d));
        }
        crate::view::testing::render(|ui| show(ui, Path::new("/nonexistent/x.png")));
    }

    #[test]
    fn renders_real_image_file() {
        let dir = std::env::temp_dir().join(format!("rteams-img-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.svg");
        std::fs::write(
            &p,
            "<svg xmlns='http://www.w3.org/2000/svg' width='4' height='4'/>",
        )
        .unwrap();
        crate::view::testing::render(|ui| {
            egui_extras::install_image_loaders(ui.ctx());
            show(ui, &p)
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
}
