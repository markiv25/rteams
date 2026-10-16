//! Semantic steps of an AI response: text, thinking, tool calls and subagent runs, with the
//! start / end times the waterfall needs.

use super::tools::ToolExec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Text,
    Thinking,
    ToolCall,
    Subagent,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub kind: StepKind,
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    /// Body for `Text` / `Thinking`.
    pub text: String,
    /// The call for `ToolCall` / `Subagent`.
    pub tool: Option<ToolExec>,
    /// Index into `Session::subagents` for `Subagent` steps.
    pub subagent: Option<usize>,
}

impl Step {
    pub fn text(kind: StepKind, text: String, ts: Option<i64>) -> Step {
        Step {
            kind,
            start_ms: ts,
            end_ms: None,
            text,
            tool: None,
            subagent: None,
        }
    }

    pub fn call(tool: ToolExec) -> Step {
        Step {
            kind: StepKind::ToolCall,
            start_ms: tool.start_ms,
            end_ms: None,
            text: String::new(),
            tool: Some(tool),
            subagent: None,
        }
    }

    pub fn duration_ms(&self) -> Option<i64> {
        match (self.start_ms, self.end_ms) {
            (Some(a), Some(b)) if b >= a => Some(b - a),
            _ => None,
        }
    }

    /// Short label for lists and the waterfall.
    pub fn label(&self) -> String {
        match self.kind {
            StepKind::Text => first_line(&self.text, 60),
            StepKind::Thinking => format!("thinking: {}", first_line(&self.text, 50)),
            StepKind::ToolCall | StepKind::Subagent => self
                .tool
                .as_ref()
                .map(|t| {
                    let s = t.summary();
                    if s.is_empty() {
                        t.name.clone()
                    } else {
                        format!("{} {}", t.name, s)
                    }
                })
                .unwrap_or_default(),
        }
    }
}

fn first_line(t: &str, n: usize) -> String {
    let l = t.trim().lines().next().unwrap_or("");
    let mut o: String = l.chars().take(n).collect();
    if l.chars().count() > n {
        o.push('\u{2026}');
    }
    o
}

/// Fill missing `end_ms`: a step ends when the next one starts (the last one at `tail`).
/// Tool calls keep the end of their result.
pub fn close_steps(steps: &mut [Step], tail: Option<i64>) {
    for i in 0..steps.len() {
        if steps[i].end_ms.is_some() {
            continue;
        }
        let next = steps[i + 1..].iter().find_map(|s| s.start_ms).or(tail);
        steps[i].end_ms = match (steps[i].start_ms, next) {
            (Some(a), Some(b)) if b >= a => Some(b),
            (Some(a), _) => Some(a),
            _ => None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_with_next_start() {
        let mut v = vec![
            Step::text(StepKind::Text, "a".into(), Some(100)),
            Step::text(StepKind::Thinking, "b".into(), Some(400)),
        ];
        close_steps(&mut v, Some(900));
        assert_eq!(v[0].duration_ms(), Some(300));
        assert_eq!(v[1].duration_ms(), Some(500));
    }

    #[test]
    fn labels() {
        let s = Step::text(StepKind::Text, "hello\nworld".into(), None);
        assert_eq!(s.label(), "hello");
    }
}
