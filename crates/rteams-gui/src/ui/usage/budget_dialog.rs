//! "Configure budget" dialog: scope (all teams / a team), period, token and API-equivalent USD
//! limits, notification thresholds, notifications switch, Save / Delete / Cancel.

use std::path::Path;

use eframe::egui;
use rteams_services::i18n::I18n;
use rteams_services::usage::budget::{self, Budget, BudgetSettings, Period};

#[derive(Debug, Default)]
pub struct BudgetDialog {
    open: bool,
    /// `None` = all teams.
    scope: Option<String>,
    period: Period,
    tokens: String,
    cost: String,
    notify: bool,
    thresholds: Vec<i64>,
    delete_pending: bool,
    error: Option<String>,
}

impl BudgetDialog {
    /// Open for `scope`, seeding the draft from the saved budget (or defaults).
    pub fn open(&mut self, home: &Path, scope: Option<String>) {
        self.load_scope(&budget::load(home), scope);
        self.open = true;
    }

    fn load_scope(&mut self, s: &BudgetSettings, scope: Option<String>) {
        let b = match &scope {
            None => s.global.clone(),
            Some(t) => s.teams.get(t).cloned(),
        }
        .unwrap_or_default();
        self.scope = scope;
        self.period = b.period;
        self.tokens = b.token_limit.map(|t| t.to_string()).unwrap_or_default();
        self.cost = b.cost_limit_usd.map(|c| c.to_string()).unwrap_or_default();
        self.notify = b.notify;
        self.thresholds = b.thresholds.iter().map(|t| *t as i64).collect();
        self.delete_pending = false;
        self.error = None;
    }

