//! Notification triggers over session data: built-in and custom rules ("session ended", "error
//! in tool", "cost over $X", "tokens over N", "keyword match") with per-rule enable, ignore
//! patterns and repository scope. Rules are evaluated incrementally on new session data by
//! `TriggerWatcher`; nothing fires for history that existed when the watcher started.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::chunks::{load_session, Block, Role, Session};
use super::paths::{discover, Roots};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    /// No new activity for `threshold` minutes.
    SessionEnded,
    /// A tool result with `is_error` (optionally matching `pattern`, scoped to `tool_name`).
    ToolError,
    /// Estimated session cost reached `threshold` USD.
    CostOver,
    /// Session tokens reached `threshold`.
    TokensOver,
    /// `pattern` matches assistant text or a tool result.
    Keyword,
}

impl TriggerKind {
    pub const ALL: [TriggerKind; 5] = [
        TriggerKind::ToolError,
        TriggerKind::SessionEnded,
        TriggerKind::CostOver,
        TriggerKind::TokensOver,
        TriggerKind::Keyword,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriggerKind::SessionEnded => "Session ended",
            TriggerKind::ToolError => "Tool error",
            TriggerKind::CostOver => "Cost over",
            TriggerKind::TokensOver => "Tokens over",
            TriggerKind::Keyword => "Keyword match",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TriggerRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub builtin: bool,
    pub kind: TriggerKind,
    /// Regex (empty = match everything for `ToolError`).
    pub pattern: String,
    /// Skip when any of these regexes matches.
    pub ignore_patterns: Vec<String>,
    /// Only this tool (empty = any).
    pub tool_name: String,
    /// USD for `CostOver`, tokens for `TokensOver`, idle minutes for `SessionEnded`.
    pub threshold: f64,
    /// Only sessions whose cwd contains one of these (empty = all repositories).
    pub repos: Vec<String>,
    /// `#rrggbb`.
    pub color: String,
}

impl Default for TriggerRule {
    fn default() -> Self {
        TriggerRule {
            id: String::new(),
            name: String::new(),
            enabled: true,
            builtin: false,
            kind: TriggerKind::ToolError,
            pattern: String::new(),
            ignore_patterns: Vec::new(),
            tool_name: String::new(),
            threshold: 0.0,
            repos: Vec::new(),
            color: "#ef4444".into(),
        }
    }
}

impl TriggerRule {
    /// A new custom rule with a fresh id.
    pub fn custom(name: &str, kind: TriggerKind) -> TriggerRule {
        TriggerRule {
            id: format!("custom-{}", uuid::Uuid::new_v4().simple()),
            name: name.to_string(),
            kind,
            threshold: match kind {
                TriggerKind::SessionEnded => 5.0,
                TriggerKind::CostOver => 5.0,
                TriggerKind::TokensOver => 1_000_000.0,
                _ => 0.0,
            },
            ..TriggerRule::default()
        }
    }

