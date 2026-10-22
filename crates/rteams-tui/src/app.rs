//! Pure UI state and key handling.
use rteams_core::{Message, State, Status, Task};

/// Board columns: title and the statuses each one shows.
pub const COLUMNS: [(&str, &[Status]); 5] = [
    ("To do", &[Status::Pending, Status::Blocked]),
    ("In progress", &[Status::InProgress, Status::NeedsFix]),
    ("Review", &[Status::Review]),
    ("Approved", &[Status::Approved]),
    ("Done", &[Status::Done]),
];

/// Visible (non-archived) tasks per column, in creation order.
pub fn columns(state: &State) -> [Vec<&Task>; 5] {
    let mut out: [Vec<&Task>; 5] = Default::default();
    for t in state.tasks.iter().filter(|t| !t.archived) {
        if let Some(i) = COLUMNS.iter().position(|(_, s)| s.contains(&t.status)) {
            out[i].push(t);
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// Typing a message: `@name text` is direct, plain text is a broadcast.
    Message,
    /// Typing a comment on the selected task.
    Comment,
}

/// A write the runtime must perform through the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    Send { to: Option<String>, text: String },
    Comment { task: String, text: String },
    Start { task: String },
    Complete { task: String },
}

pub struct App {
    pub state: State,
    pub col: usize,
    /// Selected row per column.
    pub rows: [usize; 5],
    pub mode: Mode,
    pub input: String,
    /// Detail pane expanded (Enter).
    pub open: bool,
    pub status: String,
    pub quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new(State::default())
    }
}

impl App {
    pub fn new(state: State) -> Self {
        App {
            state,
            col: 0,
            rows: [0; 5],
            mode: Mode::Normal,
            input: String::new(),
            open: false,
            status: String::new(),
            quit: false,
        }
    }

    pub fn set_state(&mut self, state: State) {
        self.state = state;
        self.clamp();
    }

    fn clamp(&mut self) {
        let lens: Vec<usize> = columns(&self.state).iter().map(Vec::len).collect();
        for (r, n) in self.rows.iter_mut().zip(lens) {
            *r = (*r).min(n.saturating_sub(1));
        }
    }

    pub fn selected(&self) -> Option<&Task> {
        columns(&self.state)[self.col]
            .get(self.rows[self.col])
            .copied()
    }

    /// Task comments for the open task, else every message, oldest first.
    pub fn messages(&self) -> Vec<&Message> {
        let sel = if self.open {
            self.selected().map(|t| t.id.as_str())
        } else {
            None
        };
        self.state
            .messages
            .iter()
            .filter(|m| match sel {
                Some(id) => matches!(&m.scope, rteams_core::Scope::Task { task } if task == id),
                None => true,
            })
            .collect()
    }

    /// Handle one key. Returns the write to perform, if any.
    pub fn key(&mut self, k: Key) -> Option<Effect> {
        match self.mode {
            Mode::Normal => self.key_normal(k),
            _ => self.key_input(k),
        }
    }

    fn key_normal(&mut self, k: Key) -> Option<Effect> {
        let lens: Vec<usize> = columns(&self.state).iter().map(Vec::len).collect();
        match k {
            Key::Char('q') => self.quit = true,
            Key::Left | Key::Char('h') => self.col = self.col.saturating_sub(1),
            Key::Right | Key::Char('l') => self.col = (self.col + 1).min(COLUMNS.len() - 1),
            Key::Up | Key::Char('k') => self.rows[self.col] = self.rows[self.col].saturating_sub(1),
            Key::Down | Key::Char('j') => {
                let max = lens[self.col].saturating_sub(1);
                self.rows[self.col] = (self.rows[self.col] + 1).min(max);
            }
            Key::Enter => self.open = !self.open && self.selected().is_some(),
            Key::Esc => self.open = false,
            Key::Char('m') => self.begin(Mode::Message),
            Key::Char('c') => {
                if self.selected().is_some() {
                    self.begin(Mode::Comment);
                } else {
                    self.status = "select a task to comment on".into();
                }
            }
            Key::Char('s') => return self.task_effect(|task| Effect::Start { task }),
            Key::Char('d') => return self.task_effect(|task| Effect::Complete { task }),
            _ => {}
        }
        self.clamp();
        None
    }

    fn task_effect(&mut self, f: impl FnOnce(String) -> Effect) -> Option<Effect> {
        match self.selected() {
            Some(t) => Some(f(t.id.clone())),
            None => {
                self.status = "no task selected".into();
                None
            }
        }
    }

