//! Event-driven agent wake scheduler. Pure and deterministic: the caller passes `now_ms`.
//!
//! Agents are one-shot runs, so we only start one when the shared log gives it something to do,
//! and we hand it everything in the prompt so it can skip briefing tool calls.

use std::collections::{BTreeMap, HashMap};

use rteams_core::{briefing, inbox, Event, EventKind, Scope, State};

/// A member becomes due this long after its latest reason (coalesces bursts).
pub const DEBOUNCE_MS: i64 = 2_000;
/// Minimum gap between two wakes of the same member.
pub const MIN_GAP_MS: i64 = 20_000;
/// Max wakes per member per rolling hour.
pub const MAX_PER_HOUR: usize = 6;
const HOUR_MS: i64 = 3_600_000;
/// Soft size budget for a prompt, in bytes.
pub const PROMPT_BUDGET: usize = 1_500;

const MAX_REASONS: usize = 5;
const MAX_TASKS: usize = 5;
const MAX_INBOX: usize = 5;
const REASON_CLIP: usize = 90;
const SUBJECT_CLIP: usize = 50;

#[derive(Debug, Clone, Default)]
struct Pending {
    reasons: Vec<String>,
    /// Time of the latest reason.
    last_ms: i64,
    /// Mode of the latest triggering message, if any.
    mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeOrder {
    pub member: String,
    pub reasons: Vec<String>,
    pub prompt: String,
    /// Mode of the latest triggering message (`do` / `ask` / `delegate`), if any.
    pub mode: Option<String>,
}

#[derive(Debug, Default)]
pub struct WakeScheduler {
    cursor: u64,
    pending: BTreeMap<String, Pending>,
    /// Wake times per member (pruned to the last hour).
    wakes: HashMap<String, Vec<i64>>,
    /// Event cursor at each member's last wake; the inbox is shown from here.
    woken_at_seq: HashMap<String, u64>,
}

impl WakeScheduler {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start after `seq`: existing history never wakes anyone.
    pub fn with_cursor(seq: u64) -> Self {
        Self {
            cursor: seq,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Fold events with `seq > cursor` into pending wake reasons.
    pub fn observe(&mut self, events: &[Event], state: &State, agents: &[String], now_ms: i64) {
        let start = self.cursor;
        for e in events.iter().filter(|e| e.seq > start) {
            self.cursor = self.cursor.max(e.seq);
            let actor = e.actor.as_str();
            let subj = |task: &str| {
                state
                    .task(task)
                    .map(|t| clip(&t.subject, SUBJECT_CLIP))
                    .unwrap_or_default()
            };
            let owner_of = |task: &str| state.task(task).and_then(|t| t.owner.clone());
            let mut out: Vec<(String, String)> = Vec::new();
            let mut mode_of: Option<String> = None;
            match &e.kind {
                EventKind::MessageSent {
                    to: Some(to),
                    text,
                    mode,
                    ..
                } => {
                    mode_of = mode.clone();
                    out.push((
                        to.clone(),
                        format!("{actor} messaged you: {}", clip(text, REASON_CLIP)),
                    ));
                }
                EventKind::MessageSent {
                    to: None,
                    text,
                    mode,
                    ..
                } => {
                    mode_of = mode.clone();
                    for a in agents {
                        out.push((
                            a.clone(),
                            format!("{actor} broadcast: {}", clip(text, REASON_CLIP)),
                        ));
                    }
                }
                EventKind::TaskAssigned {
                    task,
                    owner: Some(o),
                } => out.push((
                    o.clone(),
                    format!("task #{task} {} assigned to you", subj(task)),
                )),
                EventKind::TaskCreated {
                    task,
                    subject,
                    owner: Some(o),
                    ..
                } => out.push((
                    o.clone(),
                    format!("new task #{task} {} for you", clip(subject, SUBJECT_CLIP)),
                )),
                EventKind::ReviewRequested { task, reviewer } => out.push((
                    reviewer.clone(),
                    format!("review requested on #{task} {}", subj(task)),
                )),
                EventKind::ReviewChangesRequested { task, comment } => {
                    if let Some(o) = owner_of(task) {
                        out.push((
                            o,
                            format!(
                                "changes requested on #{task} {}: {}",
                                subj(task),
                                clip(comment, REASON_CLIP)
                            ),
                        ));
                    }
                }
                EventKind::TaskCommented { task, text, .. } => {
                    if let Some(o) = owner_of(task) {
                        out.push((
                            o,
                            format!("{actor} commented on #{task}: {}", clip(text, REASON_CLIP)),
                        ));
                    }
                }
                EventKind::TaskUnblocked { task, .. } => {
                    if let Some(o) = owner_of(task) {
                        out.push((o, format!("#{task} {} was unblocked", subj(task))));
                    }
                }
                _ => {}
            }
            for (member, reason) in out {
                if member == actor || !agents.contains(&member) || !state.is_member(&member) {
                    continue;
                }
                let p = self.pending.entry(member).or_default();
                p.reasons.push(reason);
                p.last_ms = p.last_ms.max(now_ms);
                if mode_of.is_some() {
                    p.mode = mode_of.clone();
                }
            }
        }
    }

    /// Members to launch now. `running` members are skipped (reasons stay pending);
    /// at most `max_concurrent - running_count` are returned. Marks them woken.
    pub fn due(
        &mut self,
        state: &State,
        running: &[String],
        max_concurrent: usize,
        running_count: usize,
        now_ms: i64,
    ) -> Vec<WakeOrder> {
        let mut slots = max_concurrent.saturating_sub(running_count);
        let mut orders = Vec::new();
        let names: Vec<String> = self.pending.keys().cloned().collect();
        for m in names {
            if slots == 0 {
                break;
            }
            if running.contains(&m) || now_ms < self.pending[&m].last_ms + DEBOUNCE_MS {
                continue;
            }
            let hist = self.wakes.entry(m.clone()).or_default();
            hist.retain(|t| now_ms - t < HOUR_MS);
            if hist.len() >= MAX_PER_HOUR || hist.last().is_some_and(|t| now_ms - t < MIN_GAP_MS) {
                continue;
            }
            hist.push(now_ms);
            let p = self.pending.remove(&m).unwrap_or_default();
            let after = self.woken_at_seq.get(&m).copied().unwrap_or(0);
            self.woken_at_seq.insert(m.clone(), self.cursor);
            let prompt = build_prompt_after(state, &m, &p.reasons, p.mode.as_deref(), after);
            orders.push(WakeOrder {
                member: m,
                reasons: p.reasons,
                prompt,
                mode: p.mode,
            });
            slots -= 1;
        }
        orders
    }

    /// Queued wakes for the UI.
    pub fn pending(&self) -> Vec<(String, Vec<String>)> {
        self.pending
            .iter()
            .map(|(m, p)| (m.clone(), p.reasons.clone()))
            .collect()
    }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= max {
        s
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Self-contained prompt for a woken member (inbox since the beginning of the log).
pub fn build_prompt(state: &State, member: &str, reasons: &[String], mode: Option<&str>) -> String {
    build_prompt_after(state, member, reasons, mode, 0)
}

fn build_prompt_after(
    state: &State,
    member: &str,
    reasons: &[String],
    mode: Option<&str>,
    after: u64,
) -> String {
    let b = briefing(state, member);
    let msgs: Vec<_> = inbox(state, member, after)
        .items
        .into_iter()
        .filter(|m| !matches!(m.scope, Scope::Task { .. }))
        .collect();
    let skip = msgs.len().saturating_sub(MAX_INBOX);
    let msgs = &msgs[skip..];

    let mut clips = [200usize, 120, 80, 50, 30, 15].into_iter();
    loop {
        let c = clips.next().unwrap_or(10);
        let mut s = format!("You were woken, {member}. Why:\n");
        for r in reasons.iter().take(MAX_REASONS) {
            s.push_str(&format!("- {}\n", clip(r, REASON_CLIP)));
        }
        if reasons.len() > MAX_REASONS {
            s.push_str(&format!("- (+{} more)\n", reasons.len() - MAX_REASONS));
        }
        if let Some(m) = mode {
            s.push_str(&format!("Mode: {}\n", clip(m, 40)));
        }
        if !b.actionable.is_empty() {
            s.push_str("Your tasks:\n");
            for i in b.actionable.iter().take(MAX_TASKS) {
                s.push_str(&format!(
                    "- #{} [{:?}] {}\n",
                    i.task,
                    i.status,
                    clip(&i.subject, SUBJECT_CLIP)
                ));
            }
        }
        let mut cut = false;
        if !msgs.is_empty() {
            s.push_str("Unread messages:\n");
            for m in msgs {
                let kind = if matches!(m.scope, Scope::Broadcast) {
                    " (all)"
                } else {
                    ""
                };
                let shown = clip(&m.text, c);
                cut |= shown.ends_with('…');
                s.push_str(&format!("- {}{kind}: {}\n", m.from, shown));
            }
        }
        if cut {
            // A long request clipped to a line would read as the whole request.
            s.push_str("Messages ending in … are cut short: read them in full with inbox_get.\n");
        }
        s.push_str(
            "This is everything you need — skip member_briefing/task_briefing unless something is unclear.",
        );
        if s.len() <= PROMPT_BUDGET || c <= 10 {
            return s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rteams_core::Status;

    fn ev(seq: u64, actor: &str, kind: EventKind) -> Event {
        Event {
            seq,
            ts: 0,
            actor: actor.into(),
            kind,
        }
    }
    fn base() -> Vec<Event> {
        let mut v = vec![ev(1, "lead", EventKind::TeamCreated { name: "t".into() })];
        for (i, n) in ["lead", "alice", "bob", "carol"].iter().enumerate() {
            v.push(ev(
                2 + i as u64,
                "lead",
                EventKind::MemberAdded {
                    name: (*n).into(),
                    role: "dev".into(),
                    runtime: None,
                },
            ));
        }
        v
    }
    fn dm(seq: u64, from: &str, to: Option<&str>, text: &str) -> Event {
        ev(
            seq,
            from,
            EventKind::MessageSent {
                to: to.map(Into::into),
                text: text.into(),
                mode: None,
                from_team: None,
                attachments: vec![],
                chat: None,
            },
        )
    }
    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn run(evs: &[Event], sched: &mut WakeScheduler, agents: &[&str], now: i64) {
        let st = State::from_events(evs);
        sched.observe(evs, &st, &names(agents), now);
    }

    #[test]
    fn self_events_never_wake() {
        let mut evs = base();
        evs.push(dm(10, "bob", Some("bob"), "note"));
        let mut s = WakeScheduler::new();
        run(&evs, &mut s, &["alice", "bob"], 0);
        assert!(s.pending().is_empty());
    }

    #[test]
    fn dm_wakes_once_after_debounce() {
        let mut evs = base();
        evs.push(dm(10, "lead", Some("bob"), "please look"));
        let st = State::from_events(&evs);
        let mut s = WakeScheduler::new();
        s.observe(&evs, &st, &names(&["bob"]), 1000);
        assert!(s.due(&st, &[], 3, 0, 2999).is_empty());
        let o = s.due(&st, &[], 3, 0, 3000);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].member, "bob");
        assert!(o[0].prompt.contains("lead messaged you: please look"));
        assert!(s.due(&st, &[], 3, 0, 9000).is_empty());
        assert!(s.pending().is_empty());
    }

    #[test]
    fn burst_coalesces() {
        let mut evs = base();
        for i in 0..5 {
            evs.push(dm(10 + i, "lead", Some("bob"), &format!("m{i}")));
        }
        let st = State::from_events(&evs);
        let mut s = WakeScheduler::new();
        s.observe(&evs, &st, &names(&["bob"]), 0);
        let o = s.due(&st, &[], 3, 0, 5000);
        assert_eq!(o.len(), 1);
        assert_eq!(o[0].reasons.len(), 5);
    }

    #[test]
    fn running_member_deferred() {
        let mut evs = base();
        evs.push(dm(10, "lead", Some("bob"), "x"));
        let st = State::from_events(&evs);
        let mut s = WakeScheduler::new();
        s.observe(&evs, &st, &names(&["bob"]), 0);
        assert!(s.due(&st, &names(&["bob"]), 3, 1, 5000).is_empty());
        assert_eq!(s.pending().len(), 1);
        assert_eq!(s.due(&st, &[], 3, 0, 6000).len(), 1);
    }

    #[test]
    fn rate_limits() {
        let mut evs = base();
        let st0 = State::from_events(&evs);
        let mut s = WakeScheduler::new();
        let mut woke = 0;
        let mut t = 0;
        for i in 0..8u64 {
            evs.push(dm(10 + i, "lead", Some("bob"), "x"));
            let st = State::from_events(&evs);
            s.observe(&evs, &st, &names(&["bob"]), t);
            // not before min gap after a wake
            let n = s.due(&st, &[], 3, 0, t + DEBOUNCE_MS).len();
            woke += n;
            t += MIN_GAP_MS + DEBOUNCE_MS; // 22s apart: gap ok, hourly cap bites
        }
        let _ = st0;
        assert_eq!(woke, MAX_PER_HOUR);
        assert_eq!(s.pending().len(), 1);

        // min gap: a second reason right after a wake stays pending
        let mut evs = base();
        evs.push(dm(10, "lead", Some("bob"), "a"));
        let st = State::from_events(&evs);
        let mut s = WakeScheduler::new();
        s.observe(&evs, &st, &names(&["bob"]), 0);
        assert_eq!(s.due(&st, &[], 3, 0, 2000).len(), 1);
        evs.push(dm(11, "lead", Some("bob"), "b"));
        let st = State::from_events(&evs);
        s.observe(&evs, &st, &names(&["bob"]), 3000);
        assert!(s.due(&st, &[], 3, 0, 6000).is_empty());
        assert_eq!(s.pending().len(), 1);
        assert_eq!(s.due(&st, &[], 3, 0, 22_000).len(), 1);
    }

    #[test]
    fn global_cap() {
        let mut evs = base();
        evs.push(dm(10, "lead", None, "all hands"));
        let st = State::from_events(&evs);
        let mut s = WakeScheduler::new();
        s.observe(&evs, &st, &names(&["alice", "bob", "carol"]), 0);
        assert_eq!(s.due(&st, &[], 2, 1, 5000).len(), 1);
        assert_eq!(s.pending().len(), 2);
        assert!(s.due(&st, &[], 2, 2, 5000).is_empty());
    }

    #[test]
    fn broadcast_fanout() {
        let mut evs = base();
        evs.push(dm(10, "alice", None, "hi"));
        let mut s = WakeScheduler::new();
        // carol is not an agent, lead is the human
        run(&evs, &mut s, &["alice", "bob"], 0);
        let p = s.pending();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].0, "bob");
    }

    #[test]
    fn task_reasons() {
        let mut evs = base();
        evs.push(ev(
            10,
            "lead",
            EventKind::TaskCreated {
                task: "1".into(),
                subject: "Fix login".into(),
                description: String::new(),
                owner: Some("bob".into()),
                blocked_by: vec![],
            },
        ));
        evs.push(ev(
            11,
            "bob",
            EventKind::ReviewRequested {
                task: "1".into(),
                reviewer: "alice".into(),
            },
        ));
        evs.push(ev(
            12,
            "alice",
            EventKind::TaskCommented {
                task: "1".into(),
                text: "nit".into(),
                attachments: vec![],
            },
        ));
        evs.push(ev(
            13,
            "bob",
            EventKind::TaskCommented {
                task: "1".into(),
                text: "own".into(),
                attachments: vec![],
            },
        ));
        let mut s = WakeScheduler::new();
        run(&evs, &mut s, &["alice", "bob"], 0);
        let p: HashMap<_, _> = s.pending().into_iter().collect();
        assert_eq!(p["bob"].len(), 2); // created + alice's comment, not own
        assert!(p["alice"][0].contains("review requested on #1 Fix login"));
        // cursor: re-observing does nothing
        run(&evs, &mut s, &["alice", "bob"], 5);
        assert_eq!(s.pending().len(), 2);
        assert_eq!(s.cursor(), 13);
        let _ = Status::Pending;
    }

    #[test]
    fn prompt_contents_and_size() {
        let mut evs = base();
        let long = "y".repeat(500);
        evs.push(ev(
            10,
            "lead",
            EventKind::TaskCreated {
                task: "1".into(),
                subject: "Fix login".into(),
                description: String::new(),
                owner: Some("bob".into()),
                blocked_by: vec![],
            },
        ));
        for i in 0..8 {
            evs.push(dm(20 + i, "alice", Some("bob"), &long));
        }
        evs.push(dm(40, "alice", None, "broadcast hello"));
        let st = State::from_events(&evs);
        let reasons: Vec<String> = (0..8)
            .map(|_| format!("alice messaged you: {long}"))
            .collect();
        let p = build_prompt(&st, "bob", &reasons, Some("urgent"));
        assert!(p.contains("alice messaged you"));
        assert!(p.contains("#1"));
        assert!(p.contains("Fix login"));
        assert!(p.contains("Unread messages"));
        assert!(p.contains("broadcast hello"));
        assert!(p.contains("Mode: urgent"));
        assert!(p.contains("skip member_briefing/task_briefing"));
        assert!(p.contains("read them in full with inbox_get"));
        assert!(p.len() < 1500, "{} bytes\n{p}", p.len());
        let short = build_prompt(&st_short(), "bob", &[], None);
        assert!(!short.contains("inbox_get"), "{short}");
    }

    fn st_short() -> State {
        let mut evs = base();
        evs.push(dm(20, "alice", Some("bob"), "short note"));
        State::from_events(&evs)
    }
}
