//! Soft layout pieces shared by the Teams, Dashboard and secondary screens: fixed-size tiles in
//! a responsive grid, section headers with vector icons, empty states and loading skeletons.

use eframe::egui::{self, Color32, CornerRadius, Rect, Response, RichText, Sense, Stroke, Vec2};

use crate::icons::{self, Icon};
use crate::theme;

pub const GAP: f32 = 14.0;

/// Column count and tile width for a grid of tiles at least `min_w` wide.
pub fn columns(avail: f32, min_w: f32) -> (usize, f32) {
    let avail = avail.max(1.0);
    let cols = (((avail + GAP) / (min_w + GAP)).floor() as usize).max(1);
    let w = ((avail - GAP * (cols as f32 - 1.0)) / cols as f32).max(1.0);
    (cols, w)
}

/// Lay `items` out in rows of equal tiles; `add(ui, width, item)` draws one tile.
pub fn grid<T>(
    ui: &mut egui::Ui,
    min_w: f32,
    items: &[T],
    mut add: impl FnMut(&mut egui::Ui, f32, &T),
) {
    let (cols, w) = columns(ui.available_width(), min_w);
    for row in items.chunks(cols) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for it in row {
                add(ui, w, it);
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y.min(GAP - 2.0));
    }
}

/// Fixed-size soft tile (`w` x `h`): rounded, hairline border, lifts on hover. Content is
/// drawn inside a 16px padding; the whole tile is clickable (inner buttons win).
pub fn tile(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    w: f32,
    h: f32,
    selected: bool,
    add: impl FnOnce(&mut egui::Ui),
) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    let resp = ui.interact(rect, ui.scope_id().with(id), Sense::click());
    let (fill, stroke) = if selected {
        (
            theme::tint(theme::ACCENT, 0.08),
            theme::tint(theme::ACCENT, 0.45),
        )
    } else if resp.hovered() {
        (Color32::from_rgb(0x20, 0x22, 0x2d), theme::hairline(0.16))
    } else {
        (theme::RAISED, theme::hairline(0.08))
    };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    ui.painter().rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(Vec2::new(16.0, 14.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(inner)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    // Labels and pills keep their natural width (clipped by the tile), never one char wide.
    child.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
    child.set_clip_rect(rect.shrink(1.0).intersect(ui.clip_rect()));
    add(&mut child);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Section title: 16px icon, 16px bold text and an optional count badge.
pub fn heading(ui: &mut egui::Ui, icon: Icon, title: &str, count: Option<usize>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        icons::icon(ui, icon, 16.0, theme::TEXT_2);
        ui.label(RichText::new(title).size(16.0).strong());
        if let Some(n) = count {
            theme::count_badge(ui, n, false);
        }
    });
}

/// Centered soft card for "nothing here" states: icon tile, title and a description.
pub fn empty_state(ui: &mut egui::Ui, icon: Icon, title: &str, description: &str) {
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        egui::Frame::new()
            .fill(theme::tint(theme::RAISED, 0.55))
            .stroke(Stroke::new(1.0, theme::hairline(0.06)))
            .corner_radius(CornerRadius::same(14))
            .inner_margin(egui::Margin::symmetric(28, 24))
            .show(ui, |ui| {
                ui.set_max_width(460.0);
                ui.vertical_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                    ui.painter().rect_filled(
                        r,
                        CornerRadius::same(12),
                        theme::tint(theme::ACCENT, 0.12),
                    );
                    icons::draw(
                        ui.painter(),
                        Rect::from_center_size(r.center(), Vec2::splat(22.0)),
                        icon,
                        theme::ACCENT,
                    );
                    ui.add_space(6.0);
                    ui.label(RichText::new(title).size(15.0).strong());
                    if !description.is_empty() {
                        ui.label(RichText::new(description).color(theme::TEXT_2));
                    }
                });
            });
    });
    ui.add_space(8.0);
}

/// One rounded grey placeholder block.
pub fn skeleton(ui: &mut egui::Ui, w: f32, h: f32) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    ui.painter().rect_filled(
        r,
        CornerRadius::same(8),
        Color32::from_rgb(0x1a, 0x1b, 0x24),
    );
}

