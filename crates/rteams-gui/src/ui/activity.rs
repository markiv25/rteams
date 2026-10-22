//! Activity feed: the team's events as a timeline, newest first. Runs of the lead's task
//! comments collapse into one "lead thoughts" group, and a block on top lists messages to the
//! lead that have not been answered yet.

use eframe::egui::{self, RichText};
use rteams_core::{Event, EventKind, Scope, State};

use crate::theme;
use crate::view::View;

/// Most recent timeline entries drawn.
const MAX_ENTRIES: usize = 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub ts: i64,
    pub actor: String,
    pub text: String,
    /// Lead's own task comment (grouped with its neighbours).
    pub thought: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    One(Entry),
    Thoughts(Vec<Entry>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pending {
    pub from: String,
    pub ts: i64,
    pub text: String,
}

fn clip(s: &str, n: usize) -> String {
    let one = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > n {
        format!("{}\u{2026}", one.chars().take(n - 1).collect::<String>())
    } else {
        one
    }
}

fn status_word(s: rteams_core::Status) -> String {
    format!("{s:?}").chars().fold(String::new(), |mut a, c| {
        if c.is_uppercase() && !a.is_empty() {
            a.push('_');
        }
        a.extend(c.to_lowercase());
        a
    })
}

/// One-line description of an event; `None` for noise (heartbeats, reactions, ordering).
pub fn describe(e: &Event) -> Option<String> {
    use EventKind::*;
    Some(match &e.kind {
        TeamCreated { name } => format!("created team {name}"),
        MemberAdded { name, role, .. } => format!("added {name} ({role})"),
        TaskCreated { task, subject, .. } => format!("created #{task} {}", clip(subject, 70)),
        TaskAssigned { task, owner } => match owner {
            Some(o) => format!("assigned #{task} to {o}"),
            None => format!("unassigned #{task}"),
        },
        TaskTransitioned { task, to, .. } | TaskColumnSet { task, to, .. } => {
            format!("moved #{task} to {}", status_word(*to))
        }
        TaskBlocked { task, reason, .. } => format!("blocked #{task}: {}", clip(reason, 70)),
        TaskUnblocked { task, .. } => format!("unblocked #{task}"),
        TaskCommented { task, text, .. } => {
            format!("commented on #{task}: {}", clip(text, 90))
        }
        ReviewRequested { task, reviewer } => format!("asked {reviewer} to review #{task}"),
        ReviewApproved { task, .. } => format!("approved #{task}"),
        ReviewChangesRequested { task, comment } => {
            format!("requested changes on #{task}: {}", clip(comment, 70))
        }
        MessageSent { to, text, .. } => match to {
            Some(t) => format!("messaged {t}: {}", clip(text, 90)),
            None => format!("said: {}", clip(text, 90)),
        },
        TaskArchived { task, archived } => {
            format!(
                "{} #{task}",
                if *archived { "archived" } else { "restored" }
            )
        }
        LaunchRequested { members } => format!("asked to launch {}", members.join(", ")),
        StopRequested { members } => format!("asked to stop {}", members.join(", ")),
        _ => return None,
    })
}

/// Timeline rows, newest first, with consecutive lead comments grouped.
pub fn build(events: &[Event], lead: Option<&str>) -> Vec<Row> {
    let entries: Vec<Entry> = events
        .iter()
        .rev()
        .filter_map(|e| {
            let text = describe(e)?;
            Some(Entry {
                ts: e.ts,
                actor: e.actor.clone(),
                thought: Some(e.actor.as_str()) == lead
                    && matches!(e.kind, EventKind::TaskCommented { .. }),
                text,
            })
        })
        .take(MAX_ENTRIES * 2)
        .collect();
    let mut rows: Vec<Row> = Vec::new();
    for e in entries {
        if e.thought {
            match rows.last_mut() {
                Some(Row::Thoughts(g)) => {
                    g.push(e);
                    continue;
                }
                Some(Row::One(prev)) if prev.thought => {
                    let p = prev.clone();
                    rows.pop();
                    rows.push(Row::Thoughts(vec![p, e]));
                    continue;
                }
                _ => {}
            }
        }
        rows.push(Row::One(e));
    }
    rows.truncate(MAX_ENTRIES);
    rows
}

/// Direct messages to the lead that the lead has not replied to since.
pub fn pending(state: &State, lead: &str) -> Vec<Pending> {
    let last_reply = state
        .messages
        .iter()
        .filter(|m| m.from == lead)
        .map(|m| m.seq)
        .max()
        .unwrap_or(0);
    state
        .messages
        .iter()
        .filter(|m| m.from != lead && m.seq > last_reply)
        .filter(|m| matches!(&m.scope, Scope::Direct { to } if to == lead))
        .map(|m| Pending {
            from: m.from.clone(),
            ts: m.ts,
            text: m.text.clone(),
        })
        .collect()
}

pub fn lead_of(v: &View) -> Option<String> {
    v.members()
        .iter()
        .find(|m| m.role == "lead" && !m.removed)
        .map(|m| m.name.clone())
}

pub fn show(ui: &mut egui::Ui, v: &View) {
    let lead = lead_of(v);
    if let Some(l) = &lead {
        let p = pending(v.state, l);
        if !p.is_empty() {
            egui::Frame::new()
                .fill(theme::tint(theme::WARNING, 0.08))
                .stroke(egui::Stroke::new(1.0, theme::tint(theme::WARNING, 0.35)))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(format!(
                            "{} ({})",
                            v.tr.t("rteams.shell.activity.pending"),
                            p.len()
                        ))
                        .strong()
                        .color(theme::WARNING),
                    );
                    for m in p.iter().rev().take(5) {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(theme::hhmm(m.ts))
                                    .size(11.0)
                                    .color(theme::MUTED),
                            );
                            ui.label(RichText::new(&m.from).color(v.color(&m.from)));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(clip(&m.text, 80)).color(theme::TEXT_2),
                                )
                                .truncate(),
                            );
                        });
                    }
                });
            ui.add_space(6.0);
        }
    }
    let rows = build(v.data.events(), lead.as_deref());
    if rows.is_empty() {
        ui.label(RichText::new(v.tr.t("rteams.shell.activity.empty")).color(theme::MUTED));
        return;
    }
    for (i, r) in rows.iter().enumerate() {
        match r {
            Row::One(e) => entry_ui(ui, v, e),
            Row::Thoughts(g) => {
                let title = format!(
                    "{} \u{00b7} {} ({})",
                    g[0].actor,
                    v.tr.t("rteams.shell.activity.leadThoughts"),
                    g.len()
                );
                egui::CollapsingHeader::new(RichText::new(title).color(theme::THINK_FG))
                    .id_salt(("lead-thoughts", i, g[0].ts))
                    .show(ui, |ui| {
                        for e in g {
                            entry_ui(ui, v, e);
                        }
                    });
            }
        }
    }
}

