//! `GET /api/events`: Server-Sent Events. Tails every team's `events.jsonl` (or one team with
//! `?team=`) and pushes each newly appended event as
//! `data: {"team":"<t>","seq":<n>,"event":{...}}`. A comment line goes out every 15 s as keepalive.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rteams_core::event::Event;

use super::routes_teams::team_names;

const KEEPALIVE: Duration = Duration::from_secs(15);
const POLL: Duration = Duration::from_millis(150);
const MAX_READ: u64 = 4 * 1024 * 1024;

/// Follows the event logs by byte offset; returns only what was appended after it was created
/// (plus everything in teams that appear later).
pub struct Tailer {
    home: PathBuf,
    only: Option<String>,
    offsets: HashMap<String, u64>,
}

fn log_path(home: &Path, team: &str) -> PathBuf {
    home.join(team).join("events.jsonl")
}

impl Tailer {
    pub fn new(home: &Path, only: Option<&str>) -> Tailer {
        let mut t = Tailer {
            home: home.to_path_buf(),
            only: only.map(str::to_string),
            offsets: HashMap::new(),
        };
        for n in t.teams() {
            let len = std::fs::metadata(log_path(home, &n)).map_or(0, |m| m.len());
            t.offsets.insert(n, len);
        }
        t
    }

    fn teams(&self) -> Vec<String> {
        match &self.only {
            Some(t) => vec![t.clone()],
            None => team_names(&self.home),
        }
    }

    /// New `(team, event)` pairs since the last call.
    pub fn poll(&mut self) -> Vec<(String, Event)> {
        let mut out = Vec::new();
        for team in self.teams() {
            let path = log_path(&self.home, &team);
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let off = self.offsets.entry(team.clone()).or_insert(0);
            if meta.len() < *off {
                *off = meta.len(); // truncated / rewritten
            }
            if meta.len() == *off {
                continue;
            }
            let Ok(mut f) = std::fs::File::open(&path) else {
                continue;
            };
            if f.seek(SeekFrom::Start(*off)).is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if f.take(MAX_READ).read_to_end(&mut buf).is_err() {
                continue;
            }
            // Consume whole lines only; an unterminated tail is a write in flight.
            let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
                continue;
            };
            *off += end as u64 + 1;
            for line in buf[..end].split(|b| *b == b'\n') {
                if let Ok(ev) = serde_json::from_slice::<Event>(line) {
                    out.push((team.clone(), ev));
                }
            }
        }
        out
    }
}

/// Frame one SSE message.
pub fn frame(team: &str, ev: &Event) -> String {
    let data = serde_json::json!({ "team": team, "seq": ev.seq, "event": ev });
    format!("id: {team}:{}\nevent: event\ndata: {data}\n\n", ev.seq)
}

/// Stream until the client goes away or `stop` is set. `w` is the raw socket.
pub fn serve(
    mut w: Box<dyn Write + Send>,
    mut tailer: Tailer,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    w.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\n\
          X-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n: connected\n\n",
    )?;
    w.flush()?;
    let mut last = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        let batch = tailer.poll();
        for (team, ev) in &batch {
            w.write_all(frame(team, ev).as_bytes())?;
        }
        if !batch.is_empty() {
            w.flush()?;
            last = Instant::now();
        } else if last.elapsed() >= KEEPALIVE {
            w.write_all(b": keepalive\n\n")?;
            w.flush()?;
            last = Instant::now();
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}
