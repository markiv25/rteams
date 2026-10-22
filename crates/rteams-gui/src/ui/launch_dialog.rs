//! Launch dialog (the original's LaunchTeamDialog): pick which members to launch, effort,
//! fast mode, a context limit and the permission policy. `show` returns the chosen
//! `LaunchOptions` once when the user confirms; the shell turns them into launches.

use std::collections::BTreeSet;

use eframe::egui::{self, RichText};

use crate::launcher::Permissions;
use crate::theme;
use crate::view::{Intents, View};

/// What the user chose in the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    pub members: Vec<String>,
    /// `low` | `medium` | `high`; `None` = the model default.
    pub effort: Option<String>,
    pub fast: bool,
    /// Context limit in tokens; `None` = the model default.
    pub context_limit: Option<u32>,
    pub permissions: Permissions,
    /// Extra Claude CLI flags, already validated and split.
    pub extra_args: Vec<String>,
    /// The raw extra-args text, saved per team so later launches reuse it.
    pub extra_raw: String,
}

pub const EFFORTS: [&str; 3] = ["low", "medium", "high"];

#[derive(Debug, Default)]
pub struct LaunchDialog {
    open: bool,
    /// Every launchable member, in roster order.
    all: Vec<String>,
    selected: BTreeSet<String>,
    effort: Option<String>,
    fast: bool,
    limit: String,
    permissions: Permissions,
    /// Extra CLI flags text (seeded from the team's saved value).
    pub extra: String,
    error: Option<String>,
}

/// Parses the context-limit field: empty = no limit, else a positive whole number.
pub fn parse_limit(s: &str) -> Result<Option<u32>, ()> {
    let s = s.trim().replace([',', '_', ' '], "");
    if s.is_empty() {
        return Ok(None);
    }
    match s.parse::<u32>() {
        Ok(0) | Err(_) => Err(()),
        Ok(n) => Ok(Some(n)),
    }
}

impl LaunchDialog {
    /// Opens with every member in `members` selected; other settings keep their last values.
    pub fn open(&mut self, members: &[String]) {
        self.all = members.to_vec();
        self.selected = members.iter().cloned().collect();
        self.error = None;
        self.open = true;
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.open
    }

    #[cfg(test)]
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Extra args check: protected flags always, unknown flags when `claude --help` runs.
    fn extra_check(&self) -> Result<Vec<String>, String> {
        if self.extra.trim().is_empty() {
            return Ok(vec![]);
        }
        crate::launcher::validate_extra_args(&self.extra, crate::launcher::claude_help())
    }

