//! Launch diagnostics: why an agent failed to start or died at once (a pure classifier over its
//! output tail), the per-launch record the team page renders, and the git readiness check that
//! gates worktree members.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Why a launch failed, as far as the output and environment say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    CliNotFound,
    NotSignedIn,
    InvalidModel,
    UnknownFlag,
    McpFailed,
    WorkdirMissing,
    RateLimit,
    Network,
    WorktreeNotReady,
    Unknown,
}

impl Cause {
    /// Locale key of the human message.
    pub fn key(self) -> &'static str {
        match self {
            Cause::CliNotFound => "rteams.launchDiag.cause.cliNotFound",
            Cause::NotSignedIn => "rteams.launchDiag.cause.notSignedIn",
            Cause::InvalidModel => "rteams.launchDiag.cause.invalidModel",
            Cause::UnknownFlag => "rteams.launchDiag.cause.unknownFlag",
            Cause::McpFailed => "rteams.launchDiag.cause.mcpFailed",
            Cause::WorkdirMissing => "rteams.launchDiag.cause.workdirMissing",
            Cause::RateLimit => "rteams.launchDiag.cause.rateLimit",
            Cause::Network => "rteams.launchDiag.cause.network",
            Cause::WorktreeNotReady => "rteams.launchDiag.cause.worktreeNotReady",
            Cause::Unknown => "rteams.launchDiag.cause.unknown",
        }
    }

    /// The suggested fix for this cause.
    pub fn fix(self) -> Fix {
        match self {
            Cause::CliNotFound | Cause::NotSignedIn | Cause::InvalidModel => Fix::OpenProviders,
            Cause::UnknownFlag => Fix::OpenLaunchDialog,
            _ => Fix::Retry,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    OpenProviders,
    OpenLaunchDialog,
    Retry,
}

impl Fix {
    pub fn key(self) -> &'static str {
        match self {
            Fix::OpenProviders => "rteams.launchDiag.fix.providers",
            Fix::OpenLaunchDialog => "rteams.launchDiag.fix.launchDialog",
            Fix::Retry => "rteams.launchDiag.fix.retry",
        }
    }
}

/// What the launcher knows about the environment of a failed launch.
#[derive(Debug, Clone, Copy)]
pub struct DiagEnv {
    pub cli_found: bool,
    pub workdir_ok: bool,
}

impl Default for DiagEnv {
    fn default() -> Self {
        DiagEnv {
            cli_found: true,
            workdir_ok: true,
        }
    }
}

fn any(hay: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| hay.contains(n))
}

/// Classify a failed launch from the tail of its output (stdout summaries and stderr) and the
/// environment. Pure: no I/O.
pub fn classify(text: &str, env: DiagEnv) -> Cause {
    let t = text.to_lowercase();
    if !env.workdir_ok {
        return Cause::WorkdirMissing;
    }
    let cli_missing_text = any(
        &t,
        &[
            "command not found",
            "enoent: ",
            "spawn claude",
            "claude: not found",
        ],
    ) && any(&t, &["no such file", "not found", "enoent"]);
    if !env.cli_found || cli_missing_text {
        return Cause::CliNotFound;
    }
    if any(
        &t,
        &[
            "unknown option",
            "unknown argument",
            "unrecognized option",
            "unrecognized argument",
            "unknown flag",
            "invalid option",
            "unexpected argument",
            "error: unknown",
        ],
    ) {
        return Cause::UnknownFlag;
    }
    if t.contains("model")
        && any(
            &t,
            &[
                "invalid model",
                "unknown model",
                "issue with the selected model",
                "may not exist",
                "does not exist",
                "model not found",
                "not_found_error",
                "not have access to it",
                "is not available",
            ],
        )
    {
        return Cause::InvalidModel;
    }
    if any(
        &t,
        &[
            "not logged in",
            "please run /login",
            "invalid api key",
            "authentication_error",
            "authentication failed",
            "unauthorized",
            "oauth token has expired",
            "invalid x-api-key",
            "missing api key",
            "api key",
        ],
    ) {
        return Cause::NotSignedIn;
    }
    if any(
        &t,
        &[
            "rate limit",
            "rate_limit",
            "too many requests",
            "usage limit",
            "quota",
            "credit balance",
            "limit reached",
            "overloaded_error",
        ],
    ) {
        return Cause::RateLimit;
    }
    if t.contains("mcp")
        && any(
            &t,
            &[
                "failed",
                "error",
                "could not",
                "couldn't",
                "unable",
                "not start",
                "spawn",
                "timed out",
            ],
        )
    {
        return Cause::McpFailed;
    }
    if any(
        &t,
        &[
            "econnrefused",
            "enotfound",
            "etimedout",
            "econnreset",
            "getaddrinfo",
            "connection refused",
            "connection reset",
            "fetch failed",
            "network error",
            "network is unreachable",
            "unable to connect",
            "could not resolve host",
            "connection error",
        ],
    ) {
        return Cause::Network;
    }
    Cause::Unknown
}

