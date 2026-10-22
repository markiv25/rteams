//! Pure composer logic: trigger detection, fuzzy filtering, text insertion, snippets.

use rteams_core::{Member, Task};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mention,
    Task,
    Command,
}

/// An open `@` / `#` / `/` token ending at the cursor. Indices are char indices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trigger {
    pub kind: Kind,
    pub start: usize,
    pub end: usize,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Primary text (name, `#id`, command).
    pub label: String,
    pub detail: String,
    /// Text that replaces the trigger (includes the trigger char and a trailing space).
    pub insert: String,
}

/// (command, description key, English fallback)
pub const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "task create",
        "rteams.composer.cmd.taskCreate",
        "Create a task",
    ),
    (
        "assign",
        "rteams.composer.cmd.assign",
        "Assign a task to a member",
    ),
    ("review", "rteams.composer.cmd.review", "Request a review"),
    (
        "comment",
        "rteams.composer.cmd.comment",
        "Comment on a task",
    ),
    ("start", "rteams.composer.cmd.start", "Start a task"),
    (
        "complete",
        "rteams.composer.cmd.complete",
        "Mark a task complete",
    ),
    ("block", "rteams.composer.cmd.block", "Block a task"),
    (
        "help",
        "rteams.composer.cmd.help",
        "Ask the lead what you can do",
    ),
];

pub fn detect(text: &str, cursor: usize) -> Option<Trigger> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    // Slash command: the line so far starts with '/'.
    let line_start = chars[..cursor]
        .iter()
        .rposition(|c| *c == '\n')
        .map_or(0, |i| i + 1);
    if chars.get(line_start) == Some(&'/') {
        let query: String = chars[line_start + 1..cursor].iter().collect();
        if query.ends_with(char::is_whitespace) {
            return None;
        }
        return Some(Trigger {
            kind: Kind::Command,
            start: line_start,
            end: cursor,
            query,
        });
    }
    let start = chars[..cursor]
        .iter()
        .rposition(|c| c.is_whitespace())
        .map_or(0, |i| i + 1);
    let kind = match chars.get(start)? {
        '@' => Kind::Mention,
        '#' => Kind::Task,
        _ => return None,
    };
    if start >= cursor {
        return None;
    }
    Some(Trigger {
        kind,
        start,
        end: cursor,
        query: chars[start + 1..cursor].iter().collect(),
    })
}

/// Subsequence match score (lower is better); `None` = no match. Empty query matches all.
pub fn fuzzy(query: &str, cand: &str) -> Option<usize> {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    let c: Vec<char> = cand.to_lowercase().chars().collect();
    if q.is_empty() {
        return Some(0);
    }
    let mut qi = 0;
    let (mut first, mut gaps, mut last) = (None, 0usize, 0usize);
    for (i, ch) in c.iter().enumerate() {
        if *ch == q[qi] {
            if first.is_none() {
                first = Some(i);
            } else {
                gaps += i - last - 1;
            }
            last = i;
            qi += 1;
            if qi == q.len() {
                return Some(first.unwrap_or(0) * 4 + gaps);
            }
        }
    }
    None
}

fn ranked(query: &str, mut items: Vec<(String, Item)>) -> Vec<Item> {
    let mut scored: Vec<(usize, Item)> = items
        .drain(..)
        .filter_map(|(hay, it)| fuzzy(query, &hay).map(|s| (s, it)))
        .collect();
    scored.sort_by_key(|(s, _)| *s);
    scored.into_iter().map(|(_, it)| it).take(12).collect()
}

pub fn candidates(
    t: &Trigger,
    members: &[Member],
    tasks: &[Task],
    describe: &dyn Fn(&str, &str) -> String,
) -> Vec<Item> {
    match t.kind {
        Kind::Mention => ranked(
            &t.query,
            members
                .iter()
                .filter(|m| !m.removed)
                .map(|m| {
                    (
                        m.name.clone(),
                        Item {
                            label: m.name.clone(),
                            detail: m.role.clone(),
                            insert: format!("@{} ", m.name),
                        },
                    )
                })
                .collect(),
        ),
        Kind::Task => ranked(
            &t.query,
            tasks
                .iter()
                .filter(|k| !k.archived)
                .map(|k| {
                    (
                        format!("{} {}", k.id, k.subject),
                        Item {
                            label: format!("#{}", k.id),
                            detail: k.subject.clone(),
                            insert: format!("#{} ", k.id),
                        },
                    )
                })
                .collect(),
        ),
        Kind::Command => ranked(
            &t.query,
            COMMANDS
                .iter()
                .map(|(name, key, fallback)| {
                    (
                        (*name).to_string(),
                        Item {
                            label: format!("/{name}"),
                            detail: describe(key, fallback),
                            insert: format!("/{name} "),
                        },
                    )
                })
                .collect(),
        ),
    }
}

