//! Usage budgets: a global budget and per-team budgets, each daily or monthly (UTC) with a token
//! limit and/or an API-equivalent USD limit, plus notification thresholds. Settings persist in
//! `<home>/usage-budgets.json`; fired thresholds in `<home>/usage-budgets-notified.json` so each
//! threshold notifies once per period.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use super::ledger::{now_ms, UsageRecord};
use super::summary::{day_of, day_start_ms};
use crate::{atomic, paths::AppPaths};

pub const SCHEMA_VERSION: u64 = 1;
pub const MAX_THRESHOLDS: usize = 10;
const DAY_MS: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    Daily,
    #[default]
    Monthly,
}

fn default_thresholds() -> Vec<u8> {
    vec![80, 100]
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    #[serde(default)]
    pub token_limit: Option<u64>,
    #[serde(default)]
    pub cost_limit_usd: Option<f64>,
    #[serde(default = "default_thresholds")]
    pub thresholds: Vec<u8>,
    #[serde(default = "yes")]
    pub notify: bool,
    #[serde(default)]
    pub period: Period,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            token_limit: None,
            cost_limit_usd: None,
            thresholds: default_thresholds(),
            notify: true,
            period: Period::Monthly,
        }
    }
}

impl Budget {
    /// At least one positive finite limit.
    pub fn has_limit(&self) -> bool {
        self.token_limit.is_some_and(|t| t > 0)
            || self
                .cost_limit_usd
                .is_some_and(|c| c.is_finite() && c > 0.0)
    }
}

