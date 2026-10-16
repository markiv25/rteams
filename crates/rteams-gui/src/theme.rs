//! Look and feel, matched to agent-teams' dark "Midnight Slate" theme: near-black indigo
//! surfaces, hairline borders, flat cards, colored glows, member color chips. Every view takes
//! its colors and small widgets from here so the screens stay consistent.

use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Rect, Response, RichText, Sense, Stroke,
    TextStyle, Vec2,
};

use crate::icons::{self, Icon};

// ---- surface tokens ----
pub const SURFACE: Color32 = Color32::from_rgb(0x12, 0x13, 0x1a);
pub const RAISED: Color32 = Color32::from_rgb(0x1c, 0x1d, 0x26);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x0c, 0x0d, 0x13);
pub const CARD: Color32 = Color32::from_rgb(0x0f, 0x10, 0x18);
pub const CARD_HEADER: Color32 = Color32::from_rgb(0x15, 0x16, 0x20);
pub const CODE_BG: Color32 = Color32::from_rgb(0x17, 0x18, 0x22);

// ---- text ----
pub const TEXT: Color32 = Color32::from_rgb(0xf1, 0xf5, 0xf9);
pub const TEXT_2: Color32 = Color32::from_rgb(0x94, 0xa3, 0xb8);
pub const MUTED: Color32 = Color32::from_rgb(0x64, 0x74, 0x8b);
pub const ACCENT: Color32 = Color32::from_rgb(0x81, 0x8c, 0xf8);

// ---- semantic ----
pub const SUCCESS: Color32 = Color32::from_rgb(0x22, 0xc5, 0x5e);
pub const ERROR: Color32 = Color32::from_rgb(0xef, 0x44, 0x44);
pub const WARNING: Color32 = Color32::from_rgb(0xf5, 0x9e, 0x0b);
pub const INFO: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
pub const SEND: Color32 = Color32::from_rgb(0x38, 0xbd, 0xf8);

// ---- kanban column accents ----
pub const COL_TODO: Color32 = Color32::from_rgb(0x3b, 0x82, 0xf6);
pub const COL_IN_PROGRESS: Color32 = Color32::from_rgb(0xea, 0xb3, 0x08);
pub const COL_REVIEW: Color32 = Color32::from_rgb(0x8b, 0x5c, 0xf6);
pub const COL_DONE: Color32 = Color32::from_rgb(0x14, 0xb8, 0xa6);
pub const COL_APPROVED: Color32 = Color32::from_rgb(0x65, 0xa3, 0x0d);

// ---- log blocks (thinking / tool call / tool result / error) ----
pub const THINK_FG: Color32 = Color32::from_rgb(0xd8, 0xb4, 0xfe);
pub const TOOL_FG: Color32 = Color32::from_rgb(0xfc, 0xd3, 0x4d);
pub const RESULT_FG: Color32 = Color32::from_rgb(0x86, 0xef, 0xac);
pub const ERROR_FG: Color32 = Color32::from_rgb(0xfc, 0xa5, 0xa5);

/// Hairline border: slate-400 at `alpha` (0.07 normal, 0.12 emphasis).
pub fn hairline(alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(148, 163, 184, (alpha * 255.0) as u8)
}

/// `c` at `alpha` (0..1), for chip fills and glows.
pub fn tint(c: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha * 255.0) as u8)
}

/// Member palette in assignment order after the lead (who is always green).
const PALETTE: [Color32; 12] = [
    Color32::from_rgb(0x60, 0xa5, 0xfa), // blue
    Color32::from_rgb(0xfd, 0xe0, 0x47), // saffron
    Color32::from_rgb(0x5e, 0xea, 0xd4), // turquoise
    Color32::from_rgb(0xf8, 0x71, 0x71), // brick
    Color32::from_rgb(0xc4, 0xb5, 0xfd), // indigo
    Color32::from_rgb(0x86, 0xef, 0xac), // forest
    Color32::from_rgb(0xfd, 0xba, 0x74), // apricot
    Color32::from_rgb(0xfd, 0xa4, 0xaf), // rose
    Color32::from_rgb(0x7d, 0xd3, 0xfc), // cerulean
    Color32::from_rgb(0xbe, 0xf2, 0x64), // olive
    Color32::from_rgb(0xe0, 0x9f, 0x6e), // copper
    Color32::from_rgb(0x9c, 0xb4, 0xcc), // steel
];
const LEAD_COLOR: Color32 = Color32::from_rgb(0x4a, 0xde, 0x80);

