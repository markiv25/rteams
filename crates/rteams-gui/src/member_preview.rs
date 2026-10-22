//! member_preview: read-only peeks at a member's newest recorded run for the member dialog
//! (member-log-stream:getMemberLogPreviews / getMemberRuntimeLogTail). Never writes.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use rteams_services::logtail::preview::{preview_lines, PREVIEW_LINES};
use rteams_services::logtail::stream::{newest_run, MemberTail};

/// Bytes read from the end of the newest run for the raw tail.
const TAIL_BYTES: u64 = 16 * 1024;

fn runs_dir(home: &Path, team: &str, member: &str) -> std::path::PathBuf {
    home.join(team).join("runs").join(member)
}

/// The last few meaningful entries ("Tool use: Bash ...") of the member's newest run.
pub fn latest_activity(home: &Path, team: &str, member: &str) -> Vec<String> {
    let mut tail = MemberTail::new(member);
    tail.poll(&runs_dir(home, team, member), 0);
    let entries: Vec<_> = tail.entries.iter().cloned().collect();
    preview_lines(&entries, PREVIEW_LINES)
}

/// The last `n` raw lines of the newest run file (runtime log tail). Empty when none.
pub fn runtime_tail(home: &Path, team: &str, member: &str, n: usize) -> Vec<String> {
    let Some(path) = newest_run(&runs_dir(home, team, member)) else {
        return vec![];
    };
    let Ok(mut f) = std::fs::File::open(path) else {
        return vec![];
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(TAIL_BYTES);
    let mut buf = Vec::new();
    if f.seek(SeekFrom::Start(start)).is_err() || f.read_to_end(&mut buf).is_err() {
        return vec![];
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    if start > 0 && !lines.is_empty() {
        lines.remove(0); // starts mid-line
    }
    let from = lines.len().saturating_sub(n);
    lines[from..].iter().map(|l| l.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(home: &Path, ms: u64, body: &str) {
        let d = runs_dir(home, "t", "ada");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(format!("{ms}.jsonl")), body).unwrap();
    }

    #[test]
    fn previews_and_tail_read_newest_run_only() {
        let d = tempfile::tempdir().unwrap();
        let line = |t: &str| {
            format!("{{\"type\":\"assistant\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"{t}\"}}]}}}}\n")
        };
        write(d.path(), 1000, &line("old"));
        write(d.path(), 2000, &(line("one") + &line("two")));
        let p = latest_activity(d.path(), "t", "ada");
        assert_eq!(p, ["Message: one", "Message: two"]);
        let t = runtime_tail(d.path(), "t", "ada", 1);
        assert_eq!(t.len(), 1);
        assert!(t[0].contains("two"));
        assert!(latest_activity(d.path(), "t", "nobody").is_empty());
        assert!(runtime_tail(d.path(), "t", "nobody", 5).is_empty());
    }
}
