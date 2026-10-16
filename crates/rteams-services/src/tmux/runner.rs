//! tmux::runner: runs the install command only when asked, streaming its output. stdin is closed,
//! so a password prompt fails fast instead of hanging (the UI then points at the terminal).

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::thread;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallEvent {
    Line(String),
    /// Process ended; `Ok(())` on exit status 0.
    Done(Result<(), String>),
}

pub struct InstallRun {
    pub rx: Receiver<InstallEvent>,
}

impl InstallRun {
    /// Drain pending events.
    pub fn poll(&self) -> Vec<InstallEvent> {
        self.rx.try_iter().collect()
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
    let mut child = Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {prog}: {e}"))?;
    let (tx, rx) = channel();
    let h1 = child.stdout.take().map(|o| pump(o, tx.clone()));
    let h2 = child.stderr.take().map(|o| pump(o, tx.clone()));
    thread::spawn(move || {
        let st = child.wait();
        for h in [h1, h2].into_iter().flatten() {
            let _ = h.join();
        }
        let res = match st {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(format!("exited with {s}")),
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(InstallEvent::Done(res));
    });
    Ok(InstallRun { rx })
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

    #[test]
    fn missing_program_errors() {
        assert!(run(&["definitely-not-a-binary-xyz".into()]).is_err());
        assert!(run(&[]).is_err());
    }
}