/// Stable color for `name`: leads are green, others follow roster order through the palette.
/// Unknown names (e.g. removed members) hash into the palette.
pub fn member_color(team: Option<&rteams_core::Team>, name: &str) -> Color32 {
    if let Some(t) = team {
        if t.members.iter().any(|m| m.name == name && m.role == "lead") {
            return LEAD_COLOR;
        }
        let others = t.members.iter().filter(|m| m.role != "lead");
        if let Some(i) = others.map(|m| m.name.as_str()).position(|n| n == name) {
            return PALETTE[i % PALETTE.len()];
        }
    }
    let h = name
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    PALETTE[h as usize % PALETTE.len()]
}

/// System fonts used as fallback after egui's bundled ones, so catalog text in any script
/// (CJK, arrows, box drawing, ...) renders instead of empty boxes. First existing file wins per
/// group; nothing fails if none exist.
const FALLBACK_FONTS: &[&[&str]] = &[
    &[
        "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
        "/Library/Fonts/Arial Unicode.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
    ],
    &[
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
    ],
];

/// The OS UI font, so text looks native (soft) instead of egui's bundled Ubuntu.
const UI_FONTS: &[&str] = &[
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/HelveticaNeue.ttc",
    "C:\\Windows\\Fonts\\segoeui.ttf",
    "/usr/share/fonts/truetype/inter/Inter-Regular.ttf",
    "/usr/share/fonts/opentype/inter/Inter-Regular.otf",
    "/usr/share/fonts/inter/Inter-Regular.ttf",
    "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
    "/usr/share/fonts/opentype/cantarell/Cantarell-Regular.otf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
];

/// The OS monospace font.
const MONO_FONTS: &[&str] = &[
    "/System/Library/Fonts/SFNSMono.ttf",
    "/System/Library/Fonts/Menlo.ttc",
    "C:\\Windows\\Fonts\\consola.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
];

fn first_font(paths: &[&str]) -> Option<Vec<u8>> {
    paths.iter().find_map(|p| std::fs::read(p).ok())
}

/// Font stacks: OS UI font first, then egui's emoji fonts, the system fallbacks, and the bundled
/// Ubuntu as the last resort (monospace: OS mono, then bundled Hack, then the same tail).
fn font_definitions() -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();
    let mut tail: Vec<String> = Vec::new();
    for (i, group) in FALLBACK_FONTS.iter().enumerate() {
        let Some(bytes) = first_font(group) else {
            continue;
        };
        let name = format!("system-fallback-{i}");
        defs.font_data
            .insert(name.clone(), egui::FontData::from_owned(bytes).into());
        tail.push(name);
    }
    for (family, paths, name) in [
        (FontFamily::Proportional, UI_FONTS, "system-ui"),
        (FontFamily::Monospace, MONO_FONTS, "system-mono"),
    ] {
        let list = defs.families.entry(family.clone()).or_default();
        let bundled: Vec<String> = std::mem::take(list);
        let (ubuntu, rest): (Vec<_>, Vec<_>) =
            bundled.into_iter().partition(|n| n.starts_with("Ubuntu"));
        if let Some(bytes) = first_font(paths) {
            defs.font_data
                .insert(name.into(), egui::FontData::from_owned(bytes).into());
            list.push(name.into());
        }
        list.extend(rest);
        list.extend(tail.iter().cloned());
        list.extend(ubuntu);
    }
    defs
}

fn shadow(blur: u8, alpha: u8) -> egui::epaint::Shadow {
    egui::epaint::Shadow {
        offset: [0, 8],
        blur,
        spread: 0,
        color: Color32::from_black_alpha(alpha),
    }
}

