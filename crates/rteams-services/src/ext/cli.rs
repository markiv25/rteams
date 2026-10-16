//! Running the `claude` CLI (and any other binary) with a timeout, plus binary discovery.
//! Extension changes that the CLI owns (plugins) go through here; MCP config is edited directly
//! by `mcp_config`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Result of one CLI run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CliOutput {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CliOutput {
    /// Best human message for a failure: stderr, else stdout, else the exit code.
    pub fn error_text(&self) -> String {
        if self.timed_out {
            return "timed out".into();
        }
        let e = self.stderr.trim();
        if !e.is_empty() {
            return e.to_string();
        }
        let o = self.stdout.trim();
        if !o.is_empty() {
            return o.to_string();
        }
        format!("exit {}", self.code.map_or("?".into(), |c| c.to_string()))
    }
}

fn is_executable(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    if !md.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        md.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Directories where CLI installers commonly put binaries (after `PATH`).
pub fn extra_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = home {
        for d in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".cargo/bin",
        ] {
            v.push(h.join(d));
        }
    }
    v.push(PathBuf::from("/opt/homebrew/bin"));
    v.push(PathBuf::from("/usr/local/bin"));
    v
}

/// First executable called `name` in `dirs`.
pub fn which_in(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let names: Vec<String> = if cfg!(windows) {
        vec![
            format!("{name}.exe"),
            format!("{name}.cmd"),
            name.to_string(),
        ]
    } else {
        vec![name.to_string()]
    };
    dirs.iter()
        .flat_map(|d| names.iter().map(move |n| d.join(n)))
        .find(|p| is_executable(p))
}

/// Find `name` on `PATH`, then in the usual install dirs.
pub fn which(name: &str) -> Option<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(extra_dirs(dirs::home_dir().as_deref()));
    which_in(name, &dirs)
}

/// The `claude` binary, if installed.
pub fn find_claude() -> Option<PathBuf> {
    which("claude")
}

type Reader = Box<dyn Read + Send>;

fn drain(r: Option<Reader>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(mut r) = r {
            let _ = r.read_to_end(&mut b);
        }
        let _ = tx.send(b);
    });
    rx
}

fn grab(rx: mpsc::Receiver<Vec<u8>>) -> String {
    String::from_utf8_lossy(&rx.recv_timeout(Duration::from_secs(2)).unwrap_or_default())
        .into_owned()
}

/// Run `bin args` (stdin closed) in `cwd` with extra `env`, killing it after `timeout`.
pub fn run(
    bin: &Path,
    args: &[String],
    cwd: Option<&Path>,
    env: &[(String, String)],
    timeout: Duration,
) -> CliOutput {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CliOutput {
                stderr: format!("cannot start {}: {e}", bin.display()),
                ..Default::default()
            }
        }
    };
    let out_rx = drain(child.stdout.take().map(|s| Box::new(s) as Reader));
    let err_rx = drain(child.stderr.take().map(|s| Box::new(s) as Reader));
    let start = Instant::now();
    let (status, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(st)) => break (Some(st), false),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break (None, true);
            }
        }
    };
    CliOutput {
        ok: status.is_some_and(|s| s.success()),
        code: status.and_then(|s| s.code()),
        stdout: grab(out_rx),
        stderr: grab(err_rx),
        timed_out,
    }
}

/// `run` taking `&str` args.
pub fn run_str(
    bin: &Path,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(String, String)],
    timeout: Duration,
) -> CliOutput {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    run(bin, &a, cwd, env, timeout)
}

/// Runtime availability as the Extensions banner shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeState {
    Missing,
    FailedToStart(String),
    Ready {
        path: PathBuf,
        version: Option<String>,
    },
}

/// Probe `claude --version`.
pub fn probe_claude() -> RuntimeState {
    let Some(bin) = find_claude() else {
        return RuntimeState::Missing;
    };
    let out = run_str(&bin, &["--version"], None, &[], Duration::from_secs(5));
    if out.ok {
        RuntimeState::Ready {
            path: bin,
            version: out
                .stdout
                .lines()
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string),
        }
    } else {
        RuntimeState::FailedToStart(out.error_text())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn runs_and_captures() {
        let d = tempfile::tempdir().unwrap();
        let s = script(d.path(), "t.sh", "echo out; echo err >&2; exit 3");
        let o = run_str(&s, &[], None, &[], Duration::from_secs(5));
        assert!(!o.ok);
        assert_eq!(o.code, Some(3));
        assert_eq!(o.stdout.trim(), "out");
        assert_eq!(o.error_text(), "err");
    }

    #[test]
    fn times_out_and_kills() {
        let d = tempfile::tempdir().unwrap();
        let s = script(d.path(), "slow.sh", "sleep 5");
        let t = Instant::now();
        let o = run_str(&s, &[], None, &[], Duration::from_millis(200));
        assert!(o.timed_out && !o.ok);
        assert!(t.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn env_is_passed_and_which_finds() {
        let d = tempfile::tempdir().unwrap();
        let s = script(d.path(), "e.sh", "echo $FOO_X");
        let env = [("FOO_X".to_string(), "bar".to_string())];
        let o = run_str(&s, &[], None, &env, Duration::from_secs(5));
        assert_eq!(o.stdout.trim(), "bar");
        assert_eq!(which_in("e.sh", &[d.path().to_path_buf()]), Some(s));
        assert!(which_in("nope.sh", &[d.path().to_path_buf()]).is_none());
    }
}