/// Unique whole percents in 1..=100, sorted, at most 10; `None` when the input is invalid.
pub fn normalize_thresholds(t: &[i64]) -> Option<Vec<u8>> {
    let set: BTreeSet<i64> = t.iter().copied().collect();
    if set.len() != t.len()
        || t.len() > MAX_THRESHOLDS
        || set.iter().any(|p| !(1..=100).contains(p))
    {
        return None;
    }
    Some(set.into_iter().map(|p| p as u8).collect())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetSettings {
    #[serde(default)]
    pub schema_version: u64,
    /// Master switch for budget notifications.
    #[serde(default = "yes")]
    pub notifications_enabled: bool,
    #[serde(default)]
    pub global: Option<Budget>,
    #[serde(default)]
    pub teams: BTreeMap<String, Budget>,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl Default for BudgetSettings {
    fn default() -> Self {
        BudgetSettings {
            schema_version: SCHEMA_VERSION,
            notifications_enabled: true,
            global: None,
            teams: BTreeMap::new(),
            updated_at_ms: 0,
        }
    }
}

fn settings_path(home: &Path) -> std::path::PathBuf {
    AppPaths::new(home).app("usage-budgets.json")
}

pub fn load(home: &Path) -> BudgetSettings {
    std::fs::read(settings_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(home: &Path, s: &BudgetSettings) -> std::io::Result<BudgetSettings> {
    let mut s = s.clone();
    s.schema_version = SCHEMA_VERSION;
    s.updated_at_ms = now_ms();
    atomic::write(
        &settings_path(home),
        &serde_json::to_vec_pretty(&s).map_err(std::io::Error::other)?,
    )?;
    Ok(s)
}

/// `(start_ms, key)` of the period containing `now_ms`.
pub fn budget_period(period: Period, now_ms: u64) -> (u64, String) {
    let d = day_of(now_ms);
    match period {
        Period::Daily => (day_start_ms(d), d.format("%Y-%m-%d").to_string()),
        Period::Monthly => (
            day_start_ms(NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap_or(d)),
            d.format("%Y-%m").to_string(),
        ),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BudgetStatus {
    /// `None` = the global (all teams) budget.
    pub team: Option<String>,
    pub period: Period,
    pub period_key: String,
    pub tokens_used: u64,
    pub token_limit: Option<u64>,
    pub cost_used: f64,
    pub cost_limit: Option<f64>,
    pub token_pct: Option<f64>,
    pub cost_pct: Option<f64>,
    pub level: Level,
    pub next_threshold: Option<u8>,
    pub notify: bool,
    pub thresholds: Vec<u8>,
}

impl BudgetStatus {
    pub fn pct(&self) -> f64 {
        self.token_pct
            .unwrap_or(0.0)
            .max(self.cost_pct.unwrap_or(0.0))
    }
}

pub fn build_status(
    settings: &BudgetSettings,
    records: &[UsageRecord],
    now_ms: u64,
) -> Vec<BudgetStatus> {
    let mut scopes: Vec<(Option<&str>, &Budget)> = Vec::new();
    if let Some(g) = &settings.global {
        scopes.push((None, g));
    }
    scopes.extend(settings.teams.iter().map(|(k, b)| (Some(k.as_str()), b)));
    scopes
        .into_iter()
        .filter(|(_, b)| b.has_limit())
        .map(|(team, b)| {
            let (start, key) = budget_period(b.period, now_ms);
            let end = match b.period {
                Period::Daily => start + DAY_MS,
                Period::Monthly => u64::MAX,
            };
            let (mut tokens, mut cost) = (0u64, 0.0f64);
            for r in records
                .iter()
                .filter(|r| r.ts_ms >= start && r.ts_ms < end && team.is_none_or(|t| r.team == t))
            {
                tokens += r.total(true);
                cost += r.cost_usd;
            }
            let token_limit = b.token_limit.filter(|t| *t > 0);
            let cost_limit = b.cost_limit_usd.filter(|c| c.is_finite() && *c > 0.0);
            let token_pct = token_limit.map(|l| tokens as f64 / l as f64 * 100.0);
            let cost_pct = cost_limit.map(|l| cost / l * 100.0);
            let pct = token_pct.unwrap_or(0.0).max(cost_pct.unwrap_or(0.0));
            let warn_at = b
                .thresholds
                .iter()
                .copied()
                .filter(|t| *t < 100)
                .min()
                .unwrap_or(80);
            let level = if pct >= 100.0 {
                Level::Critical
            } else if pct >= warn_at as f64 {
                Level::Warning
            } else {
                Level::Ok
            };
            BudgetStatus {
                team: team.map(String::from),
                period: b.period,
                period_key: key,
                tokens_used: tokens,
                token_limit,
                cost_used: cost,
                cost_limit,
                token_pct,
                cost_pct,
                level,
                next_threshold: b
                    .thresholds
                    .iter()
                    .copied()
                    .filter(|t| (*t as f64) > pct)
                    .min(),
                notify: b.notify,
                thresholds: b.thresholds.clone(),
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct BudgetAlert {
    /// `scope|metric|threshold|period`: each fires once per period.
    pub key: String,
    pub title: String,
    pub message: String,
    pub critical: bool,
}

/// Every threshold currently crossed by `statuses` that has notifications on.
pub fn crossings(statuses: &[BudgetStatus]) -> Vec<BudgetAlert> {
    let mut out = Vec::new();
    for s in statuses.iter().filter(|s| s.notify) {
        let scope = s.team.clone().unwrap_or_else(|| "all-teams".into());
        let label = s.team.clone().unwrap_or_else(|| "All teams".into());
        let metrics = [
            (
                "tokens",
                s.token_pct,
                format!("{} of {} tokens", s.tokens_used, s.token_limit.unwrap_or(0)),
            ),
            (
                "cost",
                s.cost_pct,
                format!("${:.2} of ${:.2}", s.cost_used, s.cost_limit.unwrap_or(0.0)),
            ),
        ];
        for (metric, pct, detail) in metrics {
            let Some(pct) = pct else { continue };
            for t in s.thresholds.iter().filter(|t| pct >= **t as f64) {
                out.push(BudgetAlert {
                    key: format!("{scope}|{metric}|{t}|{}", s.period_key),
                    title: format!("Usage budget {t}% reached"),
                    message: format!("{label}: {detail} ({pct:.0}%)"),
                    critical: *t >= 100,
                });
            }
        }
    }
    out
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Notified {
    #[serde(default)]
    schema_version: u64,
    #[serde(default)]
    keys: BTreeSet<String>,
}

/// New alerts since the last call (each key is returned once and remembered). Empty when the
/// master switch is off. Call after a successful scan.
pub fn budget_alerts(home: &Path, records: &[UsageRecord], now_ms: u64) -> Vec<BudgetAlert> {
    let settings = load(home);
    if !settings.notifications_enabled {
        return Vec::new();
    }
    let statuses = build_status(&settings, records, now_ms);
    let path = AppPaths::new(home).app("usage-budgets-notified.json");
    let mut n: Notified = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let all = crossings(&statuses);
    let fresh: Vec<BudgetAlert> = all
        .iter()
        .filter(|a| !n.keys.contains(&a.key))
        .cloned()
        .collect();
    // Forget keys of finished periods.
    let live: BTreeSet<&str> = statuses.iter().map(|s| s.period_key.as_str()).collect();
    let before = n.keys.len();
    n.keys
        .retain(|k| k.rsplit('|').next().is_some_and(|p| live.contains(p)));
    let changed = !fresh.is_empty() || n.keys.len() != before;
    n.keys.extend(fresh.iter().map(|a| a.key.clone()));
    if changed {
        n.schema_version = SCHEMA_VERSION;
        if let Ok(b) = serde_json::to_vec_pretty(&n) {
            let _ = atomic::write(&path, &b);
        }
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ledger::testutil::rec;

    // 2025-03-12 12:00 UTC.
    const NOW: u64 = 1_741_780_800_000;

    fn settings() -> BudgetSettings {
        let mut s = BudgetSettings::default();
        s.teams.insert(
            "t1".into(),
            Budget {
                token_limit: Some(1000),
                cost_limit_usd: Some(10.0),
                ..Default::default()
            },
        );
        s
    }

    #[test]
    fn period_and_status() {
        assert_eq!(budget_period(Period::Monthly, NOW).1, "2025-03");
        assert_eq!(budget_period(Period::Daily, NOW).1, "2025-03-12");
        let recs = vec![
            rec("t1", "a", "m", NOW, 400, 450, 1.0),
            rec("t1", "a", "m", NOW - 20 * DAY_MS, 9999, 0, 99.0), // previous month
            rec("t2", "a", "m", NOW, 5000, 0, 50.0),               // other team
        ];
        let st = build_status(&settings(), &recs, NOW);
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].tokens_used, 850);
        assert_eq!(st[0].level, Level::Warning);
        assert_eq!(st[0].next_threshold, Some(100));
        assert!((st[0].pct() - 85.0).abs() < 1e-9);
    }

    #[test]
    fn thresholds_validate() {
        assert_eq!(normalize_thresholds(&[100, 50]), Some(vec![50, 100]));
        assert_eq!(normalize_thresholds(&[0]), None);
        assert_eq!(normalize_thresholds(&[101]), None);
        assert_eq!(normalize_thresholds(&[5, 5]), None);
        assert_eq!(normalize_thresholds(&(1..=11).collect::<Vec<_>>()), None);
    }

    #[test]
    fn alerts_fire_once_per_period_and_respect_master_switch() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = settings();
        s.teams.get_mut("t1").unwrap().cost_limit_usd = None;
        save(dir.path(), &s).unwrap();
        let mut recs = vec![rec("t1", "a", "m", NOW, 400, 450, 1.0)];
        let a = budget_alerts(dir.path(), &recs, NOW);
        assert_eq!(a.len(), 1);
        assert!(a[0].key.ends_with("tokens|80|2025-03") && !a[0].critical);
        assert!(
            budget_alerts(dir.path(), &recs, NOW).is_empty(),
            "already notified"
        );
        recs.push(rec("t1", "a", "m", NOW, 500, 0, 0.0));
        let a = budget_alerts(dir.path(), &recs, NOW);
        assert_eq!(a.len(), 1);
        assert!(a[0].critical);
        // Next month the keys reset.
        let later = NOW + 31 * DAY_MS;
        let recs2 = vec![rec("t1", "a", "m", later, 2000, 0, 0.0)];
        assert_eq!(budget_alerts(dir.path(), &recs2, later).len(), 2);
        // Master switch off.
        let mut off = load(dir.path());
        off.notifications_enabled = false;
        save(dir.path(), &off).unwrap();
        assert!(budget_alerts(dir.path(), &recs2, later + 40 * DAY_MS).is_empty());
    }

    #[test]
    fn settings_round_trip_and_daily_period() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), BudgetSettings::default());
        let s = BudgetSettings {
            global: Some(Budget {
                cost_limit_usd: Some(2.0),
                period: Period::Daily,
                ..Default::default()
            }),
            ..Default::default()
        };
        let saved = save(dir.path(), &s).unwrap();
        assert_eq!(load(dir.path()), saved);
        assert!(saved.updated_at_ms > 0);
        let recs = vec![
            rec("x", "a", "m", NOW, 1, 1, 2.5),
            rec("x", "a", "m", NOW - DAY_MS, 1, 1, 100.0),
        ];
        let st = build_status(&saved, &recs, NOW);
        assert_eq!(st[0].level, Level::Critical);
        assert!((st[0].cost_used - 2.5).abs() < 1e-9);
    }
}