/// Install fonts, sizes, spacing and dark visuals. Call once at startup.
pub fn apply(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(20.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
    ]
    .into();
    // Every label is selectable: drag to select, Cmd/Ctrl+C to copy (chat, comments, logs...).
    style.interaction.selectable_labels = true;
    style.spacing.item_spacing = Vec2::new(10.0, 8.0);
    style.spacing.button_padding = Vec2::new(10.0, 6.0);
    style.spacing.interact_size.y = 28.0;
    style.spacing.window_margin = egui::Margin::same(20);
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.menu_spacing = 2.0;

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = SURFACE;
    v.window_fill = RAISED;
    v.extreme_bg_color = CARD;
    v.faint_bg_color = CARD_HEADER;
    v.code_bg_color = CODE_BG;
    v.override_text_color = Some(TEXT);
    v.hyperlink_color = ACCENT;
    v.window_stroke = Stroke::new(1.0, hairline(0.12));
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = shadow(24, 90);
    v.popup_shadow = shadow(24, 90);
    v.selection.bg_fill = tint(ACCENT, 0.30);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, hairline(0.07));
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_2);
    v.widgets.noninteractive.corner_radius = CornerRadius::same(8);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(8);
    }
    v.widgets.inactive.bg_fill = RAISED;
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, hairline(0.07));
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_2);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x25, 0x27, 0x33);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x25, 0x27, 0x33);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, hairline(0.12));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.bg_fill = Color32::from_rgb(0x2c, 0x2e, 0x3b);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x2c, 0x2e, 0x3b);
    v.widgets.active.bg_stroke = Stroke::new(1.0, hairline(0.18));
    v.widgets.active.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.active.expansion = 0.0;
    v.widgets.open.bg_fill = Color32::from_rgb(0x25, 0x27, 0x33);
    v.widgets.open.weak_bg_fill = Color32::from_rgb(0x25, 0x27, 0x33);
    ctx.set_global_style(style);
}

// ---- small widgets ----

/// Rounded pill: `color` text on `color` at 15%.
pub fn pill(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> Response {
    let text = text.into();
    egui::Frame::new()
        .fill(tint(color, 0.15))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        })
        .response
}

/// Small round count badge (e.g. unread). `strong` = blue fill with white text.
pub fn count_badge(ui: &mut egui::Ui, n: usize, strong: bool) -> Response {
    let (fill, fg) = if strong {
        (INFO, Color32::WHITE)
    } else {
        (RAISED, TEXT_2)
    };
    let text = if n > 99 {
        "99+".to_string()
    } else {
        n.to_string()
    };
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).strong().color(fg));
        })
        .response
}

/// Presence of an agent, drawn as the dot on its avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Online,
    Offline,
}

/// Round avatar: colored ring, dark fill, the member's icon (see `names::icon_for`), optional
/// presence dot.
pub fn avatar(
    ui: &mut egui::Ui,
    name: &str,
    color: Color32,
    size: f32,
    presence: Option<Presence>,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let p = ui.painter();
    let c = rect.center();
    let r = size / 2.0;
    p.circle_filled(c, r, tint(color, 0.18));
    p.circle_stroke(c, r - 1.0, Stroke::new(2.0, color));
    p.text(
        c,
        egui::Align2::CENTER_CENTER,
        crate::names::icon_for(ui.ctx(), name),
        FontId::proportional(size * 0.46),
        color,
    );
    if let Some(pr) = presence {
        let dot = match pr {
            Presence::Online => SUCCESS,
            Presence::Offline => ERROR,
        };
        let d = (size * 0.26).max(7.0);
        let at = rect.right_bottom() - Vec2::splat(d / 2.0);
        p.circle_filled(at, d / 2.0 + 2.0, SIDEBAR);
        p.circle_filled(at, d / 2.0, dot);
    }
    resp
}

