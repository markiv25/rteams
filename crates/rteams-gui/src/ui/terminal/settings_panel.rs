//! Terminal settings tab (theme, font, background, behavior, runtime controls).

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;
use rteams_services::terminal::TerminalSettings;

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RuntimeAction {
    #[default]
    None,
    /// Respawn the active tab's shell.
    Reconnect,
    /// Respawn every tab's shell.
    Reload,
    /// Kill the active tab's shell.
    Stop,
}

fn key(k: &str) -> String {
    format!("team.terminalWorkspace.{k}")
}

fn section(ui: &mut egui::Ui, tr: &I18n, title: &str, desc: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(8.0);
    ui.label(RichText::new(tr.t(&key(title))).strong());
    ui.label(RichText::new(tr.t(&key(desc))).small().color(theme::MUTED));
    ui.add_space(4.0);
    add(ui);
}

/// Returns (settings changed, runtime button pressed).
pub fn show(ui: &mut egui::Ui, tr: &I18n, s: &mut TerminalSettings) -> (bool, RuntimeAction) {
    let mut changed = false;
    let mut action = RuntimeAction::None;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.label(
                RichText::new(tr.t(&key("settingsTitle")))
                    .size(16.0)
                    .strong(),
            );
            ui.label(RichText::new(tr.t(&key("settingsDescription"))).color(theme::MUTED));

            section(
                ui,
                tr,
                "settingsThemeTitle",
                "settingsThemeDescription",
                |ui| {
                    ui.horizontal(|ui| {
                        for (v, k) in [("dark", "themeDark"), ("light", "themeLight")] {
                            changed |= ui
                                .selectable_value(&mut s.theme, v.to_string(), tr.t(&key(k)))
                                .changed();
                        }
                    });
                },
            );

            section(
                ui,
                tr,
                "settingsFontTitle",
                "settingsFontDescription",
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr.t(&key("settingsFontPreset")));
                        for (v, k) in [
                            ("compact", "fontScaleCompact"),
                            ("default", "fontScaleDefault"),
                            ("large", "fontScaleLarge"),
                        ] {
                            if ui
                                .selectable_label(s.font_preset == v, tr.t(&key(k)))
                                .clicked()
                            {
                                s.font_preset = v.to_string();
                                s.font_size = TerminalSettings::preset_size(v);
                                changed = true;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr.t(&key("settingsFontSize")));
                        changed |= ui
                            .add(egui::Slider::new(&mut s.font_size, 8.0..=32.0).step_by(1.0))
                            .changed();
                    });
                },
            );

            section(
                ui,
                tr,
                "settingsBackgroundTitle",
                "settingsBackgroundDescription",
                |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr.t(&key("settingsBackgroundMode")));
                        for (v, k) in [
                            ("transparent", "backgroundModeTransparent"),
                            ("solid", "backgroundModeSolid"),
                            ("image", "backgroundModeImage"),
                        ] {
                            changed |= ui
                                .selectable_value(&mut s.bg_mode, v.to_string(), tr.t(&key(k)))
                                .changed();
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr.t(&key("settingsOpacity")));
                        changed |= ui
                            .add(egui::Slider::new(&mut s.opacity, 0.1..=1.0))
                            .changed();
                    });
                    if s.bg_mode == "image" {
                        ui.horizontal(|ui| {
                            ui.label(tr.t(&key("settingsImageUrl")));
                            changed |= ui.text_edit_singleline(&mut s.image_path).changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label(tr.t(&key("settingsImageFit")));
                            for (v, k) in [
                                ("cover", "imageFitCover"),
                                ("contain", "imageFitContain"),
                                ("stretch", "imageFitStretch"),
                                ("tile", "imageFitTile"),
                                ("center", "imageFitCenter"),
                            ] {
                                changed |= ui
                                    .selectable_value(
                                        &mut s.image_fit,
                                        v.to_string(),
                                        tr.t(&key(k)),
                                    )
                                    .changed();
                            }
                        });
                    }
                },
            );

            section(
                ui,
                tr,
                "settingsBehaviorTitle",
                "settingsBehaviorDescription",
                |ui| {
                    changed |= ui
                        .checkbox(&mut s.wrap, tr.t(&key("settingsWrapLongOutput")))
                        .changed();
                },
            );

            section(
                ui,
                tr,
                "settingsRuntimeTitle",
                "settingsRuntimeDescription",
                |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(tr.t(&key("settingsReconnect"))).clicked() {
                            action = RuntimeAction::Reconnect;
                        }
                        if ui.button(tr.t(&key("settingsReload"))).clicked() {
                            action = RuntimeAction::Reload;
                        }
                        if ui.button(tr.t(&key("settingsStop"))).clicked() {
                            action = RuntimeAction::Stop;
                        }
                    });
                },
            );

            ui.add_space(12.0);
            if ui.button(tr.t(&key("settingsResetAppearance"))).clicked() {
                s.reset_appearance();
                changed = true;
            }
        });
    if changed {
        s.clamp();
    }
    (changed, action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_headless() {
        let tr = I18n::default();
        let mut s = TerminalSettings {
            bg_mode: "image".into(),
            ..Default::default()
        };
        crate::view::testing::render(|ui| {
            show(ui, &tr, &mut s);
        });
    }
}