/// Replace the trigger token with `item.insert`; returns the new text and the new cursor (chars).
pub fn apply(text: &str, t: &Trigger, insert: &str) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let end = t.end.min(chars.len());
    let start = t.start.min(end);
    let mut out: String = chars[..start].iter().collect();
    out.push_str(insert);
    out.extend(chars[end..].iter());
    (out, start + insert.chars().count())
}

fn fence_lang(path: &str) -> &str {
    match path.rsplit('.').next().unwrap_or("") {
        "rs" => "rust",
        "py" => "python",
        "js" => "javascript",
        "ts" => "typescript",
        "tsx" => "tsx",
        "json" => "json",
        "toml" => "toml",
        "md" => "markdown",
        "sh" => "bash",
        "go" => "go",
        "yaml" | "yml" => "yaml",
        _ => "",
    }
}

/// Lines `start..=end` (1-based, clamped) of `content`; `None` when the range is empty.
pub fn slice_lines(content: &str, start: usize, end: usize) -> Option<(usize, usize, String)> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let s = start.max(1).min(lines.len());
    let e = end.max(s).min(lines.len());
    Some((s, e, lines[s - 1..e].join("\n")))
}

/// Fenced snippet reference inserted into the message.
pub fn snippet(rel_path: &str, start: usize, end: usize, code: &str) -> String {
    let range = if start == end {
        start.to_string()
    } else {
        format!("{start}-{end}")
    };
    format!(
        "`{rel_path}:{range}`\n```{}\n{code}\n```\n",
        fence_lang(rel_path)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(n: &str, role: &str, removed: bool) -> Member {
        serde_json::from_value(serde_json::json!({"name": n, "role": role, "removed": removed}))
            .unwrap()
    }

    #[test]
    fn detects_triggers() {
        let t = detect("hi @ad", 6).unwrap();
        assert_eq!(
            (t.kind, t.start, t.query.as_str()),
            (Kind::Mention, 3, "ad")
        );
        assert_eq!(detect("see #1", 6).unwrap().kind, Kind::Task);
        assert!(detect("hi @ad ", 7).is_none());
        assert!(detect("a@b", 3).is_none());
        let c = detect("x\n/task cr", 10).unwrap();
        assert_eq!(
            (c.kind, c.start, c.query.as_str()),
            (Kind::Command, 2, "task cr")
        );
        assert!(detect("/help ", 6).is_none());
        assert!(detect("/", 1).is_some());
        assert!(detect("a /b", 4).is_none());
    }

    #[test]
    fn fuzzy_orders_and_filters() {
        assert!(fuzzy("zz", "ada").is_none());
        assert!(fuzzy("", "ada").is_some());
        assert!(fuzzy("ad", "ada") < fuzzy("ad", "bad"));
        assert!(fuzzy("tc", "task create").is_some());
    }

    #[test]
    fn candidates_filter_members_and_commands() {
        let ms = vec![
            member("ada", "dev", false),
            member("bob", "qa", false),
            member("old", "x", true),
        ];
        let d = |_: &str, f: &str| f.to_string();
        let t = detect("@", 1).unwrap();
        let names: Vec<_> = candidates(&t, &ms, &[], &d)
            .into_iter()
            .map(|i| i.label)
            .collect();
        assert_eq!(names, vec!["ada", "bob"]);
        let t = detect("@b", 2).unwrap();
        assert_eq!(candidates(&t, &ms, &[], &d)[0].insert, "@bob ");
        let t = detect("/rev", 4).unwrap();
        assert_eq!(candidates(&t, &ms, &[], &d)[0].label, "/review");
    }

    #[test]
    fn apply_replaces_token() {
        let t = detect("hello @a there", 8).unwrap();
        let (s, c) = apply("hello @a there", &t, "@ada ");
        assert_eq!(s, "hello @ada  there");
        assert_eq!(c, 11);
        let t = detect("/ta", 3).unwrap();
        assert_eq!(apply("/ta", &t, "/task create ").0, "/task create ");
    }

    #[test]
    fn snippet_and_slice() {
        let (s, e, code) = slice_lines("a\nb\nc\nd", 2, 99).unwrap();
        assert_eq!((s, e, code.as_str()), (2, 4, "b\nc\nd"));
        assert!(slice_lines("", 1, 2).is_none());
        let sn = snippet("src/main.rs", 2, 3, "x\ny");
        assert_eq!(sn, "`src/main.rs:2-3`\n```rust\nx\ny\n```\n");
        assert!(snippet("a.txt", 5, 5, "z").starts_with("`a.txt:5`"));
    }
}
