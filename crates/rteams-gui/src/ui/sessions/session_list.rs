//! Middle pane: sessions of the selected project, pinned first.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::metadata_index::SessionMeta;
use rteams_services::sessions::paths::Source;

use super::{fmt_ago, fmt_cost, fmt_dur, fmt_tokens, tt};
use crate::theme;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListAction {
    Select(String),
    TogglePin(String),
    ToggleHide(String),
    /// Hide (true) or unhide (false) every session in the list.
    SetHiddenAll(bool),
}

const ROW_H: f32 = 62.0;

fn source_color(s: Source) -> egui::Color32 {
    match s {
        Source::Claude => theme::ACCENT,
        Source::Codex => theme::SUCCESS,
        Source::Rteams => theme::WARNING,
    }
}

/// Draw the list. `items` is already ordered (see `scanner::order_for_display`).
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut egui::Ui,
    tr: &I18n,
    items: &[&SessionMeta],
    selected: Option<&str>,
    pinned: &[String],
    hidden: &[String],
    show_hidden: &mut bool,
    now_ms: i64,
) -> Vec<ListAction> {
    let mut out = Vec::new();
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(tr.t_count("common.sessions.count", items.len() as u64))
                .small()
                .color(theme::MUTED),
        );
        let key = if *show_hidden {
            "common.sessions.visibility.hideHidden"
        } else {
            "common.sessions.visibility.showHidden"
        };
        let default = if *show_hidden {
            "Hide hidden sessions"
        } else {
            "Show hidden sessions"
        };
        if theme::ghost(ui, &tt(tr, key, default)).clicked() {
            *show_hidden = !*show_hidden;
        }
        if !items.is_empty() {
            let all_hidden = items.iter().all(|s| hidden.contains(&s.id));
            let (key, d) = if all_hidden {
                ("common.sessions.actions.unhideAll", "Unhide all")
            } else {
                ("common.sessions.actions.hideAll", "Hide all")
            };
            if theme::ghost(ui, &tt(tr, key, d)).clicked() {
                out.push(ListAction::SetHiddenAll(!all_hidden));
            }
        }
    });
    if items.is_empty() {
        ui.add_space(8.0);
        ui.label(
            RichText::new(tt(
                tr,
                "common.sessions.empty.noSessions",
                "No sessions found",
            ))
            .color(theme::MUTED),
        );
        return out;
    }
    egui::ScrollArea::vertical()
        .id_salt("sessions_list")
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, items.len(), |ui, range| {
            for i in range {
                let s = items[i];
                let on = selected == Some(s.id.as_str());
                let is_pinned = pinned.contains(&s.id);
                let is_hidden = hidden.contains(&s.id);
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), ROW_H - 4.0),
                    egui::Sense::click(),
                );
                let fill = if on {
                    theme::tint(theme::ACCENT, 0.16)
                } else if resp.hovered() {
                    theme::RAISED
                } else {
                    theme::CARD
                };
                ui.painter().rect_filled(rect, 6.0, fill);
                let mut inner = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(egui::vec2(8.0, 4.0)))
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                inner.horizontal(|ui| {
                    if is_pinned {
                        ui.label(RichText::new("\u{1F4CC}").small());
                    }
                    let title = if s.first_prompt.trim().is_empty() {
                        s.id.clone()
                    } else {
                        s.first_prompt.replace('\n', " ")
                    };
                    let mut t = RichText::new(clip(&title, 70)).strong();
                    if is_hidden {
                        t = t.color(theme::MUTED);
                    }
                    ui.add(egui::Label::new(t).truncate());
                });
                inner.horizontal(|ui| {
                    theme::pill(ui, s.source.as_str(), source_color(s.source));
                    let when = if s.end_ms > 0 {
                        fmt_ago(now_ms, s.end_ms)
                    } else {
                        fmt_ago(now_ms, s.mtime_ms)
                    };
                    let mut bits = vec![when];
                    if s.start_ms > 0 && s.duration_ms() > 0 {
                        bits.push(fmt_dur(s.duration_ms()));
                    }
                    bits.push(format!("{} tools", s.tool_count));
                    bits.push(fmt_tokens(s.total_tokens()));
                    if s.cost_usd > 0.0 {
                        bits.push(fmt_cost(s.cost_usd));
                    }
                    ui.label(
                        RichText::new(bits.join(" \u{00B7} "))
                            .small()
                            .color(theme::MUTED),
                    );
                });
                inner.horizontal(|ui| {
                    let mut m = String::new();
                    if !s.model.is_empty() {
                        m.push_str(&s.model);
                    }
                    if !s.git_branch.is_empty() {
                        if !m.is_empty() {
                            m.push_str(" \u{00B7} ");
                        }
                        m.push_str(&s.git_branch);
                    }
                    ui.label(RichText::new(m).small().color(theme::TEXT_2));
                });
                if resp.clicked() {
                    out.push(ListAction::Select(s.id.clone()));
                }
                resp.context_menu(|ui| {
                    let pin = if is_pinned {
                        tt(tr, "common.sessions.actions.unpin", "Unpin")
                    } else {
                        tt(tr, "common.sessions.actions.pin", "Pin")
                    };
                    if ui.button(pin).clicked() {
                        out.push(ListAction::TogglePin(s.id.clone()));
                        ui.close();
                    }
                    let hide = if is_hidden {
                        tt(tr, "common.sessions.actions.unhide", "Unhide")
                    } else {
                        tt(tr, "common.sessions.actions.hide", "Hide")
                    };
                    if ui.button(hide).clicked() {
                        out.push(ListAction::ToggleHide(s.id.clone()));
                        ui.close();
                    }
                });
                ui.add_space(4.0);
            }
        });
    out
}

fn clip(s: &str, n: usize) -> String {
    let mut o: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        o.push('\u{2026}');
    }
    o
}
