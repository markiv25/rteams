//! The usage ledger: one record per request/run, derived from our recorded runs
//! (`<home>/<team>/runs/<member>/<unix_ms>.jsonl`, Claude stream-json) plus the optional Claude
//! and Codex session importers. Files are parsed once per (mtime, len) and cached in
//! `<home>/usage-cache.json`.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{claude_import, codex_import, pricing};
use crate::{atomic, paths::AppPaths};

/// Files larger than this are skipped (the original caps imports at 64 MiB too).
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
const CACHE_VERSION: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    App,
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Cost and tokens reported by the CLI's own result event.
    SdkExact,
    /// Tokens parsed from logs, cost from the pricing table.
    LogParsed,
    /// Tokens known, cost estimated from the pricing table.
    CostEstimated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Billing {
    Api,
    Subscription,
    Free,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: String,
    pub source: Source,
    /// Team directory name; empty for sessions not started by rteams (shown as "Anonymous runs").
    pub team: String,
    pub member: String,
    /// `claude` or `codex`.
    pub runtime: String,
    pub model: String,
    /// App runs: `team/member/ms`; imports: session id.
    pub run: String,
    pub session: String,
    pub ts_ms: u64,
    pub input: u64,
    pub output: u64,
    pub cache_creation: u64,
    pub cache_read: u64,
    pub reasoning: u64,
    pub cost_usd: f64,
    pub kind: SourceKind,
    pub billing: Billing,
}

impl UsageRecord {
    /// Total tokens; cache tokens are included only when asked for.
    pub fn total(&self, include_cache: bool) -> u64 {
        let base = self.input + self.output;
        if include_cache {
            base + self.cache_creation + self.cache_read
        } else {
            base
        }
    }

    pub fn cache(&self) -> u64 {
        self.cache_creation + self.cache_read
    }
}

