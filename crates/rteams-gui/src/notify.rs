//! Desktop notifications: a best-effort OS call plus a pure event-to-text mapping.

use std::collections::HashMap;

use rteams_core::{Event, EventKind, Status};

/// Show a desktop notification. macOS only (via `osascript`); spawns and does not wait.
pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            escape(body),
            escape(title)
        );
        let _ = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (title, body);
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ")
}

fn short(s: &str) -> String {
    s.chars().take(8).collect()
}

/// What a notification is about; the per-type toggles in Settings filter on this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    TaskDone,
    ReviewRequested,
    Message,
    Comment,
    Blocked,
    Approval,
}

/// One notification: what it is, its text and when it happened (unix ms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub kind: NotifyKind,
    pub title: String,
    pub body: String,
    pub ts: i64,
}

/// Notifications for undecided permission requests whose id is not in `seen` yet; every
/// returned id is added to `seen`.
pub fn approval_notifications(
    state: &rteams_core::State,
    seen: &mut std::collections::HashSet<String>,
    ts: i64,
) -> Vec<Note> {
    state
        .permissions
        .iter()
        .filter(|p| p.decided.is_none() && seen.insert(p.id.clone()))
        .map(|p| Note {
            kind: NotifyKind::Approval,
            title: "Approval needed".to_string(),
            body: format!("{} wants to use {}", p.member, p.tool),
            ts,
        })
        .collect()
}

/// A `Note` for each event with `seq > since_seq` that concerns `me`. Events by `me` are
/// skipped. All events are scanned in order so task subjects and owners known before
/// `since_seq` still apply.
pub fn notifications(events: &[Event], since_seq: u64, me: &str) -> Vec<Note> {
    let mut subject: HashMap<&str, &str> = HashMap::new();
    let mut owner: HashMap<&str, &str> = HashMap::new();
    let mut out = Vec::new();
    for e in events {
        match &e.kind {
            EventKind::TaskCreated {
                task,
                subject: s,
                owner: o,
                ..
            } => {
                subject.insert(task, s);
                if let Some(o) = o {
                    owner.insert(task, o);
                }
            }
            EventKind::TaskAssigned { task, owner: o } => match o {
                Some(o) => {
                    owner.insert(task, o);
                }
                None => {
                    owner.remove(task.as_str());
                }
            },
            _ => {}
        }
        if e.seq <= since_seq || e.actor == me {
            continue;
        }
        let label = |task: &str| {
            let s = subject.get(task).copied().unwrap_or("");
            format!("#{} {}", short(task), s).trim().to_string()
        };
        let mine = |task: &str| owner.get(task).copied() == Some(me);
        use NotifyKind as K;
        let item = match &e.kind {
            EventKind::TaskTransitioned {
                task,
                to: Status::Done,
                ..
            } => Some((
                K::TaskDone,
                "Task done".to_string(),
                format!("{} - {}", label(task), e.actor),
            )),
            EventKind::ReviewApproved { task, .. } => Some((
                K::TaskDone,
                "Task approved".to_string(),
                format!("{} - {}", label(task), e.actor),
            )),
            EventKind::ReviewRequested { task, reviewer } if reviewer == me => Some((
                K::ReviewRequested,
                "Review requested".to_string(),
                format!("{} - from {}", label(task), e.actor),
            )),
            EventKind::TaskBlocked { task, reason, .. } => Some((
                K::Blocked,
                "Task blocked".to_string(),
                format!("{} - {}", label(task), reason),
            )),
            EventKind::TaskCommented { task, text, .. } if mine(task) => Some((
                K::Comment,
                format!("Comment from {}", e.actor),
                format!("{}: {}", label(task), text),
            )),
            EventKind::MessageSent {
                to: Some(to), text, ..
            } if to == me => Some((
                K::Message,
                format!("Message from {}", e.actor),
                text.clone(),
            )),
            _ => None,
        };
        out.extend(item.map(|(kind, title, body)| Note {
            kind,
            title,
            body,
            ts: e.ts,
        }));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, actor: &str, kind: EventKind) -> Event {
        Event {
            seq,
            ts: 0,
            actor: actor.into(),
            kind,
        }
    }

    fn base() -> Vec<Event> {
        vec![ev(
            1,
            "boss",
            EventKind::TaskCreated {
                task: "t1".into(),
                subject: "Build it".into(),
                description: String::new(),
                owner: Some("boss".into()),
                blocked_by: vec![],
            },
        )]
    }

    fn run(extra: Vec<EventKind>, actor: &str) -> Vec<Note> {
        let mut evs = base();
        for (i, k) in extra.into_iter().enumerate() {
            evs.push(ev(2 + i as u64, actor, k));
        }
        notifications(&evs, 1, "boss")
    }

    #[test]
    fn per_kind() {
        let done = run(
            vec![EventKind::TaskTransitioned {
                task: "t1".into(),
                from: Status::InProgress,
                to: Status::Done,
            }],
            "ada",
        );
        assert_eq!(done[0].title, "Task done");
        assert!(done[0].body.contains("Build it"));
        assert_eq!(
            run(
                vec![EventKind::ReviewApproved {
                    task: "t1".into(),
                    note: None
                }],
                "ada"
            )[0]
            .title,
            "Task approved"
        );
        assert_eq!(
            run(
                vec![EventKind::ReviewRequested {
                    task: "t1".into(),
                    reviewer: "boss".into()
                }],
                "ada"
            )[0]
            .title,
            "Review requested"
        );
        assert!(run(
            vec![EventKind::ReviewRequested {
                task: "t1".into(),
                reviewer: "bob".into()
            }],
            "ada"
        )
        .is_empty());
        assert_eq!(
            run(
                vec![EventKind::TaskBlocked {
                    task: "t1".into(),
                    reason: "dep".into(),
                    from: Status::Pending
                }],
                "ada"
            )[0]
            .title,
            "Task blocked"
        );
        assert_eq!(
            run(
                vec![EventKind::TaskCommented {
                    task: "t1".into(),
                    text: "hi".into(),
                    attachments: vec![],
                }],
                "ada"
            )[0]
            .title,
            "Comment from ada"
        );
        assert_eq!(
            run(
                vec![EventKind::MessageSent {
                    to: Some("boss".into()),
                    from_team: None,
                    text: "yo".into(),
                    mode: None,
                    attachments: vec![],
                    chat: None,
                }],
                "ada"
            )[0]
            .body,
            "yo"
        );
        assert!(run(
            vec![EventKind::MessageSent {
                to: None,
                from_team: None,
                text: "all".into(),
                mode: None,
                attachments: vec![],
                chat: None,
            }],
            "ada"
        )
        .is_empty());
    }

    #[test]
    fn kinds_for_toggles() {
        let k = run(
            vec![EventKind::TaskBlocked {
                task: "t1".into(),
                reason: "r".into(),
                from: Status::Pending,
            }],
            "ada",
        );
        assert_eq!(k[0].kind, NotifyKind::Blocked);
    }

    #[test]
    fn skips_self_and_old() {
        let k = EventKind::TaskBlocked {
            task: "t1".into(),
            reason: "r".into(),
            from: Status::Pending,
        };
        assert!(run(vec![k.clone()], "boss").is_empty());
        let mut evs = base();
        evs.push(ev(2, "ada", k));
        assert!(notifications(&evs, 2, "boss").is_empty());
    }

    #[test]
    fn escapes() {
        assert_eq!(escape("a\"b\\c\nd"), "a\\\"b\\\\c d");
    }
}