    /// First problem with the rule's regexes, if any.
    pub fn validate(&self) -> Result<(), String> {
        if self.kind == TriggerKind::Keyword && self.pattern.trim().is_empty() {
            return Err("pattern required".into());
        }
        validate_regex(&self.pattern)?;
        self.ignore_patterns
            .iter()
            .try_for_each(|p| validate_regex(p))
    }
}

pub fn validate_regex(p: &str) -> Result<(), String> {
    if p.is_empty() {
        return Ok(());
    }
    Regex::new(p).map(|_| ()).map_err(|e| e.to_string())
}

/// The built-in rules (all disabled until the user opts in).
pub fn default_rules() -> Vec<TriggerRule> {
    let b = |id: &str, name: &str, kind, threshold: f64, color: &str| TriggerRule {
        id: id.into(),
        name: name.into(),
        enabled: false,
        builtin: true,
        kind,
        threshold,
        color: color.into(),
        ..TriggerRule::default()
    };
    vec![
        b(
            "builtin-tool-error",
            "Tool Result Error",
            TriggerKind::ToolError,
            0.0,
            "#ef4444",
        ),
        b(
            "builtin-session-ended",
            "Session Ended",
            TriggerKind::SessionEnded,
            5.0,
            "#22c55e",
        ),
        b(
            "builtin-cost",
            "Session Cost Over $5",
            TriggerKind::CostOver,
            5.0,
            "#f59e0b",
        ),
        b(
            "builtin-tokens",
            "High Token Usage",
            TriggerKind::TokensOver,
            1_000_000.0,
            "#8b5cf6",
        ),
    ]
}

fn rules_path(home: &Path) -> std::path::PathBuf {
    home.join("triggers.json")
}

/// Saved rules with the built-ins always present (their saved settings win).
pub fn load_rules(home: &Path) -> Vec<TriggerRule> {
    let saved: Vec<TriggerRule> = crate::atomic::read_json(&rules_path(home))
        .and_then(|v| serde_json::from_value(v.get("rules")?.clone()).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    for d in default_rules() {
        match saved.iter().find(|s| s.id == d.id) {
            Some(s) => out.push(TriggerRule {
                builtin: true,
                ..s.clone()
            }),
            None => out.push(d),
        }
    }
    out.extend(
        saved
            .into_iter()
            .filter(|s| !s.builtin && !s.id.starts_with("builtin-")),
    );
    out
}

pub fn save_rules(home: &Path, rules: &[TriggerRule]) -> std::io::Result<()> {
    let v = serde_json::json!({ "schema_version": 1, "rules": rules });
    crate::atomic::write_json(&rules_path(home), &v)
}

/// A fired trigger.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerHit {
    pub rule_id: String,
    pub rule_name: String,
    pub color: String,
    pub session_id: String,
    pub path: String,
    pub cwd: String,
    pub title: String,
    pub body: String,
    pub ts_ms: Option<i64>,
}

/// What has already been seen of one session.
#[derive(Debug, Clone, Default)]
pub struct SeenState {
    pub msgs: usize,
    /// Once-per-session rules that already fired.
    pub fired: HashSet<String>,
}

struct Compiled {
    pattern: Option<Regex>,
    ignore: Vec<Regex>,
}

fn compile(r: &TriggerRule) -> Option<Compiled> {
    let pattern = if r.pattern.is_empty() {
        None
    } else {
        Some(Regex::new(&r.pattern).ok()?)
    };
    Some(Compiled {
        pattern,
        ignore: r
            .ignore_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect(),
    })
}

impl Compiled {
    fn accepts(&self, text: &str) -> bool {
        self.pattern.as_ref().is_none_or(|p| p.is_match(text))
            && !self.ignore.iter().any(|i| i.is_match(text))
    }
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " ");
    let mut o: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        o.push('\u{2026}');
    }
    o
}

fn in_scope(r: &TriggerRule, s: &Session) -> bool {
    r.repos.is_empty()
        || s.cwd.as_deref().is_some_and(|c| {
            r.repos
                .iter()
                .any(|p| !p.is_empty() && c.contains(p.as_str()))
        })
}

fn project_name(s: &Session) -> String {
    s.cwd
        .as_deref()
        .and_then(|c| Path::new(c).file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| s.id.clone())
}