pub fn record_id(run: &str, key: &str) -> String {
    let h = Sha256::digest(format!("{run}\0{key}").as_bytes());
    h.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ledger {
    pub records: Vec<UsageRecord>,
    /// Files that could not be read or exceeded the size cap.
    pub errors: usize,
    pub scanned_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    mtime_ms: u64,
    len: u64,
    records: Vec<UsageRecord>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    schema_version: u64,
    #[serde(default)]
    files: BTreeMap<String, CacheEntry>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn start_ms(path: &Path) -> u64 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.split('-').next())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Records of one recorded run file. `team`/`member` come from the directory layout.
pub fn parse_app_run(raw: &str, team: &str, member: &str, ms: u64) -> Vec<UsageRecord> {
    let run = format!("{team}/{member}/{ms}");
    let mut model = String::new();
    let mut out: Vec<UsageRecord> = Vec::new();
    // Fallback when no result event exists: assistant message usage, last write per message id.
    let mut assistant: BTreeMap<String, Value> = BTreeMap::new();
    let mut session = String::new();
    for (i, line) in raw.lines().enumerate() {
        let Ok(v) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        if let Some(s) = v.get("session_id").and_then(Value::as_str) {
            session = s.to_string();
        }
        match v.get("type").and_then(Value::as_str) {
            Some("system") => {
                if let Some(m) = v.get("model").and_then(Value::as_str) {
                    model = m.to_string();
                }
            }
            Some("assistant") => {
                if let Some(m) = v.pointer("/message/model").and_then(Value::as_str) {
                    if model.is_empty() {
                        model = m.to_string();
                    }
                }
                if let Some(u) = v.pointer("/message/usage") {
                    let id = v
                        .pointer("/message/id")
                        .and_then(Value::as_str)
                        .map(String::from)
                        .unwrap_or_else(|| format!("line{i}"));
                    assistant.insert(id, u.clone());
                }
            }
            Some("result") => {
                let n = |p: &str| v.pointer(p).and_then(Value::as_u64).unwrap_or(0);
                let model_name = if model.is_empty() {
                    v.get("modelUsage")
                        .and_then(Value::as_object)
                        .and_then(|o| o.keys().next().cloned())
                        .unwrap_or_default()
                } else {
                    model.clone()
                };
                let (input, output) = (n("/usage/input_tokens"), n("/usage/output_tokens"));
                let (cc, cr) = (
                    n("/usage/cache_creation_input_tokens"),
                    n("/usage/cache_read_input_tokens"),
                );
                let reported = v.get("total_cost_usd").and_then(Value::as_f64);
                let cost_usd =
                    reported.unwrap_or_else(|| pricing::cost(&model_name, input, output, cr, cc));
                out.push(UsageRecord {
                    id: record_id(&run, &format!("result{i}")),
                    source: Source::App,
                    team: team.into(),
                    member: member.into(),
                    runtime: "claude".into(),
                    model: model_name,
                    run: run.clone(),
                    session: v
                        .get("session_id")
                        .and_then(Value::as_str)
                        .unwrap_or(&session)
                        .to_string(),
                    ts_ms: ms,
                    input,
                    output,
                    cache_creation: cc,
                    cache_read: cr,
                    reasoning: 0,
                    cost_usd,
                    kind: if reported.is_some() {
                        SourceKind::SdkExact
                    } else {
                        SourceKind::CostEstimated
                    },
                    billing: Billing::Unknown,
                });
            }
            _ => {}
        }
    }
    if out.is_empty() && !assistant.is_empty() {
        let g = |u: &Value, k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let (mut input, mut output, mut cc, mut cr) = (0, 0, 0, 0);
        for u in assistant.values() {
            input += g(u, "input_tokens");
            output += g(u, "output_tokens");
            cc += g(u, "cache_creation_input_tokens");
            cr += g(u, "cache_read_input_tokens");
        }
        out.push(UsageRecord {
            id: record_id(&run, "assistant"),
            source: Source::App,
            team: team.into(),
            member: member.into(),
            runtime: "claude".into(),
            cost_usd: pricing::cost(&model, input, output, cr, cc),
            model,
            run,
            session,
            ts_ms: ms,
            input,
            output,
            cache_creation: cc,
            cache_read: cr,
            reasoning: 0,
            kind: SourceKind::LogParsed,
            billing: Billing::Unknown,
        });
    }
    out
}

/// What to scan. `None` dirs disable that importer.
#[derive(Debug, Clone)]
pub struct Scanner {
    pub home: PathBuf,
    pub claude_dir: Option<PathBuf>,
    pub codex_dir: Option<PathBuf>,
}

impl Scanner {
    /// Home plus the real `~/.claude/projects` and `~/.codex/sessions`.
    pub fn with_default_importers(home: impl Into<PathBuf>) -> Self {
        let h = dirs::home_dir();
        Scanner {
            home: home.into(),
            claude_dir: h.as_ref().map(|h| h.join(".claude").join("projects")),
            codex_dir: h.as_ref().map(|h| h.join(".codex").join("sessions")),
        }
    }

    pub fn app_only(home: impl Into<PathBuf>) -> Self {
        Scanner {
            home: home.into(),
            claude_dir: None,
            codex_dir: None,
        }
    }

    fn cache_path(&self) -> PathBuf {
        AppPaths::new(&self.home).app("usage-cache.json")
    }

    /// Scan everything, reusing cached per-file results whose mtime and length are unchanged.
    pub fn scan(&self) -> Ledger {
        let path = self.cache_path();
        let mut cache: Cache = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .filter(|c: &Cache| c.schema_version == CACHE_VERSION)
            .unwrap_or_default();
        let mut errors = 0;
        let mut seen: HashSet<String> = HashSet::new();
        let mut app = Vec::new();
        let mut imported = Vec::new();

        let mut jobs: Vec<(PathBuf, Source, String, String, u64)> = Vec::new();
        for team in subdirs(&self.home) {
            let tname = file_name(&team);
            for member in subdirs(&team.join("runs")) {
                for f in jsonl_in(&member) {
                    let ms = start_ms(&f);
                    jobs.push((f, Source::App, tname.clone(), file_name(&member), ms));
                }
            }
        }
        if let Some(d) = &self.claude_dir {
            for f in walk_jsonl(d) {
                jobs.push((f, Source::Claude, String::new(), String::new(), 0));
            }
        }
        if let Some(d) = &self.codex_dir {
            for f in walk_jsonl(d) {
                jobs.push((f, Source::Codex, String::new(), String::new(), 0));
            }
        }

        let mut dirty = false;
        for (f, source, team, member, ms) in jobs {
            let key = f.to_string_lossy().into_owned();
            seen.insert(key.clone());
            let Ok(meta) = std::fs::metadata(&f) else {
                errors += 1;
                continue;
            };
            if meta.len() > MAX_FILE_BYTES {
                errors += 1;
                continue;
            }
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let records = match cache.files.get(&key) {
                Some(e) if e.mtime_ms == mtime_ms && e.len == meta.len() => e.records.clone(),
                _ => {
                    let Ok(raw) = std::fs::read(&f) else {
                        errors += 1;
                        continue;
                    };
                    let raw = String::from_utf8_lossy(&raw);
                    let recs = match source {
                        Source::App => parse_app_run(&raw, &team, &member, ms),
                        Source::Claude => claude_import::parse_session(&raw, &f),
                        Source::Codex => codex_import::parse_rollout(&raw, &f),
                    };
                    cache.files.insert(
                        key.clone(),
                        CacheEntry {
                            mtime_ms,
                            len: meta.len(),
                            records: recs.clone(),
                        },
                    );
                    dirty = true;
                    recs
                }
            };
            if source == Source::App {
                app.extend(records);
            } else {
                imported.extend(records);
            }
        }
        let before = cache.files.len();
        cache.files.retain(|k, _| seen.contains(k));
        dirty |= cache.files.len() != before;
        if dirty {
            cache.schema_version = CACHE_VERSION;
            if let Ok(b) = serde_json::to_vec(&cache) {
                let _ = atomic::write(&path, &b);
            }
        }
        Ledger {
            records: merge(app, imported),
            errors,
            scanned_at_ms: now_ms(),
        }
    }
}

/// App runs win: imported sessions already recorded as app runs (same session id) are dropped,
/// and records are deduped by id and sorted by time.
pub fn merge(app: Vec<UsageRecord>, imported: Vec<UsageRecord>) -> Vec<UsageRecord> {
    let sessions: HashSet<String> = app
        .iter()
        .filter(|r| !r.session.is_empty())
        .map(|r| r.session.clone())
        .collect();
    let mut ids = HashSet::new();
    let mut all: Vec<UsageRecord> = app
        .into_iter()
        .chain(
            imported
                .into_iter()
                .filter(|r| !sessions.contains(&r.session)),
        )
        .filter(|r| ids.insert(r.id.clone()))
        .collect();
    all.sort_by(|a, b| (a.ts_ms, &a.id).cmp(&(b.ts_ms, &b.id)));
    all
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn jsonl_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    v.sort();
    v
}

/// All `*.jsonl` under `dir` (depth-capped, symlinks not followed).
pub fn walk_jsonl(dir: &Path) -> Vec<PathBuf> {
    fn go(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if depth > 8 {
            return;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let p = e.path();
            if ft.is_dir() {
                go(&p, depth + 1, out);
            } else if ft.is_file() && p.extension().is_some_and(|x| x == "jsonl") {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    go(dir, 0, &mut v);
    v.sort();
    v
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    pub fn rec(
        team: &str,
        member: &str,
        model: &str,
        ts_ms: u64,
        input: u64,
        output: u64,
        cost: f64,
    ) -> UsageRecord {
        UsageRecord {
            id: record_id(&format!("{team}/{member}/{ts_ms}"), model),
            source: Source::App,
            team: team.into(),
            member: member.into(),
            runtime: "claude".into(),
            model: model.into(),
            run: format!("{team}/{member}/{ts_ms}"),
            session: String::new(),
            ts_ms,
            input,
            output,
            cache_creation: 0,
            cache_read: 0,
            reasoning: 0,
            cost_usd: cost,
            kind: SourceKind::SdkExact,
            billing: Billing::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = concat!(
        r#"{"type":"system","subtype":"init","model":"claude-sonnet-4-5","session_id":"s1"}"#,
        "\n",
        r#"{"type":"assistant","message":{"id":"m1","content":[]}}"#,
        "\n",
        "not json\n",
        r#"{"type":"result","session_id":"s1","total_cost_usd":0.25,"usage":{"input_tokens":10,"output_tokens":20,"cache_creation_input_tokens":3,"cache_read_input_tokens":4}}"#,
        "\n",
    );

    #[test]
    fn parses_result_event() {
        let r = parse_app_run(RUN, "t", "ada", 1000);
        assert_eq!(r.len(), 1);
        let r = &r[0];
        assert_eq!(
            (r.input, r.output, r.cache_creation, r.cache_read),
            (10, 20, 3, 4)
        );
        assert_eq!(r.model, "claude-sonnet-4-5");
        assert_eq!(r.cost_usd, 0.25);
        assert_eq!(r.run, "t/ada/1000");
        assert_eq!(r.total(true), 37);
        assert_eq!(r.total(false), 30);
    }

    #[test]
    fn falls_back_to_assistant_usage_and_prices_it() {
        let raw = concat!(
            r#"{"type":"assistant","message":{"id":"a","model":"claude-sonnet-4-5","usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
            "\n",
            r#"{"type":"assistant","message":{"id":"a","model":"claude-sonnet-4-5","usage":{"input_tokens":1000000,"output_tokens":0}}}"#,
            "\n",
        );
        let r = parse_app_run(raw, "t", "m", 5);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].input, 1_000_000);
        assert!((r[0].cost_usd - 3.0).abs() < 1e-9);
        assert_eq!(r[0].kind, SourceKind::LogParsed);
    }

    #[test]
    fn scan_reads_run_files_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let runs = dir.path().join("t/runs/ada");
        std::fs::create_dir_all(&runs).unwrap();
        std::fs::write(runs.join("1700000000000.jsonl"), RUN).unwrap();
        std::fs::create_dir_all(dir.path().join("t/runs/empty")).unwrap();
        let sc = Scanner::app_only(dir.path());
        let l = sc.scan();
        assert_eq!(l.records.len(), 1);
        assert_eq!(l.records[0].team, "t");
        assert_eq!(l.records[0].member, "ada");
        assert_eq!(l.records[0].ts_ms, 1_700_000_000_000);
        assert!(dir.path().join("usage-cache.json").exists());
        // Poison the cached records: an unchanged file must be served from the cache.
        let cp = dir.path().join("usage-cache.json");
        let s = std::fs::read_to_string(&cp)
            .unwrap()
            .replace("\"input\":10", "\"input\":99");
        std::fs::write(&cp, s).unwrap();
        assert_eq!(sc.scan().records[0].input, 99);
        // Changing the file invalidates the entry; removing it drops the record.
        std::fs::write(runs.join("1700000000000.jsonl"), format!("{RUN}\n")).unwrap();
        assert_eq!(sc.scan().records[0].input, 10);
        std::fs::remove_file(runs.join("1700000000000.jsonl")).unwrap();
        assert!(sc.scan().records.is_empty());
    }

    #[test]
    fn merge_drops_imports_already_recorded_as_app_runs() {
        let mut a = testutil::rec("t", "m", "x", 1, 1, 1, 0.0);
        a.session = "s1".into();
        let mut i = testutil::rec("", "", "x", 2, 5, 5, 0.0);
        i.source = Source::Claude;
        i.session = "s1".into();
        i.id = "other".into();
        let mut j = i.clone();
        j.session = "s2".into();
        j.id = "other2".into();
        let m = merge(vec![a], vec![i, j]);
        assert_eq!(m.len(), 2);
        assert!(m.iter().any(|r| r.session == "s2"));
    }
}
