//! In-app CLI installer: runs a runtime's own install command on a worker thread and streams its
//! output into shared state the Providers screen draws as a live log.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

/// Lines kept per install (the UI shows the tail).
const MAX_LINES: usize = 500;

#[derive(Debug, Clone, PartialEq)]
pub enum InstallState {
    Running { lines: Vec<String> },
    Done { ok: bool, lines: Vec<String> },
}

impl InstallState {
    pub fn lines(&self) -> &[String] {
        match self {
            InstallState::Running { lines } | InstallState::Done { lines, .. } => lines,
        }
    }
}

type States = Arc<Mutex<HashMap<String, InstallState>>>;
type Cancels = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

/// Per-runtime install states plus the cancel flags of running installs.
#[derive(Clone, Default)]
pub struct Installs {
    states: States,
    cancels: Cancels,
}

impl Installs {
    pub fn get(&self, id: &str) -> Option<InstallState> {
        self.states.lock().ok().and_then(|s| s.get(id).cloned())
    }

    pub fn is_running(&self, id: &str) -> bool {
        matches!(self.get(id), Some(InstallState::Running { .. }))
    }

    /// Record a failure without running anything (e.g. a missing prerequisite).
    pub fn fail(&self, id: &str, message: String) {
        if let Ok(mut s) = self.states.lock() {
            s.insert(
                id.to_string(),
                InstallState::Done {
                    ok: false,
                    lines: vec![message],
                },
            );
        }
    }

    /// Ask a running install to stop (kills the child's process group).
    pub fn cancel(&self, id: &str) {
        if let Some(flag) = self.cancels.lock().ok().and_then(|c| c.get(id).cloned()) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Run `cmd` (a runtime's own install command, never user text) on a worker thread.
    /// Returns false, doing nothing, when an install for `id` is already running.
    pub fn start(&self, ctx: &egui::Context, id: &str, cmd: &str) -> bool {
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let Ok(mut s) = self.states.lock() else {
                return false;
            };
            if matches!(s.get(id), Some(InstallState::Running { .. })) {
                return false;
            }
            s.insert(id.to_string(), InstallState::Running { lines: Vec::new() });
        }
        if let Ok(mut c) = self.cancels.lock() {
            c.insert(id.to_string(), cancel.clone());
        }
        let (states, cancels, ctx) = (self.states.clone(), self.cancels.clone(), ctx.clone());
        let (id, cmd) = (id.to_string(), cmd.to_string());
        std::thread::spawn(move || {
            let ok = run(&ctx, &states, &id, &cmd, &cancel);
            if let Ok(mut s) = states.lock() {
                let lines = s.remove(&id).map(|st| st.lines().to_vec());
                s.insert(
                    id.clone(),
                    InstallState::Done {
                        ok,
                        lines: lines.unwrap_or_default(),
                    },
                );
            }
            if let Ok(mut c) = cancels.lock() {
                c.remove(&id);
            }
            ctx.request_repaint();
        });
        true
    }
}

fn push(ctx: &egui::Context, states: &States, id: &str, line: String) {
    if let Ok(mut s) = states.lock() {
        if let Some(InstallState::Running { lines }) = s.get_mut(id) {
            lines.push(line);
            if lines.len() > MAX_LINES {
                let drop = lines.len() - MAX_LINES;
                lines.drain(..drop);
            }
        }
    }
    ctx.request_repaint();
}

fn pump(
    ctx: &egui::Context,
    states: &States,
    id: &str,
    r: impl std::io::Read + Send + 'static,
) -> std::thread::JoinHandle<()> {
    let (ctx, states, id) = (ctx.clone(), states.clone(), id.to_string());
    std::thread::spawn(move || {
        for line in BufReader::new(r).split(b'\n').map_while(Result::ok) {
            let text = String::from_utf8_lossy(&line);
            push(&ctx, &states, &id, text.trim_end_matches('\r').to_string());
        }
    })
}

#[cfg(unix)]
fn shell(cmd: &str) -> Command {
    use std::os::unix::process::CommandExt;
    let mut c = Command::new("sh");
    c.args(["-c", cmd]).process_group(0);
    if let Some(p) = install_path() {
        c.env("PATH", p);
    }
    c
}

/// PATH plus the usual tool dirs: an app started from Finder gets a minimal PATH, so
/// Homebrew/Volta/Bun installs of `npm` would otherwise be missed.
fn install_path() -> Option<std::ffi::OsString> {
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut extra = vec![
        std::path::PathBuf::from("/opt/homebrew/bin"),
        std::path::PathBuf::from("/usr/local/bin"),
    ];
    if let Some(h) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        for d in [".volta/bin", ".bun/bin", ".local/bin", ".npm-global/bin"] {
            extra.push(h.join(d));
        }
    }
    for d in extra {
        if d.is_dir() && !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    std::env::join_paths(dirs).ok()
}

#[cfg(windows)]
fn shell(cmd: &str) -> Command {
    let mut c = Command::new("cmd");
    c.args(["/C", cmd]);
    c
}

fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
}

