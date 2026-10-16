//! Legend in the bottom-left corner of the canvas.

use eframe::egui::{pos2, vec2, Color32, Painter, Rect, Stroke, StrokeKind};
use rteams_services::i18n::I18n;

use crate::theme;
use crate::ui::graph::kit::{draw_dot, draw_text_fit};

pub fn draw(p: &Painter, viewport: Rect, tr: &I18n, show_relations: bool) {
    let mut rows: Vec<(Color32, String)> = vec![
        (
            theme::ACCENT,
            tr.t("team.organizations.graph.legend.organization"),
        ),
        (
            theme::COL_REVIEW,
            tr.t("team.organizations.graph.legend.group"),
        ),
        (
            theme::TEXT_2,
            tr.t("team.organizations.graph.legend.hierarchy"),
        ),
        (
            theme::SUCCESS,
            tr.t("team.organizations.graph.legend.online"),
        ),
    ];
    if show_relations {
        rows.insert(
            3,
            (
                theme::WARNING,
                tr.t("team.organizations.graph.legend.connection"),
            ),
        );
    }
    let h = 12.0 + rows.len() as f32 * 17.0;
    let w = 130.0_f32.min(viewport.width() - 24.0);
    if w < 80.0 || viewport.height() < h + 140.0 {
        return;
    }
    let r = Rect::from_min_size(
        pos2(viewport.left() + 14.0, viewport.bottom() - 14.0 - h),
        vec2(w, h),
    );
    p.rect_filled(r, 14.0, theme::tint(theme::SIDEBAR, 0.92));
    p.rect_stroke(
        r,
        14.0,
        Stroke::new(1.0, theme::hairline(0.1)),
        StrokeKind::Inside,
    );
    for (i, (c, s)) in rows.iter().enumerate() {
        let y = r.top() + 14.0 + i as f32 * 17.0;
        draw_dot(p, pos2(r.left() + 14.0, y), 3.5, *c);
        draw_text_fit(
            p,
            pos2(r.left() + 26.0, y),
            s,
            11.0,
            theme::TEXT_2,
            w - 34.0,
        );
    }
}
