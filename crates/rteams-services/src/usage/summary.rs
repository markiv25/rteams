//! Pure aggregation over the ledger: totals, trend by day, breakdowns, billing split, burn rate,
//! activity heatmap data and run lists. All dates are UTC.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use super::ledger::{Billing, UsageRecord};

const DAY_MS: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    AllTime,
    Today,
    ThisWeek,
    ThisMonth,
    ThisQuarter,
    ThisYear,
    LastWeek,
    LastMonth,
    Last7Days,
    Last30Days,
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 11] = [
        Preset::AllTime,
        Preset::Today,
        Preset::ThisWeek,
        Preset::ThisMonth,
        Preset::ThisQuarter,
        Preset::ThisYear,
        Preset::LastWeek,
        Preset::LastMonth,
        Preset::Last7Days,
        Preset::Last30Days,
        Preset::Custom,
    ];

    /// Suffix of the catalog key `dashboard.tokenUsage.dateRange.presets.<key>`.
    pub fn key(self) -> &'static str {
        match self {
            Preset::AllTime => "all-time",
            Preset::Today => "today",
            Preset::ThisWeek => "this-week",
            Preset::ThisMonth => "this-month",
            Preset::ThisQuarter => "this-quarter",
            Preset::ThisYear => "this-year",
            Preset::LastWeek => "last-week",
            Preset::LastMonth => "last-month",
            Preset::Last7Days => "last-7-days",
            Preset::Last30Days => "last-30-days",
            Preset::Custom => "custom",
        }
    }

    /// `[from, to)` in unix ms for `now_ms`; `None` side means unbounded. Weeks start Monday.
    pub fn range(self, now_ms: u64) -> (Option<u64>, Option<u64>) {
        let today = day_of(now_ms);
        let ms = |d: NaiveDate| day_start_ms(d);
        let week = today - Duration::days(today.weekday().num_days_from_monday() as i64);
        let month = today.with_day(1).unwrap_or(today);
        match self {
            Preset::AllTime | Preset::Custom => (None, None),
            Preset::Today => (Some(ms(today)), Some(ms(today) + DAY_MS)),
            Preset::ThisWeek => (Some(ms(week)), Some(ms(week) + 7 * DAY_MS)),
            Preset::LastWeek => (Some(ms(week) - 7 * DAY_MS), Some(ms(week))),
            Preset::ThisMonth => (Some(ms(month)), Some(ms(next_month(month)))),
            Preset::LastMonth => (Some(ms(prev_month(month))), Some(ms(month))),
            Preset::ThisQuarter => {
                let q = NaiveDate::from_ymd_opt(today.year(), (today.month0() / 3) * 3 + 1, 1)
                    .unwrap_or(today);
                let end = (0..3).fold(q, |d, _| next_month(d));
                (Some(ms(q)), Some(ms(end)))
            }
            Preset::ThisYear => {
                let y = NaiveDate::from_ymd_opt(today.year(), 1, 1).unwrap_or(today);
                let e = NaiveDate::from_ymd_opt(today.year() + 1, 1, 1).unwrap_or(today);
                (Some(ms(y)), Some(ms(e)))
            }
            Preset::Last7Days => (Some(ms(today) - 6 * DAY_MS), Some(ms(today) + DAY_MS)),
            Preset::Last30Days => (Some(ms(today) - 29 * DAY_MS), Some(ms(today) + DAY_MS)),
        }
    }
}

fn next_month(d: NaiveDate) -> NaiveDate {
    if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1)
    }
    .unwrap_or(d)
}

fn prev_month(d: NaiveDate) -> NaiveDate {
    if d.month() == 1 {
        NaiveDate::from_ymd_opt(d.year() - 1, 12, 1)
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() - 1, 1)
    }
    .unwrap_or(d)
}

pub fn day_of(ms: u64) -> NaiveDate {
    DateTime::<Utc>::from_timestamp_millis(ms as i64)
        .map(|d| d.date_naive())
        .unwrap_or_default()
}

pub fn day_start_ms(d: NaiveDate) -> u64 {
    d.and_hms_opt(0, 0, 0)
        .map(|t| t.and_utc().timestamp_millis().max(0) as u64)
        .unwrap_or(0)
}

/// `YYYY-MM-DD` to the unix ms of that UTC day's start.
pub fn parse_day(s: &str) -> Option<u64> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .ok()
        .map(day_start_ms)
}