/// Blocking run; streams output lines into `states`. True when the command exited 0.
fn run(ctx: &egui::Context, states: &States, id: &str, cmd: &str, cancel: &AtomicBool) -> bool {
    let spawned = shell(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            push(ctx, states, id, format!("failed to start: {e}"));
            return false;
        }
    };
    let readers: Vec<_> = [
        child.stdout.take().map(|o| pump(ctx, states, id, o)),
        child.stderr.take().map(|e| pump(ctx, states, id, e)),
    ]
    .into_iter()
    .flatten()
    .collect();
    loop {
        if cancel.load(Ordering::SeqCst) {
            kill_tree(&mut child);
            let _ = child.wait();
            // Readers end once the pipes close; don't block on stragglers.
            push(ctx, states, id, "cancelled".to_string());
            return false;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                for r in readers {
                    let _ = r.join();
                }
                return status.success();
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(40)),
            Err(e) => {
                push(ctx, states, id, format!("wait failed: {e}"));
                return false;
            }
        }
    }
}

/// The program an install command needs that is not on PATH (only `npm` is checked).
pub fn missing_tool(cmd: &str) -> Option<&'static str> {
    (cmd.split_whitespace().next() == Some("npm") && !on_path("npm")).then_some("npm")
}

fn on_path(name: &str) -> bool {
    #[cfg(unix)]
    let path = install_path();
    #[cfg(windows)]
    let path = std::env::var_os("PATH");
    let Some(path) = path else {
        return false;
    };
    std::env::split_paths(&path).any(|d| {
        d.join(name).is_file()
            || (cfg!(windows)
                && ["cmd", "exe", "bat"]
                    .iter()
                    .any(|e| d.join(format!("{name}.{e}")).is_file()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_done(i: &Installs, id: &str) -> InstallState {
        for _ in 0..300 {
            if let Some(s @ InstallState::Done { .. }) = i.get(id) {
                return s;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("install did not finish");
    }

    #[test]
    fn streams_lines_and_succeeds() {
        let i = Installs::default();
        let ctx = egui::Context::default();
        assert!(i.start(&ctx, "a", "echo one; echo two >&2; exit 0"));
        match wait_done(&i, "a") {
            InstallState::Done { ok, mut lines } => {
                assert!(ok);
                lines.sort();
                assert_eq!(lines, vec!["one", "two"]);
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn failure_is_reported() {
        let i = Installs::default();
        let ctx = egui::Context::default();
        assert!(i.start(&ctx, "b", "echo boom; exit 1"));
        assert_eq!(
            wait_done(&i, "b"),
            InstallState::Done {
                ok: false,
                lines: vec!["boom".into()]
            }
        );
    }

    #[test]
    fn refuses_second_install_and_can_cancel() {
        let i = Installs::default();
        let ctx = egui::Context::default();
        assert!(i.start(&ctx, "c", "echo start; sleep 30"));
        assert!(!i.start(&ctx, "c", "echo again"));
        assert!(i.is_running("c"));
        std::thread::sleep(std::time::Duration::from_millis(200));
        i.cancel("c");
        match wait_done(&i, "c") {
            InstallState::Done { ok, lines } => {
                assert!(!ok);
                assert!(lines.contains(&"cancelled".to_string()));
            }
            s => panic!("{s:?}"),
        }
        assert!(!i.is_running("c"));
    }

    #[test]
    fn only_npm_is_prechecked() {
        assert_eq!(missing_tool("brew install x"), None);
    }
}
