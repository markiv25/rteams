//! Merge per-member entries into one time-ordered stream.

use super::{LogEntry, LogKind};

/// Total entries kept in a merged view.
pub const MERGED_CAP: usize = 2000;

/// Time-ordered merge (stable per member), identical adjacent duplicates dropped, capped to
/// the newest `cap`.
pub fn merge(per_member: Vec<Vec<LogEntry>>, cap: usize) -> Vec<LogEntry> {
    let mut all: Vec<(usize, LogEntry)> = per_member
        .into_iter()
        .flat_map(|v| v.into_iter().enumerate())
        .collect();
    all.sort_by(|a, b| (a.1.ts_ms, &a.1.member, a.0).cmp(&(b.1.ts_ms, &b.1.member, b.0)));
    let mut out: Vec<LogEntry> = Vec::with_capacity(all.len());
    for (_, e) in all {
        if out.last().is_some_and(|l| l == &e) {
            continue;
        }
        out.push(e);
    }
    if out.len() > cap {
        out.drain(..out.len() - cap);
    }
    out
}

/// Filters of the log panel.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Members to show; empty = all.
    pub members: Vec<String>,
    /// Kinds to hide.
    pub hide: Vec<LogKind>,
    /// Case-insensitive substring; empty = none.
    pub query: String,
}

impl Filter {
    pub fn matches(&self, e: &LogEntry) -> bool {
        (self.members.is_empty() || self.members.contains(&e.member))
            && !self.hide.contains(&e.kind)
            && (self.query.is_empty()
                || e.text.to_lowercase().contains(&self.query.to_lowercase())
                || e.member.to_lowercase().contains(&self.query.to_lowercase()))
    }
}

pub fn filter(entries: &[LogEntry], f: &Filter) -> Vec<LogEntry> {
    entries.iter().filter(|e| f.matches(e)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(ts: i64, m: &str, t: &str) -> LogEntry {
        LogEntry {
            ts_ms: ts,
            member: m.into(),
            kind: LogKind::Text,
            text: t.into(),
        }
    }

    #[test]
    fn interleaves_by_time_and_dedupes() {
        let a = vec![e(1, "a", "x"), e(5, "a", "y")];
        let b = vec![e(3, "b", "z"), e(3, "b", "z")];
        let m = merge(vec![a, b], 10);
        let order: Vec<_> = m.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(order, ["x", "z", "y"]);
    }

    #[test]
    fn caps_to_newest() {
        let a: Vec<_> = (0..10).map(|i| e(i, "a", &i.to_string())).collect();
        let m = merge(vec![a], 3);
        assert_eq!(m.iter().map(|e| e.ts_ms).collect::<Vec<_>>(), [7, 8, 9]);
    }

    #[test]
    fn filters() {
        let v = vec![e(1, "a", "Hello"), e(2, "b", "world")];
        let f = Filter {
            members: vec!["b".into()],
            ..Default::default()
        };
        assert_eq!(filter(&v, &f).len(), 1);
        let f = Filter {
            query: "HELLO".into(),
            ..Default::default()
        };
        assert_eq!(filter(&v, &f)[0].member, "a");
        let f = Filter {
            hide: vec![LogKind::Text],
            ..Default::default()
        };
        assert!(filter(&v, &f).is_empty());
    }
}
