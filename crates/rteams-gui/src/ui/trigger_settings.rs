//! Settings panel "Notification Triggers": built-in and custom rules over session data, with
//! per-rule enable, regex patterns (validated), ignore patterns, repository scope, color and a
//! preview against a loaded session.

use eframe::egui::{self, Color32, RichText};
use rteams_services::i18n::I18n;
use rteams_services::sessions::chunks::Session;
use rteams_services::sessions::triggers::{self, TriggerKind, TriggerRule};

use crate::theme;

fn tt(tr: &I18n, key: &str, default: &str) -> String {
    if tr.has(key) {
        tr.t(key)
    } else {
        default.to_string()
    }
}

/// `#rrggbb` to a color.
pub fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

fn kind_label(tr: &I18n, k: TriggerKind) -> String {
    let key = match k {
        TriggerKind::SessionEnded => "sessions.trigger.sessionEnded",
        TriggerKind::ToolError => "sessions.trigger.toolError",
        TriggerKind::CostOver => "sessions.trigger.costOver",
        TriggerKind::TokensOver => "sessions.trigger.tokensOver",
        TriggerKind::Keyword => "sessions.trigger.keyword",
    };
    tt(tr, key, k.label())
}

fn nt(tr: &I18n, k: &str, d: &str) -> String {
    tt(tr, &format!("settings.notificationTriggers.{k}"), d)
}

/// Draw the panel. Returns true when any rule changed (the caller saves with
/// `triggers::save_rules`).
#[cfg(test)]
pub fn show(ui: &mut egui::Ui, rules: &mut Vec<TriggerRule>, tr: &I18n) -> bool {
    show_with_session(ui, rules, tr, None)
}

/// `show` with a session to preview rules against.
pub fn show_with_session(
    ui: &mut egui::Ui,
    rules: &mut Vec<TriggerRule>,
    tr: &I18n,
    sample: Option<&Session>,
) -> bool {
    let mut changed = false;
    let mut delete: Option<usize> = None;

    ui.label(
        RichText::new(nt(tr, "builtin.title", "Built-in Triggers"))
            .strong()
            .size(15.0),
    );
    ui.label(
        RichText::new(nt(
            tr,
            "builtin.description",
            "Default triggers that come with the application. You can enable or disable them and customize their patterns.",
        ))
        .small()
        .color(theme::MUTED),
    );
    ui.add_space(6.0);
    for (i, r) in rules.iter_mut().enumerate().filter(|(_, r)| r.builtin) {
        changed |= rule_card(ui, tr, r, sample, &mut false);
        let _ = i;
    }

    ui.add_space(12.0);
    ui.label(
        RichText::new(nt(tr, "custom.title", "Custom Triggers"))
            .strong()
            .size(15.0),
    );
    ui.label(
        RichText::new(nt(
            tr,
            "custom.description",
            "Create your own triggers to get notified for specific patterns or tool outputs.",
        ))
        .small()
        .color(theme::MUTED),
    );
    ui.add_space(6.0);
    if !rules.iter().any(|r| !r.builtin) {
        ui.label(
            RichText::new(nt(tr, "custom.empty", "No custom triggers configured yet."))
                .color(theme::MUTED),
        );
    }
    for (i, r) in rules.iter_mut().enumerate().filter(|(_, r)| !r.builtin) {
        let mut del = false;
        changed |= rule_card(ui, tr, r, sample, &mut del);
        if del {
            delete = Some(i);
        }
    }
    if let Some(i) = delete {
        rules.remove(i);
        changed = true;
    }

    ui.add_space(12.0);
    ui.label(
        RichText::new(nt(tr, "add.title", "Add Custom Trigger"))
            .strong()
            .size(15.0),
    );
    let id = egui::Id::unique("trigger_settings_add");
    let (mut name, mut kind): (String, TriggerKind) = ui
        .data_mut(|d| d.get_temp(id))
        .unwrap_or((String::new(), TriggerKind::Keyword));
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut name)
                .hint_text(nt(
                    tr,
                    "fields.triggerNamePlaceholder",
                    "e.g., Build Failure Alert",
                ))
                .desired_width(240.0),
        );
        egui::ComboBox::from_id_salt("trigger_add_kind")
            .selected_text(kind_label(tr, kind))
            .show_ui(ui, |ui| {
                for k in TriggerKind::ALL {
                    ui.selectable_value(&mut kind, k, kind_label(tr, k));
                }
            });
        let ok = !name.trim().is_empty();
        if ui
            .add_enabled(ok, egui::Button::new(nt(tr, "add.submit", "Add Trigger")))
            .clicked()
        {
            let mut r = TriggerRule::custom(name.trim(), kind);
            if kind == TriggerKind::Keyword {
                r.pattern = "error|failed|exception".into();
            }
            rules.push(r);
            name.clear();
            changed = true;
        }
    });
    ui.data_mut(|d| d.insert_temp(id, (name, kind)));
    changed
}