/// Lines kept on a diagnostic card.
pub const TAIL: usize = 20;
/// An exit this soon after spawn (with next to no output) counts as a failed launch.
pub const QUICK_EXIT_MS: i64 = 5_000;

/// One failed launch, ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchDiag {
    pub cause: Cause,
    /// Exit code (`None`: killed or never spawned).
    pub code: Option<i32>,
    /// The last output line that looks like an error, if any.
    pub detail: Option<String>,
    /// The last `TAIL` log lines.
    pub tail: Vec<String>,
}

/// Decide whether a finished agent counts as a failed launch and why. `lived_ms` is the time
/// between spawn and exit; `events` the number of output events it produced.
pub fn diagnose_exit(
    code: Option<i32>,
    lived_ms: i64,
    events: usize,
    lines: &[String],
    env: DiagEnv,
) -> Option<LaunchDiag> {
    let nonzero = code != Some(0);
    let quick = lived_ms < QUICK_EXIT_MS && events <= 1;
    if !nonzero && !quick {
        return None;
    }
    let from = lines.len().saturating_sub(TAIL);
    let tail: Vec<String> = lines[from..].to_vec();
    let cause = classify(&tail.join("\n"), env);
    let detail = tail
        .iter()
        .rev()
        .find(|l| l.starts_with("error:") || l.starts_with("stderr:"))
        .cloned();
    Some(LaunchDiag {
        cause,
        code,
        detail,
        tail,
    })
}

/// Live and final facts about one launch.
#[derive(Debug, Clone)]
pub struct LaunchRecord {
    pub started_ms: i64,
    pub ended_ms: Option<i64>,
    /// Output events so far (shared with the reader thread).
    pub events: Arc<AtomicUsize>,
    pub diag: Option<LaunchDiag>,
    /// Never spawned (CLI missing, workdir missing, worktree not ready).
    pub rejected: bool,
}

impl LaunchRecord {
    pub fn new(started_ms: i64, events: Arc<AtomicUsize>) -> Self {
        LaunchRecord {
            started_ms,
            ended_ms: None,
            events,
            diag: None,
            rejected: false,
        }
    }
}

/// Spawn state of one member, for the member list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpawnState {
    /// Process started, no output yet.
    Spawning,
    Running,
    Exited(Option<i32>),
    Failed(Cause),
}

impl SpawnState {
    pub fn of(running: bool, rec: &LaunchRecord, code: Option<i32>) -> SpawnState {
        if let Some(d) = &rec.diag {
            return SpawnState::Failed(d.cause);
        }
        if running {
            if rec.events.load(Ordering::SeqCst) == 0 {
                SpawnState::Spawning
            } else {
                SpawnState::Running
            }
        } else {
            SpawnState::Exited(code)
        }
    }
}

type Registry = Mutex<std::collections::BTreeMap<(String, String), Arc<Mutex<LaunchRecord>>>>;
static REGISTRY: Registry = Mutex::new(std::collections::BTreeMap::new());

/// Publish the record of `(team, member)`'s latest launch for the team page.
pub fn publish(team: &str, member: &str, rec: Arc<Mutex<LaunchRecord>>) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.insert((team.to_string(), member.to_string()), rec);
    }
}

/// Snapshot of the latest launch record of `(team, member)`.
pub fn record(team: &str, member: &str) -> Option<LaunchRecord> {
    let r = REGISTRY.lock().ok()?;
    let rec = r.get(&(team.to_string(), member.to_string()))?;
    let g = rec.lock().unwrap_or_else(|e| e.into_inner());
    Some(g.clone())
}

// ---- worktree readiness -------------------------------------------------------------------

/// Whether the project can host per-member worktrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    NotRepo,
    NoCommit,
    Detached,
    /// With the checked-out branch.
    Ready(String),
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Is `path` a git repo with at least one commit and a checked-out branch?
pub fn readiness(path: &Path) -> Readiness {
    if git(path, &["rev-parse", "--is-inside-work-tree"]).is_err() {
        return Readiness::NotRepo;
    }
    if git(path, &["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
        return Readiness::NoCommit;
    }
    match git(path, &["symbolic-ref", "-q", "--short", "HEAD"]) {
        Ok(b) if !b.trim().is_empty() => Readiness::Ready(b.trim().to_string()),
        _ => Readiness::Detached,
    }
}

impl Readiness {
    /// Why a worktree cannot be created yet; `None` when ready.
    pub fn blocker(&self) -> Option<&'static str> {
        match self {
            Readiness::Ready(_) => None,
            Readiness::NotRepo => Some("the project is not a git repository"),
            Readiness::NoCommit => Some("the project repository has no commit yet"),
            Readiness::Detached => Some("the project repository has a detached HEAD"),
        }
    }
}

/// `git init` in `path` (created when missing).
pub fn init_repo(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path).map_err(|e| format!("create {}: {e}", path.display()))?;
    git(path, &["init"]).map(|_| ())
}