    /// The options for the current form, or why they are invalid.
    fn options(&self) -> Result<LaunchOptions, &'static str> {
        let members: Vec<String> = self
            .all
            .iter()
            .filter(|m| self.selected.contains(*m))
            .cloned()
            .collect();
        if members.is_empty() {
            return Err("rteams.launch.noMembers");
        }
        let context_limit = parse_limit(&self.limit).map_err(|()| "rteams.launch.badLimit")?;
        let extra_args = self.extra_check().map_err(|_| "rteams.launch.badArgs")?;
        Ok(LaunchOptions {
            members,
            effort: self.effort.clone(),
            fast: self.fast,
            context_limit,
            permissions: self.permissions.clone(),
            extra_args,
            extra_raw: self.extra.trim().to_string(),
        })
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        v: &View,
        _out: &mut Intents,
    ) -> Option<LaunchOptions> {
        if !self.open {
            return None;
        }
        let tr = v.tr;
        let mut open = true;
        let mut result = None;
        let mut launch = false;
        let mut cancel = false;
        let ok = self.options().is_ok();
        crate::forms::dialog_with_footer(
            ctx,
            "launch_dialog",
            &tr.t("team.launch.title.launch"),
            460.0,
            &mut open,
            |ui| self.body(ui, v),
            |ui| {
                if ui
                    .add_enabled_ui(ok, |ui| {
                        theme::solid(ui, &tr.t("team.launch.actions.launchTeam"), theme::SUCCESS)
                    })
                    .inner
                    .clicked()
                {
                    launch = true;
                }
                if ui.button(tr.t("common.actions.cancel")).clicked() {
                    cancel = true;
                }
            },
        );
        if cancel {
            self.open = false;
        }
        if launch {
            match self.options() {
                Ok(o) => {
                    result = Some(o);
                    self.open = false;
                }
                Err(k) => self.error = Some(tr.t(k)),
            }
        }
        if !open {
            self.open = false;
        }
        result
    }

    fn body(&mut self, ui: &mut egui::Ui, v: &View) {
        let tr = v.tr;
        ui.horizontal(|ui| {
            ui.label(RichText::new(tr.t("rteams.launch.members")).strong());
            if ui.small_button(tr.t("rteams.launch.all")).clicked() {
                self.selected = self.all.iter().cloned().collect();
            }
            if ui.small_button(tr.t("rteams.launch.none")).clicked() {
                self.selected.clear();
            }
        });
        for name in &self.all {
            let mut on = self.selected.contains(name);
            let role = v
                .members()
                .iter()
                .find(|m| &m.name == name)
                .map_or("", |m| m.role.as_str());
            let text = RichText::new(format!("{name}  \u{b7}  {role}")).color(v.color(name));
            if ui.checkbox(&mut on, text).changed() {
                if on {
                    self.selected.insert(name.clone());
                } else {
                    self.selected.remove(name);
                }
            }
        }
        if self.selected.is_empty() {
            ui.label(RichText::new(tr.t("rteams.launch.noMembers")).color(theme::WARNING));
        }
        ui.add_space(8.0);
        theme::rule(ui);
        ui.add_space(8.0);

        ui.label(RichText::new(tr.t("team.effortLevel.label")).color(theme::TEXT_2));
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.effort.is_none(), tr.t("rteams.launch.effortDefault"))
                .clicked()
            {
                self.effort = None;
            }
            for e in EFFORTS {
                let picked = self.effort.as_deref() == Some(e);
                if ui
                    .selectable_label(picked, tr.t(&format!("rteams.launch.effort.{e}")))
                    .clicked()
                {
                    self.effort = Some(e.to_string());
                }
            }
        });
        ui.add_space(6.0);
        ui.checkbox(
            &mut self.fast,
            tr.t("team.modelSelector.fastMode.optionalLabel"),
        )
        .on_hover_text(tr.t("rteams.launch.fastHint"));
        ui.add_space(6.0);
        ui.label(RichText::new(tr.t("rteams.launch.limit")).color(theme::TEXT_2));
        let bad = parse_limit(&self.limit).is_err();
        ui.add(
            egui::TextEdit::singleline(&mut self.limit)
                .hint_text(tr.t("rteams.launch.limitHint"))
                .desired_width(180.0),
        );
        if bad {
            ui.label(RichText::new(tr.t("rteams.launch.badLimit")).color(theme::ERROR));
        }
        ui.add_space(6.0);
        ui.label(RichText::new(tr.t("rteams.launch.permissions")).color(theme::TEXT_2));
        ui.horizontal_wrapped(|ui| {
            for (p, key) in [
                (Permissions::SkipAll, "rteams.launch.permSkip"),
                (Permissions::EditsOnly, "rteams.launch.permEdits"),
                (Permissions::Manual, "rteams.launch.permAsk"),
            ] {
                if ui
                    .selectable_label(self.permissions == p, tr.t(key))
                    .clicked()
                {
                    self.permissions = p;
                }
            }
        });
        ui.add_space(6.0);
        ui.label(RichText::new(tr.t("rteams.launch.extraArgs")).color(theme::TEXT_2));
        ui.add(
            egui::TextEdit::singleline(&mut self.extra)
                .hint_text("--add-dir ../shared")
                .desired_width(f32::INFINITY),
        );
        if let Err(e) = self.extra_check() {
            ui.label(
                RichText::new(format!("{}: {e}", tr.t("rteams.launch.badArgs")))
                    .color(theme::ERROR),
            );
        }
        if let Some(e) = &self.error {
            ui.label(RichText::new(e).color(theme::ERROR));
        }
    }
}

#[cfg(test)]
mod launch_dialog_tests {
    use super::*;
    use crate::view::testing::{fixture, render};

    #[test]
    fn limit_parsing() {
        assert_eq!(parse_limit(""), Ok(None));
        assert_eq!(parse_limit(" 200,000 "), Ok(Some(200_000)));
        assert!(parse_limit("0").is_err());
        assert!(parse_limit("abc").is_err());
        assert!(parse_limit("-5").is_err());
    }

    #[test]
    fn options_follow_the_form() {
        let mut d = LaunchDialog::default();
        d.open(&["ada".to_string(), "bob".to_string()]);
        d.selected.remove("ada");
        d.effort = Some("high".into());
        d.fast = true;
        d.limit = "50000".into();
        d.permissions = Permissions::Manual;
        assert_eq!(
            d.options(),
            Ok(LaunchOptions {
                members: vec!["bob".into()],
                effort: Some("high".into()),
                fast: true,
                context_limit: Some(50_000),
                permissions: Permissions::Manual,
                extra_args: vec![],
                extra_raw: String::new(),
            })
        );
        d.extra = "--resume x".into();
        assert!(d.options().is_err(), "protected flag rejected");
        d.extra.clear();
        d.selected.clear();
        assert!(d.options().is_err());
        d.selected.insert("bob".into());
        d.limit = "x".into();
        assert!(d.options().is_err());
    }

    #[test]
    fn renders_headless() {
        let (home, data) = fixture("launchdlg");
        let view = View {
            state: data.state(),
            data: &data,
            actor: Some("boss"),
            agents: &[],
            tr: crate::view::testing::tr(),
        };
        let mut d = LaunchDialog::default();
        d.open(&["ada".to_string(), "bob".to_string()]);
        d.limit = "oops".into();
        render(|ui| {
            assert!(d.show(ui.ctx(), &view, &mut Intents::default()).is_none());
        });
        assert!(d.is_open());
        d.close();
        let _ = std::fs::remove_dir_all(home);
    }
}