pub fn day_label(ms: u64) -> String {
    day_of(ms).format("%Y-%m-%d").to_string()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Filter {
    pub from_ms: Option<u64>,
    /// Exclusive.
    pub to_ms: Option<u64>,
    /// Empty = all teams. Anonymous (imported) runs have the empty team name.
    pub teams: BTreeSet<String>,
    pub include_cache: bool,
}

impl Filter {
    pub fn matches(&self, r: &UsageRecord) -> bool {
        self.from_ms.is_none_or(|f| r.ts_ms >= f)
            && self.to_ms.is_none_or(|t| r.ts_ms < t)
            && (self.teams.is_empty() || self.teams.contains(&r.team))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Totals {
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub reasoning: u64,
    /// Respects `include_cache`.
    pub tokens: u64,
    pub cost: f64,
    pub requests: u64,
    pub runs: u64,
    /// Requests that carry a non-zero cost.
    pub priced: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Row {
    pub key: String,
    pub input: u64,
    pub output: u64,
    pub cache: u64,
    pub reasoning: u64,
    pub tokens: u64,
    pub cost: f64,
    pub requests: u64,
    pub runs: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DayRow {
    pub day_ms: u64,
    pub tokens: u64,
    pub cost: f64,
    pub requests: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Burn {
    pub daily_cost: f64,
    pub daily_tokens: f64,
    pub days: usize,
    pub forecast_week: f64,
    pub forecast_month: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunRow {
    pub run: String,
    pub team: String,
    pub member: String,
    pub runtime: String,
    pub model: String,
    pub ts_ms: u64,
    pub tokens: u64,
    pub cost: f64,
    pub requests: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub totals: Totals,
    /// Contiguous days from the first to the last active day (zero-filled).
    pub by_day: Vec<DayRow>,
    pub by_team: Vec<Row>,
    /// Key is `team/member`.
    pub by_member: Vec<Row>,
    pub by_model: Vec<Row>,
    pub by_runtime: Vec<Row>,
    /// Keys `api`, `subscription`, `free`, `unknown`.
    pub by_billing: Vec<Row>,
    pub burn: Burn,
    /// Consecutive active days ending on the last active day.
    pub streak: u32,
    /// Tokens by weekday, Monday first.
    pub weekday_tokens: [u64; 7],
    pub recent_runs: Vec<RunRow>,
    pub expensive_runs: Vec<RunRow>,
}

/// Every team present in the ledger (for the filter popup), sorted.
pub fn teams_of(records: &[UsageRecord]) -> Vec<String> {
    records
        .iter()
        .map(|r| r.team.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn billing_key(b: Billing) -> &'static str {
    match b {
        Billing::Api => "api",
        Billing::Subscription => "subscription",
        Billing::Free => "free",
        Billing::Unknown => "unknown",
    }
}

#[derive(Default)]
struct Acc {
    row: Row,
    runs: HashSet<String>,
}

fn add(m: &mut BTreeMap<String, Acc>, key: String, r: &UsageRecord, inc: bool) {
    let a = m.entry(key.clone()).or_default();
    a.row.key = key;
    a.row.input += r.input;
    a.row.output += r.output;
    a.row.cache += r.cache();
    a.row.reasoning += r.reasoning;
    a.row.tokens += r.total(inc);
    a.row.cost += r.cost_usd;
    a.row.requests += 1;
    a.runs.insert(r.run.clone());
}

fn rows(m: BTreeMap<String, Acc>) -> Vec<Row> {
    let mut v: Vec<Row> = m
        .into_values()
        .map(|mut a| {
            a.row.runs = a.runs.len() as u64;
            a.row
        })
        .collect();
    v.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(
                b.cost
                    .partial_cmp(&a.cost)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
            .then(a.key.cmp(&b.key))
    });
    v
}

pub fn summarize(records: &[UsageRecord], filter: &Filter, now_ms: u64) -> Summary {
    let inc = filter.include_cache;
    let mut s = Summary::default();
    let (mut team, mut member, mut model, mut runtime, mut billing) = (
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
        BTreeMap::new(),
    );
    let mut days: BTreeMap<u64, DayRow> = BTreeMap::new();
    let mut runs: BTreeMap<String, RunRow> = BTreeMap::new();
    let mut run_set = HashSet::new();

    for r in records.iter().filter(|r| filter.matches(r)) {
        let t = &mut s.totals;
        t.input += r.input;
        t.output += r.output;
        t.cache_creation += r.cache_creation;
        t.cache_read += r.cache_read;
        t.reasoning += r.reasoning;
        t.tokens += r.total(inc);
        t.cost += r.cost_usd;
        t.requests += 1;
        if r.cost_usd > 0.0 {
            t.priced += 1;
        }
        run_set.insert(r.run.clone());
        add(&mut team, r.team.clone(), r, inc);
        add(&mut member, format!("{}/{}", r.team, r.member), r, inc);
        add(
            &mut model,
            if r.model.is_empty() {
                "unknown".into()
            } else {
                r.model.clone()
            },
            r,
            inc,
        );
        add(&mut runtime, r.runtime.clone(), r, inc);
        add(&mut billing, billing_key(r.billing).to_string(), r, inc);

        let d = day_start_ms(day_of(r.ts_ms));
        let e = days.entry(d).or_insert(DayRow {
            day_ms: d,
            ..Default::default()
        });
        e.tokens += r.total(inc);
        e.cost += r.cost_usd;
        e.requests += 1;
        s.weekday_tokens[day_of(r.ts_ms).weekday().num_days_from_monday() as usize] += r.total(inc);

        let e = runs.entry(r.run.clone()).or_insert_with(|| RunRow {
            run: r.run.clone(),
            team: r.team.clone(),
            member: r.member.clone(),
            runtime: r.runtime.clone(),
            model: r.model.clone(),
            ts_ms: r.ts_ms,
            ..Default::default()
        });
        e.tokens += r.total(inc);
        e.cost += r.cost_usd;
        e.requests += 1;
        e.ts_ms = e.ts_ms.max(r.ts_ms);
    }
    s.totals.runs = run_set.len() as u64;
    s.by_team = rows(team);
    s.by_member = rows(member);
    s.by_model = rows(model);
    s.by_runtime = rows(runtime);
    s.by_billing = rows(billing);

    // Zero-filled contiguous trend.
    if let (Some((&first, _)), Some((&last, _))) = (days.first_key_value(), days.last_key_value()) {
        let n = ((last - first) / DAY_MS).min(800);
        for i in 0..=n {
            let d = last - (n - i) * DAY_MS;
            s.by_day.push(days.remove(&d).unwrap_or(DayRow {
                day_ms: d,
                ..Default::default()
            }));
        }
    }
    s.streak = {
        let mut n = 0;
        for d in s.by_day.iter().rev() {
            if d.requests > 0 {
                n += 1;
            } else {
                break;
            }
        }
        n
    };
    s.burn = burn_rate(&s.by_day, now_ms);

    let mut all: Vec<RunRow> = runs.into_values().collect();
    all.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms).then(a.run.cmp(&b.run)));
    s.recent_runs = all.iter().take(50).cloned().collect();
    all.sort_by(|a, b| {
        b.cost
            .partial_cmp(&a.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.tokens.cmp(&a.tokens))
    });
    s.expensive_runs = all.into_iter().take(20).collect();
    s
}

/// Mean over the last (up to 7) visible days; forecasts add that rate to the period so far.
pub fn burn_rate(by_day: &[DayRow], now_ms: u64) -> Burn {
    let Some(last) = by_day.last() else {
        return Burn::default();
    };
    let tail = &by_day[by_day.len().saturating_sub(7)..];
    let n = tail.len();
    let daily_cost = tail.iter().map(|d| d.cost).sum::<f64>() / n as f64;
    let daily_tokens = tail.iter().map(|d| d.tokens as f64).sum::<f64>() / n as f64;
    let today = day_of(now_ms);
    let week_start =
        day_start_ms(today - Duration::days(today.weekday().num_days_from_monday() as i64));
    let month_start = day_start_ms(today.with_day(1).unwrap_or(today));
    let to_date = |from: u64| {
        by_day
            .iter()
            .filter(|d| d.day_ms >= from)
            .map(|d| d.cost)
            .sum::<f64>()
    };
    let days_left_week = 6 - today.weekday().num_days_from_monday() as i64;
    let dim = (next_month(today.with_day(1).unwrap_or(today)) - today.with_day(1).unwrap_or(today))
        .num_days();
    let days_left_month = dim - today.day() as i64;
    let _ = last;
    Burn {
        daily_cost,
        daily_tokens,
        days: n,
        forecast_week: to_date(week_start) + daily_cost * days_left_week as f64,
        forecast_month: to_date(month_start) + daily_cost * days_left_month as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::ledger::testutil::rec;

    // 2025-03-12 (Wednesday) 12:00 UTC.
    const NOW: u64 = 1_741_780_800_000;

    #[test]
    fn totals_filters_and_breakdowns() {
        let mut a = rec("t1", "ada", "claude-sonnet-4-5", NOW, 100, 50, 1.0);
        a.cache_read = 400;
        let b = rec("t1", "bob", "claude-opus-4-1", NOW - DAY_MS, 10, 5, 2.0);
        let c = rec("t2", "cy", "claude-sonnet-4-5", NOW - 3 * DAY_MS, 1, 1, 0.5);
        let recs = vec![a, b, c];
        let all = summarize(&recs, &Filter::default(), NOW);
        assert_eq!(all.totals.tokens, 167);
        assert_eq!(all.totals.requests, 3);
        assert_eq!(all.totals.runs, 3);
        assert!((all.totals.cost - 3.5).abs() < 1e-9);
        assert_eq!(all.by_team[0].key, "t1");
        assert_eq!(
            all.by_model
                .iter()
                .find(|r| r.key == "claude-sonnet-4-5")
                .unwrap()
                .requests,
            2
        );
        let with_cache = summarize(
            &recs,
            &Filter {
                include_cache: true,
                ..Default::default()
            },
            NOW,
        );
        assert_eq!(with_cache.totals.tokens, 567);
        let only_t2 = summarize(
            &recs,
            &Filter {
                teams: ["t2".to_string()].into(),
                ..Default::default()
            },
            NOW,
        );
        assert_eq!(only_t2.totals.requests, 1);
        assert_eq!(teams_of(&recs), vec!["t1", "t2"]);
        // Trend is contiguous: 4 days with a hole 2 days ago.
        assert_eq!(all.by_day.len(), 4);
        assert_eq!(all.by_day[1].requests, 0);
        assert_eq!(all.streak, 2);
        assert_eq!(all.expensive_runs[0].member, "bob");
        assert_eq!(all.recent_runs[0].member, "ada");
    }

    #[test]
    fn presets_resolve_ranges() {
        let (f, t) = Preset::Today.range(NOW);
        assert_eq!(f.unwrap(), day_start_ms(day_of(NOW)));
        assert_eq!(t.unwrap() - f.unwrap(), DAY_MS);
        let (f, _) = Preset::ThisWeek.range(NOW);
        assert_eq!(day_label(f.unwrap()), "2025-03-10");
        let (f, t) = Preset::LastMonth.range(NOW);
        assert_eq!(
            (day_label(f.unwrap()), day_label(t.unwrap())),
            ("2025-02-01".into(), "2025-03-01".into())
        );
        let (f, t) = Preset::ThisQuarter.range(NOW);
        assert_eq!(
            (day_label(f.unwrap()), day_label(t.unwrap())),
            ("2025-01-01".into(), "2025-04-01".into())
        );
        assert_eq!(Preset::AllTime.range(NOW), (None, None));
        assert_eq!(
            parse_day("2025-03-10"),
            Some(day_start_ms(day_of(NOW)) - 2 * DAY_MS)
        );
        assert_eq!(parse_day("nope"), None);
        let f = Filter {
            from_ms: Some(NOW),
            to_ms: Some(NOW + 1),
            ..Default::default()
        };
        assert!(f.matches(&rec("t", "m", "x", NOW, 0, 0, 0.0)));
        assert!(!f.matches(&rec("t", "m", "x", NOW + 1, 0, 0, 0.0)));
    }

    #[test]
    fn burn_rate_forecasts() {
        let day = day_start_ms(day_of(NOW));
        let by_day = vec![
            DayRow {
                day_ms: day - DAY_MS,
                tokens: 100,
                cost: 2.0,
                requests: 1,
            },
            DayRow {
                day_ms: day,
                tokens: 300,
                cost: 4.0,
                requests: 1,
            },
        ];
        let b = burn_rate(&by_day, NOW);
        assert_eq!(b.days, 2);
        assert!((b.daily_cost - 3.0).abs() < 1e-9);
        assert!((b.daily_tokens - 200.0).abs() < 1e-9);
        // Wed: Mon..Wed so far = 6.0 (Tue + Wed), 4 days left in the week (Thu..Sun).
        assert!((b.forecast_week - (6.0 + 3.0 * 4.0)).abs() < 1e-9);
        // March: 31 days, 19 left, month to date 6.0.
        assert!((b.forecast_month - (6.0 + 3.0 * 19.0)).abs() < 1e-9);
        assert_eq!(burn_rate(&[], NOW), Burn::default());
    }
}
