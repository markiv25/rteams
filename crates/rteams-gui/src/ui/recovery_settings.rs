//! Settings panel "Automatic agent recovery": retry transient runtime errors and rate limits
//! with a reset time, initial delay and attempt cap (stored in `AppConfig.recovery`).

use eframe::egui::{self, RichText};
use rteams_services::config::AppConfig;
use rteams_services::i18n::I18n;

use crate::theme;

#[cfg(test)]
thread_local! {
    static ENGLISH: I18n = I18n::english();
}

/// English panel; see `show_tr` for a translated one. Returns true when a value changed.
#[cfg(test)]
pub fn show(ui: &mut egui::Ui, cfg: &mut AppConfig) -> bool {
    ENGLISH.with(|tr| show_tr(ui, cfg, tr))
}

/// Draw the panel with `tr`. Returns true when a value changed (the caller saves the config).
pub fn show_tr(ui: &mut egui::Ui, cfg: &mut AppConfig, tr: &I18n) -> bool {
    let r = &mut cfg.recovery;
    let before = (r.transient, r.rate_limits, r.delay_s, r.attempts);
    ui.label(RichText::new(tr.t("settings.notifications.recovery.title")).strong());
    ui.add_space(4.0);
    ui.checkbox(
        &mut r.transient,
        tr.t("settings.notifications.recovery.transient.label"),
    );
    hint(
        ui,
        &tr.t("settings.notifications.recovery.transient.description"),
    );
    ui.checkbox(
        &mut r.rate_limits,
        tr.t("settings.notifications.recovery.rateLimits.label"),
    );
    hint(
        ui,
        &tr.t("settings.notifications.recovery.rateLimits.description"),
    );
    ui.horizontal(|ui| {
        ui.label(tr.t("settings.notifications.recovery.delay.label"));
        ui.add(
            egui::DragValue::new(&mut r.delay_s)
                .range(15..=900)
                .speed(5),
        );
    });
    hint(
        ui,
        &tr.t("settings.notifications.recovery.delay.description"),
    );
    ui.horizontal(|ui| {
        ui.label(tr.t("settings.notifications.recovery.attempts.label"));
        ui.add(egui::DragValue::new(&mut r.attempts).range(1..=5));
    });
    hint(
        ui,
        &tr.t("settings.notifications.recovery.attempts.description"),
    );
    r.delay_s = r.delay_s.clamp(15, 900);
    r.attempts = r.attempts.clamp(1, 5);
    before != (r.transient, r.rate_limits, r.delay_s, r.attempts)
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).size(11.0).color(theme::MUTED));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::testing::render;

    #[test]
    fn renders_unchanged_without_interaction() {
        let mut cfg = AppConfig::default();
        let copy = cfg.clone();
        render(|ui| {
            assert!(!show(ui, &mut cfg));
        });
        assert_eq!(cfg, copy);
    }

    #[test]
    fn out_of_range_values_are_clamped_and_reported() {
        let mut cfg = AppConfig::default();
        cfg.recovery.delay_s = 5000;
        cfg.recovery.attempts = 0;
        let mut changed = false;
        render(|ui| changed |= show(ui, &mut cfg));
        assert!(changed);
        assert_eq!((cfg.recovery.delay_s, cfg.recovery.attempts), (900, 1));
    }
}