    /// Validate the draft into a budget (`Err` = catalog key of the problem).
    fn build(&self) -> Result<Budget, &'static str> {
        let tokens = match self.tokens.trim() {
            "" => None,
            s => match s.replace(['_', ','], "").parse::<u64>() {
                Ok(n) if n > 0 => Some(n),
                _ => return Err("invalidLimit"),
            },
        };
        let cost = match self.cost.trim() {
            "" => None,
            s => match s.trim_start_matches('$').parse::<f64>() {
                Ok(c) if c.is_finite() && c > 0.0 => Some(c),
                _ => return Err("invalidLimit"),
            },
        };
        if tokens.is_none() && cost.is_none() {
            return Err("requiredLimit");
        }
        let thresholds =
            budget::normalize_thresholds(&self.thresholds).ok_or("invalidThresholds")?;
        Ok(Budget {
            token_limit: tokens,
            cost_limit_usd: cost,
            thresholds,
            notify: self.notify,
            period: self.period,
        })
    }

    /// Draw the dialog; returns true when settings were saved or deleted.
    pub fn show(&mut self, ctx: &egui::Context, tr: &I18n, home: &Path, teams: &[String]) -> bool {
        if !self.open {
            return false;
        }
        let t = |k: &str| tr.t(&format!("dashboard.tokenUsage.budgets.{k}"));
        let e = |k: &str| tr.t(&format!("dashboard.tokenUsage.budgets.editor.{k}"));
        let mut changed = false;
        let mut close = false;
        let mut open = true;
        egui::Window::new(e("title"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(egui::RichText::new(e("description")).small().weak());
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(e("scope"));
                    let current = self.scope.clone();
                    let label = current.clone().unwrap_or_else(|| t("allTeams"));
                    let mut pick = None;
                    egui::ComboBox::from_id_salt("budget_scope")
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(current.is_none(), t("allTeams"))
                                .clicked()
                            {
                                pick = Some(None);
                            }
                            for team in teams.iter().filter(|x| !x.is_empty()) {
                                if ui
                                    .selectable_label(current.as_deref() == Some(team), team)
                                    .clicked()
                                {
                                    pick = Some(Some(team.clone()));
                                }
                            }
                        });
                    if let Some(p) = pick {
                        self.load_scope(&budget::load(home), p);
                    }
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.period, Period::Daily, "Daily");
                    ui.selectable_value(&mut self.period, Period::Monthly, "Monthly");
                });
                egui::Grid::new("budget_limits")
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.label(t("tokenLimit"));
                        ui.text_edit_singleline(&mut self.tokens);
                        ui.end_row();
                        ui.label(t("costLimit"));
                        ui.text_edit_singleline(&mut self.cost);
                        ui.end_row();
                    });
                ui.checkbox(&mut self.notify, e("notifications"));
                ui.label(e("thresholds"));
                if self.thresholds.is_empty() {
                    ui.label(egui::RichText::new(e("noThresholds")).small().weak());
                }
                let mut remove = None;
                for (i, th) in self.thresholds.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(th).range(1..=100).suffix("%"));
                        if ui
                            .small_button("🗙")
                            .on_hover_text(tr.t_args(
                                "dashboard.tokenUsage.budgets.editor.removeThreshold",
                                &[("index", &(i + 1).to_string())],
                            ))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.thresholds.remove(i);
                }
                if ui.button(e("addThreshold")).clicked()
                    && self.thresholds.len() < budget::MAX_THRESHOLDS
                {
                    self.thresholds.push(90);
                }
                if self.delete_pending {
                    ui.colored_label(crate::theme::WARNING, e("deletePending"));
                }
                if let Some(err) = &self.error {
                    ui.colored_label(
                        crate::theme::ERROR,
                        tr.t(&format!("dashboard.tokenUsage.budgets.editor.{err}")),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button(e("save")).clicked() {
                        changed |= self.save(home);
                        close = changed;
                    }
                    if ui.button(e("delete")).clicked() {
                        self.delete_pending = true;
                        changed |= self.save(home);
                        close = changed;
                    }
                    if ui.button(e("cancel")).clicked() {
                        close = true;
                    }
                });
            });
        if !open || close {
            self.open = false;
        }
        changed
    }

    fn save(&mut self, home: &Path) -> bool {
        let mut s = budget::load(home);
        if self.delete_pending {
            match &self.scope {
                None => s.global = None,
                Some(t) => {
                    s.teams.remove(t);
                }
            }
        } else {
            match self.build() {
                Ok(b) => match &self.scope {
                    None => s.global = Some(b),
                    Some(t) => {
                        s.teams.insert(t.clone(), b);
                    }
                },
                Err(k) => {
                    self.error = Some(k.to_string());
                    return false;
                }
            }
        }
        match budget::save(home, &s) {
            Ok(_) => true,
            Err(_) => {
                self.error = Some("saveFailed".into());
                self.delete_pending = false;
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TmpDir(std::path::PathBuf);
    impl TmpDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rteams-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
    }

    #[test]
    fn validates_and_saves_drafts() {
        let dir = TmpDir::new("budget-dialog");
        let mut d = BudgetDialog::default();
        d.open(dir.0.as_path(), Some("t1".into()));
        assert_eq!(d.thresholds, vec![80, 100]);
        assert!(!d.save(dir.0.as_path()));
        assert_eq!(d.error.as_deref(), Some("requiredLimit"));
        d.tokens = "abc".into();
        assert_eq!(d.build().unwrap_err(), "invalidLimit");
        d.tokens = "1_000".into();
        d.cost = "$2.5".into();
        d.thresholds = vec![50, 50];
        assert_eq!(d.build().unwrap_err(), "invalidThresholds");
        d.thresholds = vec![50, 100];
        assert!(d.save(dir.0.as_path()));
        let s = budget::load(dir.0.as_path());
        let b = &s.teams["t1"];
        assert_eq!(
            (b.token_limit, b.cost_limit_usd, b.thresholds.clone()),
            (Some(1000), Some(2.5), vec![50, 100])
        );
        d.delete_pending = true;
        assert!(d.save(dir.0.as_path()));
        assert!(budget::load(dir.0.as_path()).teams.is_empty());
    }
}