/// Stage everything and make the first commit.
pub fn initial_commit(path: &Path) -> Result<(), String> {
    git(path, &["add", "-A"])?;
    git(
        path,
        &[
            "-c",
            "user.name=rteams",
            "-c",
            "user.email=rteams@localhost",
            "commit",
            "-m",
            "Initial commit",
            "--allow-empty",
        ],
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(text: &str) -> Cause {
        classify(text, DiagEnv::default())
    }

    #[test]
    fn classifies_sample_outputs() {
        assert_eq!(c("error: unknown option '--foo'"), Cause::UnknownFlag);
        assert_eq!(
            c("error: Invalid API key · Please run /login"),
            Cause::NotSignedIn
        );
        assert_eq!(c("Not logged in · Please run /login"), Cause::NotSignedIn);
        assert_eq!(
            c("There's an issue with the selected model (claude-foo). It may not exist or you may not have access to it."),
            Cause::InvalidModel
        );
        assert_eq!(
            c("stderr: MCP server \"rteams\" failed to start: spawn ENOENT"),
            Cause::McpFailed
        );
        assert_eq!(
            c("error: Claude AI usage limit reached|1760000000"),
            Cause::RateLimit
        );
        assert_eq!(c("error: 429 Too Many Requests"), Cause::RateLimit);
        assert_eq!(
            c("stderr: Error: connect ECONNREFUSED 127.0.0.1:443"),
            Cause::Network
        );
        assert_eq!(
            c("stderr: sh: claude: command not found"),
            Cause::CliNotFound
        );
        assert_eq!(c("something odd happened"), Cause::Unknown);
        assert_eq!(c(""), Cause::Unknown);
    }

    #[test]
    fn environment_wins_over_text() {
        let no_cli = DiagEnv {
            cli_found: false,
            workdir_ok: true,
        };
        assert_eq!(classify("anything", no_cli), Cause::CliNotFound);
        let no_dir = DiagEnv {
            cli_found: false,
            workdir_ok: false,
        };
        assert_eq!(
            classify("No such file or directory", no_dir),
            Cause::WorkdirMissing
        );
    }

    #[test]
    fn exit_is_a_failed_launch_only_when_nonzero_or_instant() {
        let lines: Vec<String> = (0..30).map(|i| format!("line {i}")).collect();
        assert!(diagnose_exit(Some(0), 60_000, 12, &lines, DiagEnv::default()).is_none());
        let d = diagnose_exit(Some(1), 60_000, 12, &lines, DiagEnv::default()).unwrap();
        assert_eq!(d.tail.len(), TAIL);
        assert_eq!(d.tail.last().unwrap(), "line 29");
        assert_eq!(d.cause, Cause::Unknown);
        let d = diagnose_exit(Some(0), 800, 0, &[], DiagEnv::default()).unwrap();
        assert_eq!(d.cause, Cause::Unknown);
        let lines = vec!["stderr: error: unknown option '--zzz'".to_string()];
        let d = diagnose_exit(Some(2), 100, 0, &lines, DiagEnv::default()).unwrap();
        assert_eq!(d.cause, Cause::UnknownFlag);
        assert_eq!(d.cause.fix(), Fix::OpenLaunchDialog);
        assert!(d.detail.unwrap().contains("--zzz"));
        assert_eq!(Cause::NotSignedIn.fix(), Fix::OpenProviders);
        assert_eq!(Cause::Network.fix(), Fix::Retry);
    }

    #[test]
    fn spawn_state_follows_record() {
        let events = Arc::new(AtomicUsize::new(0));
        let mut rec = LaunchRecord::new(0, events.clone());
        assert_eq!(SpawnState::of(true, &rec, None), SpawnState::Spawning);
        events.store(3, Ordering::SeqCst);
        assert_eq!(SpawnState::of(true, &rec, None), SpawnState::Running);
        assert_eq!(
            SpawnState::of(false, &rec, Some(0)),
            SpawnState::Exited(Some(0))
        );
        rec.diag = diagnose_exit(Some(1), 10, 0, &[], DiagEnv::default());
        assert_eq!(
            SpawnState::of(false, &rec, Some(1)),
            SpawnState::Failed(Cause::Unknown)
        );
    }

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn readiness_over_temp_repos() {
        let d = std::env::temp_dir().join(format!("rteams-ready-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(readiness(&d), Readiness::NotRepo);
        assert_eq!(readiness(&d.join("missing")), Readiness::NotRepo);
        init_repo(&d).unwrap();
        assert_eq!(readiness(&d), Readiness::NoCommit);
        assert!(Readiness::NoCommit.blocker().is_some());
        std::fs::write(d.join("a.txt"), "a").unwrap();
        initial_commit(&d).unwrap();
        assert!(matches!(readiness(&d), Readiness::Ready(_)));
        assert!(readiness(&d).blocker().is_none());
        sh(&d, &["checkout", "-q", "--detach", "HEAD"]);
        assert_eq!(readiness(&d), Readiness::Detached);
        let _ = std::fs::remove_dir_all(&d);
    }
}