/// Evaluate `rules` over the part of `s` that `st` has not seen, then advance `st`.
/// `last_activity_ms` is the newest sign of life (record time or file mtime).
pub fn evaluate(
    rules: &[TriggerRule],
    s: &Session,
    st: &mut SeenState,
    now_ms: i64,
    last_activity_ms: i64,
) -> Vec<TriggerHit> {
    let mut hits = Vec::new();
    let hit = |r: &TriggerRule, title: String, body: String, ts: Option<i64>| TriggerHit {
        rule_id: r.id.clone(),
        rule_name: r.name.clone(),
        color: r.color.clone(),
        session_id: s.id.clone(),
        path: s.path.to_string_lossy().into_owned(),
        cwd: s.cwd.clone().unwrap_or_default(),
        title,
        body,
        ts_ms: ts,
    };
    // tool id -> (name, command-ish summary) for results
    let mut calls: HashMap<&str, (&str, String)> = HashMap::new();
    for m in &s.msgs {
        for b in &m.blocks {
            if let Block::ToolUse { id, name, input } = b {
                calls.insert(id, (name, super::tools::summarize(name, input)));
            }
        }
    }
    let fresh = &s.msgs[st.msgs.min(s.msgs.len())..];
    let totals = s.grand_totals();

    for r in rules.iter().filter(|r| r.enabled && in_scope(r, s)) {
        let Some(c) = compile(r) else { continue };
        match r.kind {
            TriggerKind::ToolError => {
                for m in fresh {
                    for b in &m.blocks {
                        let Block::ToolResult {
                            id,
                            content,
                            is_error: true,
                        } = b
                        else {
                            continue;
                        };
                        let (name, sum) = calls
                            .get(id.as_str())
                            .cloned()
                            .unwrap_or(("tool", String::new()));
                        if !r.tool_name.is_empty() && r.tool_name != name {
                            continue;
                        }
                        if !c.accepts(content) {
                            continue;
                        }
                        hits.push(hit(
                            r,
                            format!("Error in {name} - {}", project_name(s)),
                            clip(&format!("{sum} {content}"), 160),
                            m.ts_ms,
                        ));
                    }
                }
            }
            TriggerKind::Keyword => {
                let Some(p) = &c.pattern else { continue };
                for m in fresh.iter().filter(|m| !m.sidechain) {
                    for b in &m.blocks {
                        let text = match b {
                            Block::Text(t) if m.role == Role::Assistant => t,
                            Block::ToolResult { content, .. } => content,
                            _ => continue,
                        };
                        if let Some(f) = p.find(text) {
                            if c.ignore.iter().any(|i| i.is_match(text)) {
                                continue;
                            }
                            hits.push(hit(
                                r,
                                format!("Match \"{}\" - {}", clip(f.as_str(), 40), project_name(s)),
                                clip(text, 160),
                                m.ts_ms,
                            ));
                            break;
                        }
                    }
                }
            }
            TriggerKind::CostOver => {
                if totals.cost_usd >= r.threshold && st.fired.insert(r.id.clone()) {
                    hits.push(hit(
                        r,
                        format!("Cost over ${:.2} - {}", r.threshold, project_name(s)),
                        format!("Session cost is ${:.2}", totals.cost_usd),
                        s.end_ms,
                    ));
                }
            }
            TriggerKind::TokensOver => {
                if totals.usage.total() as f64 >= r.threshold && st.fired.insert(r.id.clone()) {
                    hits.push(hit(
                        r,
                        format!("Tokens over {} - {}", r.threshold as u64, project_name(s)),
                        format!("Session used {} tokens", totals.usage.total()),
                        s.end_ms,
                    ));
                }
            }
            TriggerKind::SessionEnded => {
                let idle = now_ms - last_activity_ms;
                if idle >= (r.threshold * 60_000.0) as i64
                    && !s.msgs.is_empty()
                    && st.fired.insert(r.id.clone())
                {
                    let first = s
                        .chunks
                        .first()
                        .and_then(|c| c.prompt.as_deref())
                        .unwrap_or("");
                    hits.push(hit(
                        r,
                        format!("Session ended - {}", project_name(s)),
                        clip(first, 120),
                        Some(last_activity_ms),
                    ));
                }
            }
        }
    }
    st.msgs = s.msgs.len();
    hits
}

/// Evaluate against a whole session without keeping state (the settings "Preview" / test).
pub fn test_rule(rule: &TriggerRule, s: &Session, now_ms: i64) -> Vec<TriggerHit> {
    let mut st = SeenState::default();
    let mut r = rule.clone();
    r.enabled = true;
    // For a preview the session counts as ended whatever the clock says.
    evaluate(&[r], s, &mut st, now_ms, s.end_ms.unwrap_or(0))
}

fn mtime_ms(p: &Path) -> Option<(i64, u64)> {
    let m = std::fs::metadata(p).ok()?;
    let t = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((t.as_millis() as i64, m.len()))
}

struct Tracked {
    state: SeenState,
    mtime: i64,
    len: u64,
    last_change_ms: i64,
    pending_end: bool,
}

/// Polls session files and reports trigger hits on data that appeared after it started.
pub struct TriggerWatcher {
    started_ms: i64,
    tracked: HashMap<String, Tracked>,
    /// Only files touched within this window are considered.
    pub window_ms: i64,
}