    fn begin(&mut self, mode: Mode) {
        self.mode = mode;
        self.input.clear();
        self.status.clear();
    }

    fn key_input(&mut self, k: Key) -> Option<Effect> {
        match k {
            Key::Esc => self.mode = Mode::Normal,
            Key::Char(c) => self.input.push(c),
            Key::Backspace => {
                self.input.pop();
            }
            Key::Enter => {
                let text = self.input.trim().to_string();
                let mode = self.mode;
                self.mode = Mode::Normal;
                self.input.clear();
                if text.is_empty() {
                    return None;
                }
                return match mode {
                    Mode::Comment => self.selected().map(|t| Effect::Comment {
                        task: t.id.clone(),
                        text,
                    }),
                    _ => Some(parse_message(&text)),
                };
            }
            _ => {}
        }
        None
    }
}

/// `@name rest` -> direct message, anything else -> broadcast.
pub fn parse_message(text: &str) -> Effect {
    if let Some(rest) = text.strip_prefix('@') {
        if let Some((to, body)) = rest.split_once(char::is_whitespace) {
            if !to.is_empty() && !body.trim().is_empty() {
                return Effect::Send {
                    to: Some(to.to_string()),
                    text: body.trim().to_string(),
                };
            }
        }
    }
    Effect::Send {
        to: None,
        text: text.to_string(),
    }
}

/// Terminal-independent key, so tests need no crossterm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Up,
    Down,
    Left,
    Right,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(statuses: &[Status]) -> State {
        let mut s = State::default();
        for (i, st) in statuses.iter().enumerate() {
            let mut t: Task = serde_json::from_value(serde_json::json!({
                "id": format!("t{i}"), "subject": "s", "description": "", "owner": null,
                "status": "pending", "blocked_by": [], "reviewer": null, "blocked_from": null,
                "blocked_reason": null, "archived": false, "created_seq": i
            }))
            .unwrap();
            t.status = *st;
            s.tasks.push(t);
        }
        s
    }

    #[test]
    fn grouping_and_navigation() {
        let s = state_with(&[
            Status::Pending,
            Status::Blocked,
            Status::Done,
            Status::Review,
        ]);
        let cols = columns(&s);
        assert_eq!(cols[0].len(), 2);
        assert_eq!(cols[2].len(), 1);
        let mut a = App::new(s);
        a.key(Key::Char('j'));
        assert_eq!(a.rows[0], 1);
        a.key(Key::Down); // clamped
        assert_eq!(a.rows[0], 1);
        a.key(Key::Char('l'));
        a.key(Key::Right);
        assert_eq!(a.col, 2);
        assert_eq!(a.selected().unwrap().id, "t3");
        for _ in 0..9 {
            a.key(Key::Char('l'));
        }
        assert_eq!(a.col, 4);
        a.key(Key::Char('h'));
        assert_eq!(a.col, 3);
        assert!(a.selected().is_none());
    }

    #[test]
    fn actions_and_input() {
        let mut a = App::new(state_with(&[Status::Pending]));
        assert_eq!(
            a.key(Key::Char('s')),
            Some(Effect::Start { task: "t0".into() })
        );
        assert_eq!(
            a.key(Key::Char('d')),
            Some(Effect::Complete { task: "t0".into() })
        );
        a.key(Key::Char('c'));
        assert_eq!(a.mode, Mode::Comment);
        for c in "hi".chars() {
            a.key(Key::Char(c));
        }
        a.key(Key::Backspace);
        a.key(Key::Char('o'));
        assert_eq!(
            a.key(Key::Enter),
            Some(Effect::Comment {
                task: "t0".into(),
                text: "ho".into()
            })
        );
        assert_eq!(a.mode, Mode::Normal);
        a.key(Key::Char('m'));
        for c in "@bob hello there".chars() {
            a.key(Key::Char(c));
        }
        assert_eq!(
            a.key(Key::Enter),
            Some(Effect::Send {
                to: Some("bob".into()),
                text: "hello there".into()
            })
        );
        a.key(Key::Char('m'));
        a.key(Key::Esc);
        assert_eq!(a.mode, Mode::Normal);
        a.key(Key::Char('q'));
        assert!(a.quit);
    }

    #[test]
    fn broadcast_and_empty_selection() {
        assert_eq!(
            parse_message("@bob"),
            Effect::Send {
                to: None,
                text: "@bob".into()
            }
        );
        let mut a = App::default();
        assert_eq!(a.key(Key::Char('s')), None);
        a.key(Key::Char('c'));
        assert_eq!(a.mode, Mode::Normal);
        a.key(Key::Enter);
        assert!(!a.open);
    }
}