/// Soft vertical glow from `color` at the top of `rect` fading to transparent.
pub fn glow(painter: &egui::Painter, rect: Rect, color: Color32, height: f32) {
    use egui::epaint::{Mesh, Vertex, WHITE_UV};
    let top = tint(color, 0.16);
    let bottom = tint(color, 0.0);
    let h = height.min(rect.height());
    let mut mesh = Mesh::default();
    let pts = [
        (rect.left_top(), top),
        (rect.right_top(), top),
        (egui::pos2(rect.right(), rect.top() + h), bottom),
        (egui::pos2(rect.left(), rect.top() + h), bottom),
    ];
    for (pos, color) in pts {
        mesh.vertices.push(Vertex {
            pos,
            uv: WHITE_UV,
            color,
        });
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Collapsible flat section header (icon + title + count + chevron), as in the team view.
/// Mirrors the original's section props one-to-one, hence the argument count.
#[allow(clippy::too_many_arguments)]
pub fn section(
    ui: &mut egui::Ui,
    id: &str,
    icon: &str,
    title: &str,
    count: Option<usize>,
    default_open: bool,
    add_right: impl FnOnce(&mut egui::Ui),
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(("section", id));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    ui.horizontal(|ui| {
        let open = state.is_open();
        let mut head = ui
            .horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                icons::icon(
                    ui,
                    if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    14.0,
                    TEXT_2,
                );
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{icon}  {title}"))
                            .size(15.0)
                            .strong(),
                    )
                    .sense(Sense::click()),
                )
            })
            .response;
        head = head.interact(Sense::click());
        if head.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if let Some(n) = count {
            count_badge(ui, n, false);
        }
        if head.clicked() {
            state.toggle(ui);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add_right);
    });
    state.show_body_unindented(ui, body);
    ui.add_space(4.0);
}

/// Raised card: radius 10, hairline border, 12px padding.
#[allow(dead_code)] // shared widget for the dialogs and forms
pub fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> egui::InnerResponse<R> {
    egui::Frame::new()
        .fill(RAISED)
        .stroke(Stroke::new(1.0, hairline(0.07)))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::same(12))
        .show(ui, add)
}

/// Fully rounded button; `selected` tints it with the accent.
#[allow(dead_code)] // shared widget for the dialogs and forms
pub fn pill_button(ui: &mut egui::Ui, text: &str, selected: bool) -> Response {
    pill_icon_button(ui, None, text, selected)
}

/// [`pill_button`] with an optional leading 14px icon.
pub fn pill_icon_button(
    ui: &mut egui::Ui,
    icon: Option<Icon>,
    text: &str,
    selected: bool,
) -> Response {
    let font = TextStyle::Button.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let icon_w = if icon.is_some() { 20.0 } else { 0.0 };
    let size = Vec2::new(galley.size().x + 24.0 + icon_w, 30.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, stroke, fg) = if selected {
            (tint(ACCENT, 0.16), tint(ACCENT, 0.35), ACCENT)
        } else if resp.hovered() {
            (Color32::from_rgb(0x25, 0x27, 0x33), hairline(0.12), TEXT)
        } else {
            (Color32::TRANSPARENT, hairline(0.12), TEXT_2)
        };
        ui.painter()
            .rect_filled(rect, CornerRadius::same(255), fill);
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(255),
            Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        let mut x = rect.left() + 12.0;
        if let Some(i) = icon {
            icons::draw(
                ui.painter(),
                Rect::from_center_size(egui::pos2(x + 7.0, rect.center().y), Vec2::splat(14.0)),
                i,
                fg,
            );
            x += icon_w;
        }
        ui.painter().galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// 28px square icon button: no frame until hover, tooltip `tooltip`.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, tooltip: &str) -> Response {
    icon_button_active(ui, icon, tooltip, false)
}

/// [`icon_button`] drawn in the accent color while `active` (e.g. the terminal is open).
pub fn icon_button_active(ui: &mut egui::Ui, icon: Icon, tooltip: &str, active: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() || resp.has_focus() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(8),
                Color32::from_rgb(0x25, 0x27, 0x33),
            );
        }
        let fg = if active {
            ACCENT
        } else if resp.hovered() {
            TEXT
        } else {
            TEXT_2
        };
        icons::draw(
            ui.painter(),
            Rect::from_center_size(rect.center(), Vec2::splat(16.0)),
            icon,
            fg,
        );
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