fn entry_ui(ui: &mut egui::Ui, v: &View, e: &Entry) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(theme::hhmm(e.ts))
                .size(11.0)
                .color(theme::MUTED),
        );
        ui.label(RichText::new(&e.actor).strong().color(v.color(&e.actor)));
        ui.add(egui::Label::new(RichText::new(&e.text).color(theme::TEXT_2)).truncate());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(seq: u64, actor: &str, kind: EventKind) -> Event {
        Event {
            seq,
            ts: seq as i64 * 1000,
            actor: actor.into(),
            kind,
        }
    }

    fn comment(seq: u64, actor: &str) -> Event {
        ev(
            seq,
            actor,
            EventKind::TaskCommented {
                task: "1".into(),
                text: "thinking".into(),
                attachments: vec![],
            },
        )
    }

    #[test]
    fn lead_comments_group() {
        let evs = vec![
            ev(
                1,
                "ada",
                EventKind::TaskUnblocked {
                    task: "1".into(),
                    to: rteams_core::Status::Pending,
                },
            ),
            comment(2, "boss"),
            comment(3, "boss"),
            comment(4, "boss"),
            comment(5, "ada"),
        ];
        let rows = build(&evs, Some("boss"));
        assert_eq!(rows.len(), 3);
        assert!(matches!(&rows[1], Row::Thoughts(g) if g.len() == 3));
    }

    #[test]
    fn pending_until_lead_replies() {
        let mut s = State::default();
        let msg = |seq: u64, from: &str, to: &str| -> rteams_core::Message {
            serde_json::from_value(serde_json::json!({
                "seq": seq, "ts": 0, "from": from, "scope": "direct", "to": to, "text": "hi"
            }))
            .unwrap()
        };
        s.messages.push(msg(1, "ada", "boss"));
        assert_eq!(pending(&s, "boss").len(), 1);
        s.messages.push(msg(2, "boss", "ada"));
        assert!(pending(&s, "boss").is_empty());
    }
}
