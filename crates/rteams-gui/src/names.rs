//! Default member names (pop-culture parodies) and member icons. A new member's name field is
//! prefilled with the next unused parody name; the user can type any other name. Each member
//! shows an icon in its avatar: the one picked in the member dialog, else the parody's own icon,
//! else one chosen from the name so it stays the same across runs.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;

/// Parody names with their icon. Names stick to `[a-z0-9-]` so they work in `@mentions`.
pub const DEFAULTS: &[(&str, &str)] = &[
    ("chad-gpt", "👾"),
    ("sherlock-codes", "🔍"),
    ("darth-coder", "⚔"),
    ("yoda-lint", "🐸"),
    ("gandalf-the-grep", "🔮"),
    ("bob-the-bugfixer", "🔨"),
    ("captain-commit", "⚓"),
    ("tony-stack", "⚙"),
    ("bruce-debugger", "🌃"),
    ("arnold-shellzenegger", "💪"),
    ("elon-tusk", "🐘"),
    ("jeff-bezbot", "📦"),
    ("mark-zuckerbot", "👓"),
    ("hal-9001", "🔴"),
    ("marie-curious", "🔬"),
    ("alan-turingtest", "💡"),
    ("ada-lovebytes", "💾"),
    ("clippy-jr", "📎"),
];

/// Icons offered in the member dialog's picker: every parody icon plus a few extras.
pub const PICKER: &[&str] = &[
    "👾", "🔍", "⚔", "🐸", "🔮", "🔨", "⚓", "⚙", "🌃", "💪", "🐘", "📦", "👓", "🔴", "🔬", "💡",
    "💾", "📎", "🚀", "⚡", "🔥", "🌙", "⭐", "🐙", "🐺", "🐼", "🐧", "🐝", "🎯", "🎨", "🔧", "📚",
    "🎲", "📡", "🎩", "👑", "👻", "😎", "👽", "🐱", "🐶", "🐉", "☕", "🍕", "🎸",
];

/// The next parody name not in `taken`; after all of them are used, `<name>-2`, `-3`, ...
pub fn suggest<'a>(taken: impl IntoIterator<Item = &'a str>) -> String {
    let taken: Vec<String> = taken.into_iter().map(str::to_lowercase).collect();
    let free = |n: &str| !taken.iter().any(|t| t == n);
    for round in 1.. {
        for (name, _) in DEFAULTS {
            let candidate = if round == 1 {
                name.to_string()
            } else {
                format!("{name}-{round}")
            };
            if free(&candidate) {
                return candidate;
            }
        }
    }
    unreachable!()
}

/// Icon for a member with no picked icon: the parody's own (also for `chad-gpt-2`), else a
/// stable pick from `PICKER` based on the name.
pub fn default_icon(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if let Some((_, icon)) = DEFAULTS.iter().find(|(n, _)| {
        lower == *n
            || lower
                .strip_prefix(n)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
    }) {
        return icon;
    }
    // FNV-1a: stable across runs and platforms, unlike the std hasher.
    let mut h: u32 = 0x811c_9dc5;
    for b in lower.bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    PICKER[h as usize % PICKER.len()]
}

fn key() -> egui::Id {
    egui::Id::unique("rteams-member-icons")
}

/// Publish the team's picked icons (member name -> icon) for `icon_for` to read this frame.
pub fn publish(ctx: &egui::Context, icons: HashMap<String, String>) {
    ctx.data_mut(|d| d.insert_temp(key(), Arc::new(icons)));
}

/// Icon to draw for `name`: the picked one when published, else `default_icon`.
pub fn icon_for(ctx: &egui::Context, name: &str) -> String {
    let picked = ctx.data(|d| d.get_temp::<Arc<HashMap<String, String>>>(key()));
    picked
        .and_then(|m| m.get(name).filter(|s| !s.trim().is_empty()).cloned())
        .unwrap_or_else(|| default_icon(name).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggests_first_unused_then_numbers() {
        assert_eq!(suggest([]), "chad-gpt");
        assert_eq!(suggest(["chad-gpt", "Sherlock-Codes"]), "darth-coder");
        let all: Vec<&str> = DEFAULTS.iter().map(|(n, _)| *n).collect();
        assert_eq!(suggest(all), "chad-gpt-2");
    }

    #[test]
    fn names_are_mention_safe_and_unique() {
        for (i, (n, icon)) in DEFAULTS.iter().enumerate() {
            assert!(n
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
            assert!(PICKER.contains(icon), "{n}'s icon missing from picker");
            assert!(DEFAULTS[..i].iter().all(|(m, _)| m != n), "duplicate {n}");
        }
    }

    #[test]
    fn default_icons_are_stable() {
        assert_eq!(default_icon("yoda-lint"), "🐸");
        assert_eq!(default_icon("chad-gpt-3"), "👾");
        assert_eq!(default_icon("ada"), default_icon("ADA"));
        assert!(PICKER.contains(&default_icon("someone-else")));
    }

    #[test]
    fn every_icon_has_a_glyph() {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
        let font = egui::FontId::proportional(14.0);
        let missing: Vec<&str> = PICKER
            .iter()
            .copied()
            .filter(|i| !ctx.fonts_mut(|f| f.has_glyphs(&font, i)))
            .collect();
        assert!(missing.is_empty(), "no glyph for {missing:?}");
        ctx.run_ui(Default::default(), |_| {})
            .textures_delta
            .clear();
    }

    #[test]
    fn picked_icon_wins_over_default() {
        let ctx = egui::Context::default();
        assert_eq!(icon_for(&ctx, "yoda-lint"), "🐸");
        publish(&ctx, HashMap::from([("yoda-lint".into(), "🚀".into())]));
        assert_eq!(icon_for(&ctx, "yoda-lint"), "🚀");
        assert_eq!(icon_for(&ctx, "chad-gpt"), "👾");
    }
}
