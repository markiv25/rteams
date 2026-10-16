//! Short previews of a member's latest activity (graph nodes, member dialog).

use super::{LogEntry, LogKind};

/// Entries shown in a preview.
pub const PREVIEW_LINES: usize = 5;

/// The last `n` meaningful entries: tool use / result, thinking, messages and errors (the
/// end-of-run summary is skipped).
pub fn last_entries(entries: &[LogEntry], n: usize) -> Vec<&LogEntry> {
    let mut v: Vec<&LogEntry> = entries
        .iter()
        .filter(|e| e.kind != LogKind::Result)
        .rev()
        .take(n)
        .collect();
    v.reverse();
    v
}

/// `"Tool use: Bash {...}"` style single lines.
pub fn preview_lines(entries: &[LogEntry], n: usize) -> Vec<String> {
    last_entries(entries, n)
        .into_iter()
        .map(|e| format!("{}: {}", e.kind.label(), super::clip(&e.text, 120)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(kind: LogKind, t: &str) -> LogEntry {
        LogEntry {
            ts_ms: 0,
            member: "a".into(),
            kind,
            text: t.into(),
        }
    }

    #[test]
    fn last_five_meaningful_in_order() {
        let mut v: Vec<LogEntry> = (0..8)
            .map(|i| e(LogKind::ToolUse, &format!("t{i}")))
            .collect();
        v.push(e(LogKind::Result, "done"));
        let p = preview_lines(&v, PREVIEW_LINES);
        assert_eq!(p.len(), 5);
        assert_eq!(p[0], "Tool use: t3");
        assert_eq!(p[4], "Tool use: t7");
        assert!(preview_lines(&[], 5).is_empty());
    }
}
