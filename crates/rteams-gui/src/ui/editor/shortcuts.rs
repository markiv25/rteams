//! Keyboard shortcuts: polling while the editor is visible, and the help dialog.

use eframe::egui;
use rteams_services::i18n::I18n;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    QuickOpen,
    Save,
    SaveAll,
    CloseTab,
    SearchInFiles,
    NextTab,
    PrevTab,
    ToggleSidebar,
    Refresh,
    SplitPreview,
    FullPreview,
}

/// Consume editor shortcuts. Call only while the editor screen is shown.
pub fn poll(ctx: &egui::Context) -> Vec<Cmd> {
    use egui::{Key, Modifiers as M};
    let cmd = M::COMMAND;
    let table: [(M, Key, Cmd); 13] = [
        (cmd | M::SHIFT, Key::F, Cmd::SearchInFiles),
        (cmd | M::ALT, Key::S, Cmd::SaveAll),
        (cmd | M::SHIFT, Key::V, Cmd::SplitPreview),
        (cmd | M::ALT, Key::V, Cmd::FullPreview),
        (cmd, Key::P, Cmd::QuickOpen),
        (cmd, Key::S, Cmd::Save),
        (cmd, Key::W, Cmd::CloseTab),
        (cmd, Key::B, Cmd::ToggleSidebar),
        (M::CTRL | M::SHIFT, Key::Tab, Cmd::PrevTab),
        (M::CTRL, Key::Tab, Cmd::NextTab),
        (M::NONE, Key::F5, Cmd::Refresh),
        (cmd | M::ALT, Key::ArrowRight, Cmd::NextTab),
        (cmd | M::ALT, Key::ArrowLeft, Cmd::PrevTab),
    ];
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        for (m, k, c) in table {
            if i.consume_key(m, k) {
                out.push(c);
            }
        }
    });
    out
}

fn mod_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    }
}

/// (group key, [(action key, keys)]).
pub fn groups() -> Vec<(&'static str, Vec<(&'static str, String)>)> {
    let m = mod_name();
    let k = |s: &str| format!("{m}+{s}");
    vec![
        (
            "fileOperations",
            vec![
                ("quickOpen", k("P")),
                ("save", k("S")),
                ("saveAll", k("Alt+S")),
                ("closeTab", k("W")),
            ],
        ),
        (
            "search",
            vec![
                ("findInFile", k("F")),
                ("searchInFiles", k("Shift+F")),
                ("goToLine", k("G")),
            ],
        ),
        (
            "navigation",
            vec![
                ("nextTab", "Ctrl+Tab".into()),
                ("previousTab", "Ctrl+Shift+Tab".into()),
                ("toggleSidebar", k("B")),
            ],
        ),
        (
            "editing",
            vec![
                ("undo", k("Z")),
                ("redo", k("Shift+Z")),
                ("selectNextMatch", k("D")),
                ("toggleComment", k("/")),
            ],
        ),
        (
            "markdown",
            vec![("splitPreview", k("Shift+V")), ("fullPreview", k("Alt+V"))],
        ),
        ("general", vec![("closeEditor", "Esc".into())]),
    ]
}

pub fn show(ctx: &egui::Context, open: &mut bool, tr: &I18n) {
    let mut o = *open;
    egui::Window::new(tr.t("team.editor.shortcuts.title"))
        .open(&mut o)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            for (g, items) in groups() {
                ui.strong(tr.t(&format!("team.editor.shortcuts.groups.{g}")));
                egui::Grid::new(format!("sc-{g}"))
                    .num_columns(2)
                    .show(ui, |ui| {
                        for (a, keys) in items {
                            ui.label(tr.t(&format!("team.editor.shortcuts.actions.{a}")));
                            ui.monospace(keys);
                            ui.end_row();
                        }
                    });
                ui.add_space(6.0);
            }
        });
    *open = o;
}
