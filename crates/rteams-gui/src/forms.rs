//! Input forms: new task, message, create team. Each form owns its draft text and returns a
//! `GuiCmd` when submitted; `app.rs` runs it and closes the form on success.

use eframe::egui::{self, RichText};
use rteams_services::i18n::I18n;

use crate::actions::GuiCmd;
use crate::theme;
use crate::view::{Intent, Intents};

const ROLES: [&str; 5] = ["developer", "reviewer", "designer", "qa", "devops"];

/// `true` when `name` can be added: non-empty and without whitespace.
pub fn valid_member_name(name: &str) -> bool {
    let n = name.trim();
    !n.is_empty() && !n.chars().any(char::is_whitespace)
}

/// tr.t("rteams.addMember.title") modal. `app.rs` calls `open()`, `show()` each frame, and `clear()` on success.
#[derive(Default)]
pub struct AddMemberDialog {
    open: bool,
    name: String,
    role: String,
}

impl AddMemberDialog {
    /// Open with the name prefilled to the next unused parody name (see `names::suggest`);
    /// `taken` is every member name, removed ones included since their names stay reserved.
    pub fn open<'a>(&mut self, taken: impl IntoIterator<Item = &'a str>) {
        if self.name.trim().is_empty() {
            self.name = crate::names::suggest(taken);
        }
        self.open = true;
    }

    /// Reset the drafts and close.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n, out: &mut Intents) {
        if !self.open {
            return;
        }
        let mut cancel = false;
        let mut add = false;
        let modal = egui::Modal::new(egui::Id::unique("add_member_dialog"))
            .backdrop_color(egui::Color32::from_black_alpha(150))
            .frame(
                egui::Frame::new()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::hairline(0.12)))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::same(20)),
            )
            .show(ctx, |ui| {
                ui.set_width(420.0f32.min((ctx.content_rect().width() - 80.0).max(120.0)));
                ui.label(
                    RichText::new(tr.t("rteams.addMember.title"))
                        .size(18.0)
                        .strong(),
                );
                ui.add_space(8.0);
                ui.label(RichText::new(tr.t("team.editTeam.fields.name")).color(theme::TEXT_2));
                ui.horizontal(|ui| {
                    let name = self.name.trim();
                    theme::avatar(ui, name, theme::ACCENT, 28.0, None);
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text(tr.t("rteams.addMember.namePlaceholder"))
                            .desired_width(f32::INFINITY),
                    );
                });
                if !self.name.is_empty() && !valid_member_name(&self.name) {
                    ui.label(
                        RichText::new(tr.t("rteams.addMember.noSpaces"))
                            .size(11.0)
                            .color(theme::ERROR),
                    );
                }
                ui.add_space(4.0);
                ui.label(RichText::new(tr.t("rteams.addMember.role")).color(theme::TEXT_2));
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("add_member_role")
                        .selected_text(tr.t("rteams.addMember.pick"))
                        .show_ui(ui, |ui| {
                            for r in ROLES {
                                ui.selectable_value(&mut self.role, r.to_string(), r);
                            }
                        });
                    ui.add(
                        egui::TextEdit::singleline(&mut self.role)
                            .hint_text("developer")
                            .desired_width(f32::INFINITY),
                    );
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ok = valid_member_name(&self.name);
                        let b = egui::Button::new(
                            RichText::new(tr.t("team.detail.actions.add"))
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(theme::ACCENT);
                        if ui.add_enabled(ok, b).clicked() {
                            add = true;
                        }
                        if ui.button(tr.t("common.actions.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        if add {
            let role = self.role.trim();
            out.push(Intent::Run(GuiCmd::AddMember {
                name: self.name.trim().to_string(),
                role: if role.is_empty() { "developer" } else { role }.to_string(),
            }));
        }
        if cancel || modal.should_close() {
            self.clear();
        }
    }
}

/// A combo box over `members` with an optional "none" entry. Empty `selected` = none.
/// Centered modal (like `theme::dialog`) that sizes to its content: at most 90% of the screen
/// high, body scrolls, optional pinned footer (right-to-left). `Window::auto_sized` avoids egui's
/// 340x420 default outer size capping the scroll area.
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

pub fn dialog_with_footer(
    ctx: &egui::Context,
    id: impl std::hash::Hash + std::fmt::Debug,
    title: &str,
    width: f32,
    open: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui),
) {
    dialog_impl(
        ctx,
        egui::Id::unique(id),
        title,
        width,
        open,
        body,
        Some(Box::new(footer)),
    );
}

type Footer<'a> = Box<dyn FnOnce(&mut egui::Ui) + 'a>;

