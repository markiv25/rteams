use crate::command::Command;
use crate::error::CoreError;
use crate::event::Event;
use crate::state::State;

/// In-memory event log + incrementally folded state. Single team per store.
/// Persistent stores (Phase 1) must give the same semantics: `execute` = decide, append, apply.
#[derive(Debug, Clone, Default)]
pub struct MemStore {
    events: Vec<Event>,
    state: State,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_events(events: Vec<Event>) -> Self {
        let state = State::from_events(&events);
        Self { events, state }
    }

    /// Validate `cmd`, append the resulting event (seq = len + 1) and update state.
    pub fn execute(&mut self, actor: &str, ts: i64, cmd: Command) -> Result<&Event, CoreError> {
        let kind = self.state.decide(cmd)?;
        let event = Event {
            seq: self.events.len() as u64 + 1,
            ts,
            actor: actor.to_string(),
            kind,
        };
        self.state.apply(&event);
        self.events.push(event);
        Ok(self.events.last().expect("just pushed"))
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    /// Fold the log from scratch.
    pub fn rebuild(&self) -> State {
        State::from_events(&self.events)
    }
}
