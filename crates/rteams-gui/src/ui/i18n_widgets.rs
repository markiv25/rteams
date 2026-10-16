//! Language widgets: the app-language picker and a translated-label helper.

use eframe::egui;
use rteams_services::i18n::{self, I18n};

/// Combo box of every shipped locale (native names; complex-script ones tagged "(experimental)").
/// Switches `tr` in place and returns true when the user picked a different locale.
pub fn locale_picker(ui: &mut egui::Ui, tr: &mut I18n) -> bool {
    let label = |code: &str, name: &str| {
        if i18n::experimental(code) {
            format!("{name} (experimental)")
        } else {
            name.to_string()
        }
    };
    let current = i18n::available_locales()
        .into_iter()
        .find(|(c, _)| *c == tr.locale)
        .map(|(c, n)| label(c, n))
        .unwrap_or_else(|| tr.locale.clone());
    let mut picked: Option<&'static str> = None;
    egui::ComboBox::from_id_salt("rteams-locale-picker")
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (code, name) in i18n::available_locales() {
                if ui
                    .selectable_label(tr.locale == code, label(code, name))
                    .clicked()
                {
                    picked = Some(code);
                }
            }
        });
    match picked {
        Some(code) if code != tr.locale => {
            tr.set_locale(code);
            true
        }
        _ => false,
    }
}

/// Inline list of every shipped locale (for menus). Returns true when a different locale was
/// picked; `tr` is switched in place.
pub fn locale_list(ui: &mut egui::Ui, tr: &mut I18n) -> bool {
    let mut picked: Option<&'static str> = None;
    egui::ScrollArea::vertical()
        .id_salt("rteams-locale-list")
        .max_height(240.0)
        .show(ui, |ui| {
            for (code, name) in i18n::available_locales() {
                let mut text = name.to_string();
                if i18n::experimental(code) {
                    text.push_str(" (experimental)");
                }
                let on = tr.locale == code;
                let resp =
                    crate::theme::menu_item(ui, on.then_some(crate::icons::Icon::Check), &text);
                if resp.clicked() {
                    picked = Some(code);
                }
            }
        });
    match picked {
        Some(code) if code != tr.locale => {
            tr.set_locale(code);
            true
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;

    #[test]
    fn i18n_picker_renders_headless() {
        let mut tr = I18n::load("ar");
        render(|ui| {
            let mut local = tr.clone();
            assert!(!locale_picker(ui, &mut local));
        });
        tr.set_locale("de");
        assert_eq!(tr.locale, "de");
    }
}