fn dialog_impl<'a>(
    ctx: &egui::Context,
    id: egui::Id,
    title: &str,
    width: f32,
    open: &mut bool,
    body: impl FnOnce(&mut egui::Ui),
    footer: Option<Footer<'a>>,
) {
    use crate::icons::Icon;
    if !*open {
        return;
    }
    let screen = ctx.content_rect();
    egui::Area::new(id.with("scrim"))
        .order(egui::Order::Middle)
        .interactable(false)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(110));
        });
    let w = width.min(screen.width() - 32.0).max(240.0);
    let chrome = 40.0 + 44.0 + if footer.is_some() { 64.0 } else { 0.0 };
    let body_max = (screen.height() * 0.9 - chrome).max(80.0);
    let frame = egui::Frame::window(&ctx.global_style())
        .fill(theme::RAISED)
        .inner_margin(egui::Margin::same(20))
        .corner_radius(egui::CornerRadius::same(12));
    egui::Window::new(title)
        .id(id)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .auto_sized()
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(w - 40.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).size(18.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, Icon::X, "").clicked() {
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
                theme::rule(ui);
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), footer);
            }
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.egui_wants_keyboard_input() {
        *open = false;
    }
}

/// Bordered input on a card background (visible on the dark surface); `rows` 1 = single line.
pub fn text_field(ui: &mut egui::Ui, text: &mut String, hint: &str, rows: usize) -> egui::Response {
    egui::Frame::new()
        .fill(crate::theme::CARD)
        .stroke(egui::Stroke::new(1.0, crate::theme::hairline(0.14)))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(10, 7))
        .show(ui, |ui| {
            let edit = if rows <= 1 {
                egui::TextEdit::singleline(text)
            } else {
                egui::TextEdit::multiline(text).desired_rows(rows)
            };
            ui.add(
                edit.hint_text(hint)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY),
            )
        })
        .inner
}

pub fn member_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    none_label: &str,
    members: &[String],
    selected: &mut String,
) {
    let shown = if selected.is_empty() {
        none_label.to_string()
    } else {
        selected.clone()
    };
    egui::ComboBox::from_id_salt(id)
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, String::new(), none_label);
            for m in members {
                ui.selectable_value(selected, m.clone(), m);
            }
        });
}

/// Shown in the central panel when the team does not exist yet.
pub struct CreateTeam {
    pub lead: String,
    /// One member per line: `name` or `name role` (role defaults to `member`).
    members: String,
}

impl CreateTeam {
    pub fn new(lead: String) -> Self {
        CreateTeam {
            lead,
            members: String::new(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, tr: &I18n, team: &str) -> Option<GuiCmd> {
        let mut out = None;
        ui.vertical_centered(|ui| {
            ui.add_space(24.0);
            ui.set_max_width(480.0);
            egui::Frame::new()
                .fill(theme::CARD)
                .stroke(egui::Stroke::new(1.0, theme::hairline(0.10)))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(20))
                .show(ui, |ui| {
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.label(
                            RichText::new(tr.t("team.create.title.create"))
                                .size(20.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(
                                tr.t_args("rteams.createTeam.notExist", &[("team", team)]),
                            )
                            .color(theme::TEXT_2),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(tr.t("rteams.createTeam.youLead")).color(theme::TEXT_2),
                        );
                        ui.add(
                            egui::TextEdit::singleline(&mut self.lead).desired_width(f32::INFINITY),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(tr.t("team.members.editor.title")).color(theme::TEXT_2),
                        );
                        ui.add(
                            egui::TextEdit::multiline(&mut self.members)
                                .desired_rows(5)
                                .desired_width(f32::INFINITY)
                                .hint_text(tr.t("rteams.createTeam.membersHint")),
                        );
                        ui.add_space(10.0);
                        let ready = !self.lead.trim().is_empty();
                        let b = egui::Button::new(
                            RichText::new(tr.t("team.create.title.create"))
                                .strong()
                                .color(egui::Color32::WHITE),
                        )
                        .fill(theme::ACCENT);
                        if ui.add_enabled(ready, b).clicked() {
                            out = Some(GuiCmd::CreateTeam {
                                name: team.to_string(),
                                members: parse_members(&self.members),
                            });
                        }
                    });
                });
        });
        out
    }
}

/// Lines of `name [role]`; role defaults to `member`. Blank lines are skipped.
pub fn parse_members(s: &str) -> Vec<(String, String)> {
    s.lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let name = parts.next()?;
            let role = parts.next().unwrap_or("member");
            Some((name.to_string(), role.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_name_validation() {
        assert!(valid_member_name("ada"));
        assert!(!valid_member_name(""));
        assert!(!valid_member_name("  "));
        assert!(!valid_member_name("a b"));
    }

    #[test]
    fn dialogs_render() {
        let mut d = AddMemberDialog::default();
        d.open(["chad-gpt"]);
        assert_eq!(
            d.name, "sherlock-codes",
            "prefilled with the next free parody name"
        );
        d.name = "my-own-name".into();
        d.open(["chad-gpt"]);
        assert_eq!(d.name, "my-own-name", "a typed name is kept on reopen");
        let mut c = CreateTeam::new("boss".into());
        crate::view::testing::render(|ui| {
            let mut out = Intents::default();
            d.show(ui.ctx(), crate::view::testing::tr(), &mut out);
            assert!(c.show(ui, crate::view::testing::tr(), "t").is_none());
        });
        assert!(d.is_open());
        d.clear();
        assert!(!d.is_open());
    }

    #[test]
    fn members_default_to_member_role() {
        assert_eq!(
            parse_members("ada dev\n\n bob \n"),
            vec![
                ("ada".to_string(), "dev".to_string()),
                ("bob".to_string(), "member".to_string())
            ]
        );
    }
}
