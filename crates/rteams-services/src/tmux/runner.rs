//! tmux::runner: runs the install command only when asked, streaming its output. stdin is closed,
//! so a password prompt fails fast instead of hanging (the UI then points at the terminal).

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallEvent {
    Line(String),
    /// Process ended; `Ok(())` on exit status 0.
    Done(Result<(), String>),
}

pub struct InstallRun {
    pub rx: Receiver<InstallEvent>,
    child: Arc<Mutex<Child>>,
    cancelled: Arc<AtomicBool>,
}

impl InstallRun {
    /// Drain pending events.
    pub fn poll(&self) -> Vec<InstallEvent> {
        self.rx.try_iter().collect()
    }

    /// Stop the installer (tmux:cancelInstall). The run then ends with `Done(Err("cancelled"))`.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
        }
    }
}

fn pump<R: Read + Send + 'static>(
    r: R,
    tx: std::sync::mpsc::Sender<InstallEvent>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(r).lines().map_while(Result::ok) {
            if tx.send(InstallEvent::Line(line)).is_err() {
                break;
            }
        }
    })
}

/// Execute `cmd` (program + args) and stream stdout/stderr lines. Call only from an explicit
/// user action.
pub fn run(cmd: &[String]) -> Result<InstallRun, String> {
    let (prog, args) = cmd.split_first().ok_or("empty command")?;
    let mut command = Command::new(prog);
    if let Some(p) = super::platform::child_path() {
        command.env("PATH", p);
    }
    // Homebrew must never wait for a keypress; sudo cannot prompt (stdin is closed).
    command.env("NONINTERACTIVE", "1");
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {prog}: {e}"))?;
    let (tx, rx) = channel();
    let h1 = child.stdout.take().map(|o| pump(o, tx.clone()));
    let h2 = child.stderr.take().map(|o| pump(o, tx.clone()));
    let child = Arc::new(Mutex::new(child));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (wc, wf) = (child.clone(), cancelled.clone());
    thread::spawn(move || {
        let st = loop {
            let polled = wc.lock().map(|mut c| c.try_wait());
            match polled {
                Ok(Ok(Some(s))) => break Ok(s),
                Ok(Ok(None)) => thread::sleep(std::time::Duration::from_millis(40)),
                Ok(Err(e)) => break Err(e),
                Err(_) => break Err(std::io::Error::other("lock poisoned")),
            }
        };
        for h in [h1, h2].into_iter().flatten() {
            let _ = h.join();
        }
        let res = if wf.load(Ordering::SeqCst) {
            Err("cancelled".to_string())
        } else {
            match st {
                Ok(s) if s.success() => Ok(()),
                Ok(s) => Err(format!("exited with {s}")),
                Err(e) => Err(e.to_string()),
            }
        };
        let _ = tx.send(InstallEvent::Done(res));
    });
    Ok(InstallRun {
        rx,
        child,
        cancelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn streams_output_and_status() {
        let run = run(&[
            "sh".into(),
            "-c".into(),
            "echo hi; echo err >&2; exit 3".into(),
        ])
        .unwrap();
        let mut evs = vec![];
        loop {
            match run
                .rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap()
            {
                InstallEvent::Done(r) => {
                    assert!(r.is_err());
                    break;
                }
                e => evs.push(e),
            }
        }
        assert!(evs.contains(&InstallEvent::Line("hi".into())));
        assert!(evs.contains(&InstallEvent::Line("err".into())));
    }

    #[cfg(unix)]
    #[test]
    fn cancel_stops_a_running_install() {
        let run = run(&["sleep".into(), "30".into()]).unwrap();
        run.cancel();
        let ev = run
            .rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        assert_eq!(ev, InstallEvent::Done(Err("cancelled".into())));
    }

    #[test]
    fn missing_program_errors() {
        assert!(run(&["definitely-not-a-binary-xyz".into()]).is_err());
        assert!(run(&[]).is_err());
    }
}