impl TriggerWatcher {
    pub fn new(started_ms: i64) -> Self {
        TriggerWatcher {
            started_ms,
            tracked: HashMap::new(),
            window_ms: 2 * 3_600_000,
        }
    }

    /// One poll (call off the UI thread, e.g. every 15 s). Cheap when nothing is enabled.
    pub fn poll(&mut self, roots: &Roots, rules: &[TriggerRule], now_ms: i64) -> Vec<TriggerHit> {
        if !rules.iter().any(|r| r.enabled) {
            return Vec::new();
        }
        let min_idle = rules
            .iter()
            .filter(|r| r.enabled && r.kind == TriggerKind::SessionEnded)
            .map(|r| (r.threshold * 60_000.0) as i64)
            .min();
        let mut out = Vec::new();
        for f in discover(roots) {
            let Some((mtime, len)) = mtime_ms(&f.path) else {
                continue;
            };
            if now_ms - mtime > self.window_ms {
                continue;
            }
            let key = f.path.to_string_lossy().into_owned();
            let changed;
            match self.tracked.get_mut(&key) {
                Some(t) => {
                    changed = t.mtime != mtime || t.len != len;
                    if changed {
                        t.mtime = mtime;
                        t.len = len;
                        t.last_change_ms = now_ms;
                        t.pending_end = true;
                        for r in rules.iter().filter(|r| r.kind == TriggerKind::SessionEnded) {
                            t.state.fired.remove(&r.id);
                        }
                    }
                    let due =
                        t.pending_end && min_idle.is_some_and(|m| now_ms - t.last_change_ms >= m);
                    if !changed && !due {
                        continue;
                    }
                }
                None => {
                    changed = true;
                    let old = mtime < self.started_ms;
                    self.tracked.insert(
                        key.clone(),
                        Tracked {
                            state: SeenState::default(),
                            mtime,
                            len,
                            last_change_ms: if old { mtime } else { now_ms },
                            pending_end: !old,
                        },
                    );
                    if old {
                        // History from before the watcher started: baseline without firing.
                        if let Ok(s) = load_session(&f.path, f.source) {
                            let t = self.tracked.get_mut(&key).expect("just inserted");
                            let _ = evaluate(rules, &s, &mut t.state, now_ms, mtime);
                            t.state.msgs = s.msgs.len();
                        }
                        continue;
                    }
                }
            }
            let Ok(s) = load_session(&f.path, f.source) else {
                continue;
            };
            let t = self.tracked.get_mut(&key).expect("tracked");
            let last = s.end_ms.unwrap_or(0).max(mtime);
            let hits = evaluate(rules, &s, &mut t.state, now_ms, last);
            if !changed
                || hits.iter().any(|h| {
                    rules
                        .iter()
                        .any(|r| r.id == h.rule_id && r.kind == TriggerKind::SessionEnded)
                })
            {
                t.pending_end = false;
            }
            out.extend(hits);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::chunks::{fixtures, parse_str};
    use super::super::paths::Source;
    use super::*;

    fn sess() -> Session {
        let msgs = parse_str(&fixtures::claude(), Source::Claude);
        Session::from_msgs(Path::new("/nonexistent/s1.jsonl"), Source::Claude, msgs)
    }

    fn rule(kind: TriggerKind) -> TriggerRule {
        TriggerRule {
            id: "r".into(),
            name: "R".into(),
            kind,
            ..TriggerRule::default()
        }
    }

    #[test]
    fn tool_error_with_scope_and_ignore() {
        let s = sess();
        let mut r = rule(TriggerKind::ToolError);
        let h = test_rule(&r, &s, 0);
        assert_eq!(h.len(), 1);
        assert!(h[0].title.contains("Error in Bash"));
        assert!(h[0].body.contains("cargo test"));
        r.tool_name = "Read".into();
        assert!(test_rule(&r, &s, 0).is_empty());
        r.tool_name = "Bash".into();
        r.ignore_patterns = vec!["test failed".into()];
        assert!(test_rule(&r, &s, 0).is_empty());
        r.ignore_patterns.clear();
        r.pattern = "no such".into();
        assert!(test_rule(&r, &s, 0).is_empty());
        r.repos = vec!["/elsewhere".into()];
        r.pattern.clear();
        assert!(test_rule(&r, &s, 0).is_empty(), "repo scope");
        r.repos = vec!["work/proj".into()];
        assert_eq!(test_rule(&r, &s, 0).len(), 1);
    }

    #[test]
    fn cost_keyword_ended_fire_once() {
        let s = sess();
        let mut cost = rule(TriggerKind::CostOver);
        cost.id = "c".into();
        cost.threshold = 0.001;
        let mut kw = rule(TriggerKind::Keyword);
        kw.id = "k".into();
        kw.pattern = "(?i)login".into();
        let mut end = rule(TriggerKind::SessionEnded);
        end.id = "e".into();
        end.threshold = 1.0;
        let rules = vec![cost, kw, end];
        let mut st = SeenState::default();
        let end_ms = s.end_ms.unwrap();
        let h = evaluate(&rules, &s, &mut st, end_ms + 30_000, end_ms);
        let ids: Vec<&str> = h.iter().map(|x| x.rule_id.as_str()).collect();
        assert!(ids.contains(&"c"));
        assert!(ids.contains(&"k"));
        assert!(!ids.contains(&"e"), "only 30s idle");
        let h = evaluate(&rules, &s, &mut st, end_ms + 120_000, end_ms);
        assert_eq!(
            h.iter().map(|x| x.rule_id.as_str()).collect::<Vec<_>>(),
            vec!["e"]
        );
        // nothing new, nothing fires again
        assert!(evaluate(&rules, &s, &mut st, end_ms + 999_000, end_ms).is_empty());
    }

    #[test]
    fn disabled_and_invalid_rules_are_inert() {
        let s = sess();
        let mut r = rule(TriggerKind::ToolError);
        r.enabled = false;
        let mut st = SeenState::default();
        assert!(evaluate(&[r.clone()], &s, &mut st, 0, 0).is_empty());
        r.enabled = true;
        r.pattern = "(".into();
        assert!(r.validate().is_err());
        assert!(evaluate(&[r], &s, &mut SeenState::default(), 0, 0).is_empty());
    }

    #[test]
    fn rules_persist_with_builtins_merged() {
        let d = tempfile::tempdir().unwrap();
        let mut rules = load_rules(d.path());
        assert_eq!(rules.len(), default_rules().len());
        assert!(rules.iter().all(|r| !r.enabled));
        rules[0].enabled = true;
        rules.push(TriggerRule::custom("mine", TriggerKind::Keyword));
        save_rules(d.path(), &rules).unwrap();
        let back = load_rules(d.path());
        assert!(back[0].enabled && back[0].builtin);
        assert_eq!(back.last().unwrap().name, "mine");
        assert_eq!(back.len(), rules.len());
    }

    #[test]
    fn watcher_ignores_history_and_reports_new_errors() {
        let d = tempfile::tempdir().unwrap();
        let roots = super::super::scanner::tests::make_roots(d.path());
        let mut r = rule(TriggerKind::ToolError);
        r.id = "te".into();
        let rules = vec![r];
        let now = 2_000_000_000_000i64; // far future: all fixtures are older than `started`
        let mut w = TriggerWatcher::new(now);
        w.window_ms = i64::MAX;
        assert!(w.poll(&roots, &rules, now).is_empty(), "baseline is silent");
        assert!(w.poll(&roots, &rules, now + 15_000).is_empty());
        // a new error line appended to the Claude session
        let p = roots
            .claude
            .as_ref()
            .unwrap()
            .join("projects/-work-proj/s1.jsonl");
        let mut raw = std::fs::read_to_string(&p).unwrap();
        raw.push_str("\n{\"type\":\"user\",\"timestamp\":\"2025-03-01T10:09:00Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t4\",\"content\":\"boom\",\"is_error\":true}]}}\n");
        std::fs::write(&p, raw).unwrap();
        let h = w.poll(&roots, &rules, now + 30_000);
        assert_eq!(h.len(), 1, "{h:?}");
        assert!(h[0].body.contains("boom"));
        assert!(w.poll(&roots, &rules, now + 45_000).is_empty());
        let _ = Source::Claude;
    }
}