/// `n` placeholder tiles in the grid, with a caption so the page never looks blank.
pub fn skeleton_tiles(ui: &mut egui::Ui, n: usize, min_w: f32, h: f32, caption: &str) {
    let items: Vec<usize> = (0..n).collect();
    grid(ui, min_w, &items, |ui, w, _| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        p.rect_filled(rect, CornerRadius::same(12), theme::RAISED);
        let bar = |y: f32, f: f32, a: u8| {
            let r = Rect::from_min_size(
                rect.min + Vec2::new(16.0, y),
                Vec2::new((w - 32.0) * f, 12.0),
            );
            p.rect_filled(r, CornerRadius::same(6), Color32::from_white_alpha(a));
        };
        bar(16.0, 0.5, 18);
        bar(42.0, 0.8, 10);
        bar(62.0, 0.35, 10);
    });
    if !caption.is_empty() {
        ui.label(RichText::new(caption).color(theme::MUTED));
    }
}

/// Pill-shaped search box, 34px high, 14px text.
pub fn search_box(ui: &mut egui::Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let frame = egui::Frame::new()
        .fill(theme::CARD)
        .stroke(Stroke::new(1.0, theme::hairline(0.1)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(12, 4));
    frame
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                icons::icon(ui, Icon::Search, 14.0, theme::MUTED);
                ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(hint)
                        .frame(egui::Frame::NONE)
                        .desired_width(width - 48.0),
                )
            })
            .inner
        })
        .inner
}

/// "Create Team" modal: asks for a directory-safe name. Returns the trimmed name once confirmed
/// (and closes itself); `open` is cleared on cancel / Escape.
pub fn create_team_dialog(
    ctx: &egui::Context,
    tr: &rteams_services::i18n::I18n,
    open: &mut bool,
    name: &mut String,
) -> Option<String> {
    if !*open {
        return None;
    }
    let create = std::cell::Cell::new(false);
    let cancel = std::cell::Cell::new(false);
    let valid = crate::teams::valid_name(name);
    let mut still_open = true;
    let buf = std::cell::RefCell::new(std::mem::take(name));
    theme::dialog_with_footer(
        ctx,
        "create_team_dialog",
        &tr.t("team.list.actions.createTeam"),
        420.0,
        &mut still_open,
        |ui| {
            ui.label(RichText::new(tr.t("rteams.teams.newName")).color(theme::TEXT_2));
            let mut b = buf.borrow_mut();
            let r = ui.add(
                egui::TextEdit::singleline(&mut *b)
                    .hint_text(tr.t("rteams.teams.newName"))
                    .desired_width(f32::INFINITY),
            );
            let ok = crate::teams::valid_name(&b);
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && ok {
                create.set(true);
            }
            if !b.trim().is_empty() && !ok {
                ui.label(
                    RichText::new("Use letters, digits, '.', '_' or '-'.")
                        .size(12.5)
                        .color(theme::WARNING),
                );
            }
        },
        |ui| {
            let b = egui::Button::new(
                RichText::new(tr.t("team.list.actions.createTeam"))
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(theme::ACCENT)
            .corner_radius(CornerRadius::same(255));
            let ok = crate::teams::valid_name(&buf.borrow());
            if ui.add_enabled(ok, b).clicked() {
                create.set(true);
            }
            if theme::pill_button(ui, &tr.t("common.actions.cancel"), false).clicked() {
                cancel.set(true);
            }
        },
    );
    let _ = valid;
    *name = buf.into_inner();
    if create.get() && crate::teams::valid_name(name) {
        *open = false;
        return Some(std::mem::take(name).trim().to_string());
    }
    if cancel.get() || !still_open {
        *open = false;
    }
    None
}

/// Make checkboxes / radio buttons readable on dark cards: lighter box and a visible outline.
pub fn checkbox_visuals(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.bg_fill = Color32::from_rgb(0x2a, 0x2c, 0x3a);
    w.inactive.bg_stroke = Stroke::new(1.0, theme::hairline(0.45));
    w.hovered.bg_stroke = Stroke::new(1.0, theme::tint(theme::ACCENT, 0.8));
}