/// Full-width menu row (icon + text) with a rounded hover highlight, for popup menus.
pub fn menu_item(ui: &mut egui::Ui, icon: Option<Icon>, text: &str) -> Response {
    let font = TextStyle::Body.resolve(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let w = ui.available_width().max(galley.size().x + 56.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter().rect_filled(
                rect,
                CornerRadius::same(7),
                Color32::from_rgb(0x2c, 0x2e, 0x3b),
            );
        }
        let fg = if resp.hovered() { TEXT } else { TEXT_2 };
        let mut x = rect.left() + 10.0;
        if let Some(i) = icon {
            icons::draw(
                ui.painter(),
                Rect::from_center_size(egui::pos2(x + 8.0, rect.center().y), Vec2::splat(16.0)),
                i,
                fg,
            );
            x += 26.0;
        }
        ui.painter().galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Pill segmented control. Returns true when the selection changed.
#[allow(dead_code)] // shared widget for the dialogs and forms
pub fn segmented(ui: &mut egui::Ui, idx: &mut usize, labels: &[&str]) -> bool {
    let mut changed = false;
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, hairline(0.07)))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::same(3))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (i, label) in labels.iter().enumerate() {
                    let on = *idx == i;
                    let font = TextStyle::Button.resolve(ui.style());
                    let galley = ui.painter().layout_no_wrap(
                        (*label).to_string(),
                        font,
                        Color32::PLACEHOLDER,
                    );
                    let (rect, resp) = ui.allocate_exact_size(
                        Vec2::new(galley.size().x + 22.0, 24.0),
                        Sense::click(),
                    );
                    if on {
                        ui.painter().rect_filled(
                            rect,
                            CornerRadius::same(255),
                            Color32::from_rgb(0x2c, 0x2e, 0x3b),
                        );
                    }
                    let fg = if on {
                        TEXT
                    } else if resp.hovered() {
                        TEXT_2
                    } else {
                        MUTED
                    };
                    ui.painter()
                        .galley(rect.center() - galley.size() / 2.0, galley, fg);
                    if resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                        && !on
                    {
                        *idx = i;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Toggle switch (36x20). Marks the response changed when flipped.
#[allow(dead_code)] // shared widget for the dialogs and forms
pub fn switch(ui: &mut egui::Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    if ui.is_rect_visible(rect) {
        let off = Color32::from_rgb(0x2c, 0x2e, 0x3b);
        let fill = off.lerp_to_gamma(ACCENT, t);
        ui.painter()
            .rect_filled(rect, CornerRadius::same(255), fill);
        let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        ui.painter()
            .circle_filled(egui::pos2(x, rect.center().y), 7.5, Color32::WHITE);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Centered dialog without a footer. See [`dialog_with_footer`].
type Footer<'a> = Box<dyn FnOnce(&mut egui::Ui) + 'a>;

#[allow(dead_code)] // shared widget for the dialogs and forms
pub fn dialog(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    width: f32,
    open: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    dialog_impl(ctx, egui::Id::unique(id), title, width, open, body, None);
}

/// Centered modal that is always fully visible: at most 90% of the screen high, the body scrolls
/// inside, and `footer` stays pinned below it. The footer lays out right-to-left, so the first
/// widget added is the rightmost (primary) button. The `x` and Escape clear `open`.
#[allow(dead_code)] // shared widget for the dialogs and forms
pub fn dialog_with_footer(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    width: f32,
    open: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui),
) {
    let footer: Footer<'_> = Box::new(footer);
    dialog_impl(
        ctx,
        egui::Id::unique(id),
        title,
        width,
        open,
        body,
        Some(footer),
    );
}

#[allow(dead_code)] // shared widget for the dialogs and forms
fn dialog_impl<'a>(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    width: f32,
    open: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
    footer: Option<Footer<'a>>,
) {
    if !*open {
        return;
    }
    let screen = ctx.content_rect();
    // Dim everything behind the dialog.
    egui::Area::new(id.with("scrim"))
        .order(egui::Order::Middle)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(110));
        });
    let w = width.min(screen.width() - 32.0).max(240.0);
    let chrome = 40.0 + 44.0 + if footer.is_some() { 64.0 } else { 0.0 };
    let body_max = (screen.height() * 0.9 - chrome).max(80.0);
    let frame = egui::Frame::window(&ctx.global_style())
        .fill(RAISED)
        .inner_margin(egui::Margin::same(20))
        .corner_radius(CornerRadius::same(12));
    egui::Window::new(title)
        .id(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .auto_sized()
        .movable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(w - 40.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(18.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if icon_button(ui, Icon::X, "").clicked() {
                        *open = false;
                    }
                });
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical()
                .id_salt(id.with("body"))
                .max_height(body_max)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add_space(2.0);
                    body(ui);
                    ui.add_space(2.0);
                });
            if let Some(footer) = footer {
                ui.add_space(10.0);
                rule(ui);
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), footer);
            }
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.egui_wants_keyboard_input() {
        *open = false;
    }
}