fn rule_card(
    ui: &mut egui::Ui,
    tr: &I18n,
    r: &mut TriggerRule,
    sample: Option<&Session>,
    delete: &mut bool,
) -> bool {
    let mut changed = false;
    let id = egui::Id::unique(("trigger_rule", &r.id));
    let mut open: bool = ui.data_mut(|d| d.get_temp(id)).unwrap_or(false);
    egui::Frame::new()
        .fill(theme::CARD)
        .stroke(egui::Stroke::new(1.0, theme::hairline(0.07)))
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                changed |= ui.checkbox(&mut r.enabled, "").changed();
                let c = parse_hex(&r.color).unwrap_or(theme::MUTED);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, c);
                ui.label(RichText::new(&r.name).strong());
                if r.builtin {
                    theme::pill(ui, nt(tr, "card.builtinBadge", "Builtin"), theme::INFO);
                }
                ui.label(
                    RichText::new(kind_label(tr, r.kind))
                        .small()
                        .color(theme::MUTED),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (txt, key) = if open {
                        ("\u{23F7}", "card.collapseAriaLabel")
                    } else {
                        ("\u{23F5}", "card.expandAriaLabel")
                    };
                    if ui
                        .add(egui::Button::new(txt).frame(false))
                        .on_hover_text(nt(tr, key, "Toggle"))
                        .clicked()
                    {
                        open = !open;
                    }
                    if !r.builtin
                        && ui
                            .add(egui::Button::new("\u{1F5D1}").frame(false))
                            .on_hover_text(nt(tr, "card.deleteAriaLabel", "Delete trigger"))
                            .clicked()
                    {
                        *delete = true;
                    }
                });
            });
            if open {
                ui.add_space(6.0);
                changed |= config(ui, tr, r, id);
                if let Some(s) = sample {
                    ui.add_space(4.0);
                    let hits = triggers::test_rule(r, s, now_ms());
                    ui.label(
                        RichText::new(format!(
                            "{}: {}",
                            nt(tr, "preview.title", "Preview"),
                            hits.len()
                        ))
                        .small()
                        .color(theme::TEXT_2),
                    );
                    for h in hits.iter().take(3) {
                        ui.label(RichText::new(&h.title).small().color(theme::MUTED));
                    }
                }
            }
        });
    ui.data_mut(|d| d.insert_temp(id, open));
    ui.add_space(6.0);
    changed
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn regex_field(ui: &mut egui::Ui, tr: &I18n, label: &str, value: &mut String, hint: &str) -> bool {
    ui.label(RichText::new(label).small().color(theme::MUTED));
    let bad = triggers::validate_regex(value).is_err();
    let r = ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(f32::INFINITY)
            .text_color_opt(bad.then_some(theme::ERROR)),
    );
    if bad {
        ui.label(
            RichText::new(nt(
                tr,
                "errors.invalidRegexPattern",
                "Invalid regex pattern",
            ))
            .small()
            .color(theme::ERROR),
        );
    }
    r.changed()
}

