//! Session report: overview, tokens and cost, tool usage, files touched, git activity, errors,
//! friction and quality signals, subagents, an activity timeline, takeaways and badges.

use std::collections::BTreeMap;

use super::chunks::{Block, Role, Session, Usage};
use super::steps::StepKind;
use super::tools::{command_of, read_file, stats, written_file, ToolExec};

#[derive(Debug, Clone, PartialEq)]
pub struct Overview {
    pub id: String,
    pub source: &'static str,
    pub cwd: String,
    pub branch: String,
    pub model: String,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub messages: u64,
    pub turns: usize,
    pub tool_calls: usize,
    pub subagents: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelRow {
    pub model: String,
    pub usage: Usage,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TokenUsage {
    pub usage: Usage,
    pub total: u64,
    /// cache_read / (input + cache_read + cache_write), 0..1.
    pub cache_hit_ratio: f64,
    pub by_model: Vec<ModelRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CostAnalysis {
    pub total: f64,
    pub parent: f64,
    pub subagent: f64,
    pub per_commit: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRow {
    pub name: String,
    pub calls: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileTouch {
    pub path: String,
    pub edits: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitActivity {
    pub commits: u64,
    pub pushes: u64,
    pub checkouts: u64,
    pub pull_requests: u64,
    pub commands: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ErrorRow {
    pub tool: String,
    pub summary: String,
    pub message: String,
    pub ts_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Friction {
    /// A call repeated with the same input right after it failed.
    pub retries: u64,
    /// Three or more consecutive failed calls.
    pub failure_streaks: u64,
    pub interruptions: u64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Quality {
    pub test_runs: u64,
    pub test_failures: u64,
    pub build_runs: u64,
    pub build_failures: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubagentRow {
    pub agent_id: String,
    pub description: String,
    pub tool_calls: u64,
    pub tokens: u64,
    pub cost_usd: f64,
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub start_ms: i64,
    pub events: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Good,
    Warn,
    Bad,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    /// `cost`, `errors`, `friction` or `quality`.
    pub key: &'static str,
    pub level: Level,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub overview: Overview,
    pub tokens: TokenUsage,
    pub cost: CostAnalysis,
    pub tools: Vec<ToolRow>,
    pub files_edited: Vec<FileTouch>,
    pub files_read: Vec<String>,
    pub git: GitActivity,
    pub errors: Vec<ErrorRow>,
    pub friction: Friction,
    pub quality: Quality,
    pub subagents: Vec<SubagentRow>,
    pub timeline: Vec<Bucket>,
    pub takeaways: Vec<String>,
    pub assessments: Vec<Assessment>,
}

const TEST_MARKERS: &[&str] = &[
    "cargo test",
    "cargo nextest",
    "pytest",
    "npm test",
    "npm run test",
    "yarn test",
    "pnpm test",
    "go test",
    "jest",
    "vitest",
    "mvn test",
    "rspec",
    "gradle test",
];
const BUILD_MARKERS: &[&str] = &[
    "cargo build",
    "cargo check",
    "npm run build",
    "yarn build",
    "pnpm build",
    "tsc",
    "go build",
    "make",
    "gradle build",
];

fn segments(cmd: &str) -> impl Iterator<Item = &str> {
    cmd.split(['&', ';', '|', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn git_activity<'a>(tools: impl Iterator<Item = &'a ToolExec>) -> GitActivity {
    let mut g = GitActivity::default();
    for t in tools {
        if !matches!(t.name.as_str(), "Bash" | "shell" | "exec_command") {
            continue;
        }
        let Some(cmd) = command_of(&t.input) else {
            continue;
        };
        for seg in segments(&cmd) {
            let hit = if seg.starts_with("git commit") {
                g.commits += 1;
                true
            } else if seg.starts_with("git push") {
                g.pushes += 1;
                true
            } else if seg.starts_with("git checkout") || seg.starts_with("git switch") {
                g.checkouts += 1;
                true
            } else if seg.starts_with("gh pr create") {
                g.pull_requests += 1;
                true
            } else {
                false
            };
            if hit && g.commands.len() < 50 {
                g.commands.push(seg.chars().take(120).collect());
            }
        }
    }
    g
}

fn has_marker(cmd: &str, markers: &[&str]) -> bool {
    segments(cmd).any(|s| {
        markers
            .iter()
            .any(|m| s.starts_with(m) || s.contains(&format!(" {m}")))
    })
}

fn level_for(v: f64, good: f64, warn: f64) -> Level {
    if v <= good {
        Level::Good
    } else if v <= warn {
        Level::Warn
    } else {
        Level::Bad
    }
}

pub fn build(s: &Session) -> Report {
    let tools: Vec<&ToolExec> = s.tools().collect();
    let grand = s.grand_totals();
    let sub_cost: f64 = s.subagents.iter().map(|x| x.totals.cost_usd).sum();

    // tool stats include subagent calls
    let sub_tools: Vec<&ToolExec> = s
        .subagents
        .iter()
        .flat_map(|x| x.chunks.iter())
        .flat_map(|c| c.steps.iter())
        .filter_map(|st| st.tool.as_ref())
        .collect();
    let st = stats(tools.iter().copied().chain(sub_tools.iter().copied()));
    let mut tool_rows: Vec<ToolRow> = st
        .into_iter()
        .map(|(name, v)| ToolRow {
            name,
            calls: v.calls,
            errors: v.errors,
        })
        .collect();
    tool_rows.sort_by(|a, b| b.calls.cmp(&a.calls).then_with(|| a.name.cmp(&b.name)));

    let mut edits: BTreeMap<String, u64> = BTreeMap::new();
    let mut reads: BTreeMap<String, ()> = BTreeMap::new();
    for t in tools.iter().chain(sub_tools.iter()) {
        if let Some(p) = written_file(&t.name, &t.input) {
            *edits.entry(p).or_default() += 1;
        }
        if let Some(p) = read_file(&t.name, &t.input) {
            reads.insert(p, ());
        }
    }
    let mut files_edited: Vec<FileTouch> = edits
        .into_iter()
        .map(|(path, edits)| FileTouch { path, edits })
        .collect();
    files_edited.sort_by(|a, b| b.edits.cmp(&a.edits).then_with(|| a.path.cmp(&b.path)));
    let edited: std::collections::HashSet<&str> =
        files_edited.iter().map(|f| f.path.as_str()).collect();
    let files_read: Vec<String> = reads
        .into_keys()
        .filter(|p| !edited.contains(p.as_str()))
        .collect();

    let git = git_activity(tools.iter().copied());

    let errors: Vec<ErrorRow> = tools
        .iter()
        .filter(|t| t.is_error)
        .map(|t| ErrorRow {
            tool: t.name.clone(),
            summary: t.summary(),
            message: t
                .result
                .as_deref()
                .unwrap_or("")
                .trim()
                .chars()
                .take(300)
                .collect(),
            ts_ms: t.end_ms.or(t.start_ms),
        })
        .collect();

    // friction
    let mut fr = Friction::default();
    let mut streak = 0u64;
    let mut last_err: Option<(String, String)> = None;
    for t in &tools {
        let key = (t.name.clone(), t.input.to_string());
        if last_err.as_ref() == Some(&key) {
            fr.retries += 1;
        }
        if t.is_error {
            streak += 1;
            if streak == 3 {
                fr.failure_streaks += 1;
            }
            last_err = Some(key);
        } else {
            streak = 0;
            last_err = None;
        }
    }
    fr.interruptions = s
        .msgs
        .iter()
        .filter(|m| m.role == Role::User && !m.sidechain)
        .filter(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, Block::Text(t) if t.trim_start().starts_with("[Request interrupted")))
        })
        .count() as u64;

    // quality
    let mut q = Quality::default();
    for t in &tools {
        if !matches!(t.name.as_str(), "Bash" | "shell" | "exec_command") {
            continue;
        }
        let Some(cmd) = command_of(&t.input) else {
            continue;
        };
        if has_marker(&cmd, TEST_MARKERS) {
            q.test_runs += 1;
            q.test_failures += t.is_error as u64;
        } else if has_marker(&cmd, BUILD_MARKERS) {
            q.build_runs += 1;
            q.build_failures += t.is_error as u64;
        }
    }

    let subagents: Vec<SubagentRow> = s
        .subagents
        .iter()
        .map(|x| SubagentRow {
            agent_id: x.agent_id.clone(),
            description: if x.description.is_empty() {
                x.prompt.chars().take(60).collect()
            } else {
                x.description.clone()
            },
            tool_calls: x.tool_count,
            tokens: x.totals.usage.total(),
            cost_usd: x.totals.cost_usd,
            duration_ms: x.duration_ms(),
        })
        .collect();

    // timeline: step starts bucketed over the session
    let mut stamps: Vec<i64> = s
        .chunks
        .iter()
        .flat_map(|c| c.steps.iter())
        .filter_map(|st| st.start_ms)
        .collect();
    stamps.sort_unstable();
    let mut timeline = Vec::new();
    if let (Some(&a), Some(&b)) = (stamps.first(), stamps.last()) {
        let span = (b - a).max(1);
        let n = 60i64.min((span / 60_000).max(1) + 1);
        let width = (span / n).max(1) + 1;
        let mut buckets: Vec<Bucket> = (0..n)
            .map(|i| Bucket {
                start_ms: a + i * width,
                events: 0,
            })
            .collect();
        for t in stamps {
            let i = (((t - a) / width) as usize).min(buckets.len() - 1);
            buckets[i].events += 1;
        }
        timeline = buckets;
    }

    let by_model: Vec<ModelRow> = grand
        .by_model
        .iter()
        .map(|(m, v)| ModelRow {
            model: m.clone(),
            usage: v.usage,
            cost_usd: v.cost_usd,
        })
        .collect();
    let u = grand.usage;
    let denom = (u.input + u.cache_read + u.cache_write) as f64;
    let calls = tool_rows.iter().map(|r| r.calls).sum::<u64>();
    let errs = tool_rows.iter().map(|r| r.errors).sum::<u64>();
    let err_rate = if calls == 0 {
        0.0
    } else {
        errs as f64 / calls as f64
    };

    let mut takeaways = Vec::new();
    takeaways.push(format!(
        "{} turns, {} tool calls{}",
        s.chunks.len(),
        calls,
        if s.subagents.is_empty() {
            String::new()
        } else {
            format!(", {} subagents", s.subagents.len())
        }
    ));
    if let Some(top) = tool_rows.first() {
        takeaways.push(format!(
            "Most used tool: {} ({} calls)",
            top.name, top.calls
        ));
    }
    takeaways.push(format!(
        "Estimated cost ${:.2} across {} tokens",
        grand.cost_usd,
        u.total()
    ));
    if errs > 0 {
        takeaways.push(format!(
            "{errs} tool errors ({:.0}% of calls)",
            err_rate * 100.0
        ));
    }
    if !files_edited.is_empty() {
        takeaways.push(format!("Edited {} files", files_edited.len()));
    }
    if git.commits > 0 {
        takeaways.push(format!("{} commits", git.commits));
    }
    if fr.retries + fr.failure_streaks + fr.interruptions > 0 {
        takeaways.push(format!(
            "Friction: {} retries, {} failure streaks, {} interruptions",
            fr.retries, fr.failure_streaks, fr.interruptions
        ));
    }

    let mut assessments = vec![
        Assessment {
            key: "cost",
            level: level_for(grand.cost_usd, 1.0, 5.0),
            label: format!("${:.2}", grand.cost_usd),
        },
        Assessment {
            key: "errors",
            level: level_for(err_rate, 0.05, 0.15),
            label: format!("{:.0}% errors", err_rate * 100.0),
        },
        Assessment {
            key: "friction",
            level: level_for(
                (fr.retries + fr.failure_streaks + fr.interruptions) as f64,
                0.0,
                3.0,
            ),
            label: format!(
                "{} signals",
                fr.retries + fr.failure_streaks + fr.interruptions
            ),
        },
    ];
    assessments.push(if q.test_runs == 0 {
        Assessment {
            key: "quality",
            level: Level::Warn,
            label: "no tests run".into(),
        }
    } else {
        Assessment {
            key: "quality",
            level: level_for(q.test_failures as f64 / q.test_runs as f64, 0.0, 0.5),
            label: format!("{}/{} test runs failed", q.test_failures, q.test_runs),
        }
    });

    Report {
        overview: Overview {
            id: s.id.clone(),
            source: s.source.as_str(),
            cwd: s.cwd.clone().unwrap_or_default(),
            branch: s.branch.clone().unwrap_or_default(),
            model: s.model.clone(),
            start_ms: s.start_ms,
            end_ms: s.end_ms,
            duration_ms: s.duration_ms(),
            messages: s.message_count,
            turns: s.chunks.len(),
            tool_calls: tools.len(),
            subagents: s.subagents.len(),
        },
        tokens: TokenUsage {
            usage: u,
            total: u.total(),
            cache_hit_ratio: if denom > 0.0 {
                u.cache_read as f64 / denom
            } else {
                0.0
            },
            by_model,
        },
        cost: CostAnalysis {
            total: grand.cost_usd,
            parent: s.totals.cost_usd,
            subagent: sub_cost,
            per_commit: (git.commits > 0).then(|| grand.cost_usd / git.commits as f64),
        },
        tools: tool_rows,
        files_edited,
        files_read,
        git,
        errors,
        friction: fr,
        quality: q,
        subagents,
        timeline,
        takeaways,
        assessments,
    }
}

/// Plain-text copy of the report.
pub fn to_markdown(r: &Report) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    let _ = writeln!(o, "# Session report {}\n", r.overview.id);
    let _ = writeln!(
        o,
        "- Project: {}\n- Model: {}\n- Turns: {}, tool calls: {}, subagents: {}",
        r.overview.cwd,
        r.overview.model,
        r.overview.turns,
        r.overview.tool_calls,
        r.overview.subagents
    );
    let _ = writeln!(
        o,
        "\n## Cost\n\nTotal ${:.4} (parent ${:.4}, subagents ${:.4})",
        r.cost.total, r.cost.parent, r.cost.subagent
    );
    let u = r.tokens.usage;
    let _ = writeln!(
        o,
        "\n## Tokens\n\ninput {} / output {} / cache read {} / cache write {}",
        u.input, u.output, u.cache_read, u.cache_write
    );
    let _ = writeln!(o, "\n## Tools\n");
    for t in &r.tools {
        let _ = writeln!(o, "- {}: {} calls, {} errors", t.name, t.calls, t.errors);
    }
    let _ = writeln!(o, "\n## Key takeaways\n");
    for t in &r.takeaways {
        let _ = writeln!(o, "- {t}");
    }
    o
}

/// Number of steps that are subagent runs (for badges in the viewer).
pub fn subagent_steps(s: &Session) -> usize {
    s.chunks
        .iter()
        .flat_map(|c| c.steps.iter())
        .filter(|st| st.kind == StepKind::Subagent)
        .count()
}

#[cfg(test)]
mod tests {
    use super::super::chunks::{fixtures, parse_str, Session};
    use super::super::paths::Source;
    use super::*;
    use std::path::Path;

    fn session() -> Session {
        let msgs = parse_str(&fixtures::claude(), Source::Claude);
        Session::from_msgs(Path::new("/nonexistent/s1.jsonl"), Source::Claude, msgs)
    }

    #[test]
    fn report_numbers() {
        let r = build(&session());
        assert_eq!(r.overview.turns, 2);
        assert_eq!(r.overview.tool_calls, 4);
        assert_eq!(r.overview.duration_ms, Some(66_000));
        assert_eq!(r.tokens.usage.input, 310);
        assert_eq!(r.tokens.usage.output, 85);
        assert_eq!(r.tokens.total, 310 + 85 + 1000 + 50);
        assert!((r.tokens.cache_hit_ratio - 1000.0 / 1360.0).abs() < 1e-9);
        // sonnet-4-5: 3/15 per M, cache read 0.3, write 3.75
        let want = (310.0 * 3.0 + 85.0 * 15.0 + 1000.0 * 0.3 + 50.0 * 3.75) / 1e6;
        assert!(
            (r.cost.total - want).abs() < 1e-9,
            "{} vs {want}",
            r.cost.total
        );
        assert_eq!(r.cost.per_commit, Some(r.cost.total));
        let bash = r.tools.iter().find(|t| t.name == "Bash").unwrap();
        assert_eq!((bash.calls, bash.errors), (2, 1));
        assert_eq!(
            r.files_edited,
            vec![FileTouch {
                path: "/work/proj/src/auth.rs".into(),
                edits: 1
            }]
        );
        assert!(
            r.files_read.is_empty(),
            "edited files are not listed as read-only"
        );
        assert_eq!(r.git.commits, 1);
        assert_eq!(r.errors.len(), 1);
        assert_eq!(r.errors[0].message, "error: test failed");
        assert_eq!(r.quality.test_runs, 1);
        assert_eq!(r.quality.test_failures, 1);
        assert!(!r.timeline.is_empty());
        assert_eq!(r.timeline.iter().map(|b| b.events).sum::<u32>(), 7);
        assert!(r.takeaways.iter().any(|t| t.contains("tool errors")));
        let q = r.assessments.iter().find(|a| a.key == "quality").unwrap();
        assert_eq!(q.level, Level::Bad);
    }

    #[test]
    fn friction_detects_retry_and_streak() {
        let line = |id: &str, ts: u32| {
            format!(
                r#"{{"type":"assistant","timestamp":"2025-03-01T10:00:{ts:02}Z","message":{{"id":"m{id}","content":[{{"type":"tool_use","id":"{id}","name":"Bash","input":{{"command":"false"}}}}]}}}}
{{"type":"user","timestamp":"2025-03-01T10:00:{ts:02}Z","message":{{"role":"user","content":[{{"type":"tool_result","tool_use_id":"{id}","content":"x","is_error":true}}]}}}}"#
            )
        };
        let raw = format!(
            "{{\"type\":\"user\",\"timestamp\":\"2025-03-01T10:00:00Z\",\"message\":{{\"role\":\"user\",\"content\":\"go\"}}}}\n{}\n{}\n{}\n{{\"type\":\"user\",\"timestamp\":\"2025-03-01T10:00:30Z\",\"message\":{{\"role\":\"user\",\"content\":[{{\"type\":\"text\",\"text\":\"[Request interrupted by user]\"}}]}}}}",
            line("a", 1),
            line("b", 2),
            line("c", 3)
        );
        let msgs = parse_str(&raw, Source::Claude);
        let s = Session::from_msgs(Path::new("/x/s.jsonl"), Source::Claude, msgs);
        let r = build(&s);
        assert_eq!(r.friction.retries, 2);
        assert_eq!(r.friction.failure_streaks, 1);
        assert_eq!(r.friction.interruptions, 1);
    }

    #[test]
    fn markdown_export_mentions_totals() {
        let md = to_markdown(&build(&session()));
        assert!(md.contains("Session report s1"));
        assert!(md.contains("Bash: 2 calls, 1 errors"));
    }
}