/// Thin full-width rule in the separator color.
pub fn rule(ui: &mut egui::Ui) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, hairline(0.07)),
    );
}

/// Ghost button: no fill until hover, muted text.
pub fn ghost(ui: &mut egui::Ui, text: &str) -> Response {
    ui.add(egui::Button::new(RichText::new(text).color(TEXT_2)).frame_when_inactive(false))
}

/// Solid colored button (e.g. blue Send, green Launch).
pub fn solid(ui: &mut egui::Ui, text: &str, fill: Color32) -> Response {
    ui.add(egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(fill))
}

/// First 8 chars of an id, as the original's `#8hexid`.
pub fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

/// `HH:MM` (UTC) for chat rows and log lines; event timestamps are unix ms.
pub fn hhmm(ms: i64) -> String {
    let secs = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}", secs / 3600, secs % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_is_green_and_members_are_stable() {
        let team = rteams_core::Team {
            name: "t".into(),
            members: vec![
                rteams_core::Member {
                    name: "boss".into(),
                    role: "lead".into(),
                    prompt: None,
                    model: None,
                    removed: false,
                    runtime: None,
                    worktree: false,
                    icon: None,
                },
                rteams_core::Member {
                    name: "ada".into(),
                    role: "dev".into(),
                    prompt: None,
                    model: None,
                    removed: false,
                    runtime: None,
                    worktree: false,
                    icon: None,
                },
            ],
            project_path: None,
            lead_prompt: None,
            deleted: false,
            column_order: Default::default(),
            schedules: vec![],
        };
        assert_eq!(member_color(Some(&team), "boss"), LEAD_COLOR);
        assert_eq!(member_color(Some(&team), "ada"), PALETTE[0]);
        assert_eq!(member_color(None, "zed"), member_color(None, "zed"));
    }

    #[test]
    fn hhmm_wraps_days() {
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm((25 * 3600 + 61) * 1000), "01:01");
    }
}

#[cfg(test)]
mod glyph_tests {
    use eframe::egui;

    /// Symbol / emoji characters used anywhere in the GUI sources, literal or `\u{..}`
    /// escaped, outside `//` comments. Letters (CJK, Cyrillic, ...) are text, not icons, and are
    /// left to font fallback.
    fn used_symbols() -> std::collections::BTreeSet<char> {
        fn walk(dir: &std::path::Path, out: &mut std::collections::BTreeSet<char>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let full = std::fs::read_to_string(&p).unwrap();
                    // Test modules hold fixtures (e.g. CLI box drawing), not UI text.
                    let src = full.split("#[cfg(test)]\nmod ").next().unwrap_or(&full);
                    for line in src.lines().filter(|l| !l.trim_start().starts_with("//")) {
                        let code = line.split(" //").next().unwrap_or(line);
                        let mut rest = code;
                        while let Some(i) = rest.find("\\u{") {
                            let tail = &rest[i + 3..];
                            if let Some(j) = tail.find('}') {
                                if let Some(c) = u32::from_str_radix(&tail[..j], 16)
                                    .ok()
                                    .and_then(char::from_u32)
                                {
                                    out.insert(c);
                                }
                            }
                            rest = &rest[i + 3..];
                        }
                        out.extend(code.chars());
                    }
                }
            }
        }
        let mut all = std::collections::BTreeSet::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut all,
        );
        all.into_iter()
            .filter(|&c| matches!(c as u32, 0x2000..=0x2BFF | 0x1F000..=0x1FAFF))
            .collect()
    }

    #[test]
    fn all_ui_glyphs_exist_in_default_fonts() {
        let ctx = egui::Context::default();
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let used = used_symbols();
        let missing: String = ctx.fonts_mut(|f| {
            used.iter()
                .copied()
                .filter(|&c| !f.has_glyph(&egui::FontId::proportional(14.0), c))
                .collect()
        });
        assert!(missing.is_empty(), "missing glyphs: {missing}");
    }
}