fn config(ui: &mut egui::Ui, tr: &I18n, r: &mut TriggerRule, id: egui::Id) -> bool {
    let mut changed = false;
    if r.name.is_empty() || !r.builtin {
        ui.label(
            RichText::new(nt(tr, "fields.triggerNameRequired", "Trigger Name *"))
                .small()
                .color(theme::MUTED),
        );
        changed |= ui
            .add(egui::TextEdit::singleline(&mut r.name).desired_width(f32::INFINITY))
            .changed();
    }
    match r.kind {
        TriggerKind::ToolError => {
            ui.label(
                RichText::new(nt(
                    tr,
                    "configuration.errorStatusDescription",
                    "Triggers when a tool execution reports an error (is_error: true).",
                ))
                .small()
                .color(theme::TEXT_2),
            );
            ui.label(
                RichText::new(nt(
                    tr,
                    "fields.scopeToolNameOptional",
                    "Scope / Tool Name (optional)",
                ))
                .small()
                .color(theme::MUTED),
            );
            changed |= ui
                .add(egui::TextEdit::singleline(&mut r.tool_name).desired_width(f32::INFINITY))
                .changed();
            changed |= regex_field(
                ui,
                tr,
                &nt(tr, "fields.matchPattern", "Match Pattern (Regex)"),
                &mut r.pattern,
                &nt(
                    tr,
                    "configuration.matchPatternPlaceholder",
                    "e.g., error|failed|exception",
                ),
            );
            ui.label(
                RichText::new(nt(
                    tr,
                    "configuration.emptyPatternHint",
                    "Leave empty to match all content.",
                ))
                .small()
                .color(theme::MUTED),
            );
        }
        TriggerKind::Keyword => {
            changed |= regex_field(
                ui,
                tr,
                &nt(tr, "fields.matchPattern", "Match Pattern (Regex)"),
                &mut r.pattern,
                &nt(
                    tr,
                    "configuration.matchPatternPlaceholder",
                    "e.g., error|failed|exception",
                ),
            );
            if r.pattern.trim().is_empty() {
                ui.label(
                    RichText::new("A pattern is required")
                        .small()
                        .color(theme::ERROR),
                );
            }
        }
        TriggerKind::CostOver => {
            ui.horizontal(|ui| {
                ui.label(tt(
                    tr,
                    "sessions.trigger.alertCost",
                    "Alert if cost over (USD)",
                ));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut r.threshold)
                            .range(0.0..=100_000.0)
                            .speed(0.25)
                            .prefix("$"),
                    )
                    .changed();
            });
        }
        TriggerKind::TokensOver => {
            ui.horizontal(|ui| {
                ui.label(nt(tr, "configuration.alertIfGreaterThan", "Alert if >"));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut r.threshold)
                            .range(0.0..=1e12)
                            .speed(1000.0),
                    )
                    .changed();
                ui.label(nt(tr, "configuration.tokensUnit", "tokens"));
            });
        }
        TriggerKind::SessionEnded => {
            ui.horizontal(|ui| {
                ui.label(tt(tr, "sessions.trigger.idleMinutes", "Idle for (minutes)"));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut r.threshold)
                            .range(0.5..=1440.0)
                            .speed(0.5),
                    )
                    .changed();
            });
        }
    }

    // ignore patterns
    ui.add_space(4.0);
    ui.label(
        RichText::new(nt(
            tr,
            "ignorePatterns.title",
            "Ignore Patterns (skip if matches)",
        ))
        .small()
        .color(theme::MUTED),
    );
    let mut remove = None;
    for (i, p) in r.ignore_patterns.iter().enumerate() {
        ui.horizontal(|ui| {
            let bad = triggers::validate_regex(p).is_err();
            ui.label(RichText::new(p).monospace().small().color(if bad {
                theme::ERROR
            } else {
                theme::TEXT_2
            }));
            if ui
                .add(egui::Button::new("\u{1F5D9}").frame(false))
                .clicked()
            {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        r.ignore_patterns.remove(i);
        changed = true;
    }
    let add_id = id.with("ignore_add");
    let mut new_p: String = ui.data_mut(|d| d.get_temp(add_id)).unwrap_or_default();
    let resp = ui.add(
        egui::TextEdit::singleline(&mut new_p)
            .hint_text(nt(tr, "ignorePatterns.placeholder", "Add ignore regex..."))
            .desired_width(f32::INFINITY),
    );
    if resp.lost_focus()
        && ui.input(|i| i.key_pressed(egui::Key::Enter))
        && !new_p.trim().is_empty()
        && triggers::validate_regex(&new_p).is_ok()
    {
        r.ignore_patterns.push(new_p.trim().to_string());
        new_p.clear();
        changed = true;
    }
    ui.data_mut(|d| d.insert_temp(add_id, new_p));

    // repository scope + color
    ui.add_space(4.0);
    ui.label(
        RichText::new(tt(
            tr,
            "sessions.trigger.repos",
            "Repositories (comma separated, empty = all)",
        ))
        .small()
        .color(theme::MUTED),
    );
    let repo_id = id.with("repos");
    let mut repos: String = ui
        .data_mut(|d| d.get_temp(repo_id))
        .unwrap_or_else(|| r.repos.join(", "));
    if ui
        .add(egui::TextEdit::singleline(&mut repos).desired_width(f32::INFINITY))
        .changed()
    {
        r.repos = repos
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        changed = true;
    }
    ui.data_mut(|d| d.insert_temp(repo_id, repos));
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(nt(tr, "color.customHexTitle", "Custom hex color"))
                .small()
                .color(theme::MUTED),
        );
        changed |= ui
            .add(egui::TextEdit::singleline(&mut r.color).desired_width(80.0))
            .changed();
        if parse_hex(&r.color).is_none() {
            ui.label(
                RichText::new(nt(tr, "color.invalidHex", "Invalid hex"))
                    .small()
                    .color(theme::ERROR),
            );
        }
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_services::sessions::chunks::{parse_str, Session};
    use rteams_services::sessions::paths::Source;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#ff0000"), Some(Color32::from_rgb(255, 0, 0)));
        assert_eq!(parse_hex("ff0000"), None);
        assert_eq!(parse_hex("#ff00"), None);
    }

    #[test]
    fn renders_and_adds_rules() {
        let tr = I18n::default();
        let mut rules = triggers::default_rules();
        rules.push(TriggerRule::custom("mine", TriggerKind::Keyword));
        let raw = r#"{"type":"user","message":{"role":"user","content":"hi"}}"#;
        let s = Session::from_msgs(
            std::path::Path::new("/x/s.jsonl"),
            Source::Claude,
            parse_str(raw, Source::Claude),
        );
        // open every card by seeding its temp state
        crate::view::testing::render(|ui| {
            for r in &rules {
                let id = egui::Id::unique(("trigger_rule", &r.id));
                ui.data_mut(|d| d.insert_temp(id, true));
            }
            let _ = show_with_session(ui, &mut rules, &tr, Some(&s));
        });
        let n = rules.len();
        crate::view::testing::render(|ui| {
            let _ = show(ui, &mut rules, &tr);
        });
        assert_eq!(rules.len(), n);
    }
}
