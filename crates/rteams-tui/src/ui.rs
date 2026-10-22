//! Rendering. Pure function of `App`.
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use rteams_core::Scope;

use crate::app::{columns, App, Mode, COLUMNS};

pub fn draw(f: &mut Frame, app: &App) {
    let rows = Layout::vertical([
        Constraint::Percentage(50),
        Constraint::Min(5),
        Constraint::Length(3),
    ])
    .split(f.area());
    board(f, app, rows[0]);
    let bottom = if app.open {
        vec![Constraint::Percentage(60), Constraint::Percentage(40)]
    } else {
        vec![Constraint::Percentage(40), Constraint::Percentage(60)]
    };
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(bottom)
        .split(rows[1]);
    detail(f, app, panes[0]);
    messages(f, app, panes[1]);
    command_line(f, app, rows[2]);
}

fn board(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::horizontal([Constraint::Ratio(1, 5); 5]).split(area);
    let data = columns(&app.state);
    for (i, (title, _)) in COLUMNS.iter().enumerate() {
        let focused = i == app.col;
        let items: Vec<ListItem> = data[i]
            .iter()
            .map(|t| ListItem::new(format!("{} {}", t.id, t.subject)))
            .collect();
        let border = if focused {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(border))
                    .title(format!("{} ({})", title, data[i].len())),
            )
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        let mut st = ListState::default();
        if focused && !data[i].is_empty() {
            st.select(Some(app.rows[i]));
        }
        f.render_stateful_widget(list, cols[i], &mut st);
    }
}

fn detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match app.selected() {
        None => lines.push(Line::from("No task selected")),
        Some(t) => {
            lines.push(Line::from(Span::styled(
                format!("{} {}", t.id, t.subject),
                Style::default().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!(
                "status: {:?}   owner: {}",
                t.status,
                t.owner.as_deref().unwrap_or("-")
            )));
            if let Some(r) = &t.reviewer {
                lines.push(Line::from(format!("reviewer: {r}")));
            }
            if !t.blocked_by.is_empty() {
                lines.push(Line::from(format!(
                    "blocked by: {}",
                    t.blocked_by.join(", ")
                )));
            }
            if let Some(r) = &t.blocked_reason {
                lines.push(Line::from(format!("blocked: {r}")));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(t.description.clone()));
        }
    }
    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title("Task"));
    f.render_widget(p, area);
}

fn messages(f: &mut Frame, app: &App, area: Rect) {
    let msgs = app.messages();
    let keep = area.height.saturating_sub(2) as usize;
    let skip = msgs.len().saturating_sub(keep);
    let items: Vec<ListItem> = msgs
        .iter()
        .skip(skip)
        .map(|m| {
            let to = match &m.scope {
                Scope::Task { task } => format!("#{task}"),
                Scope::Direct { to } => format!("@{to}"),
                Scope::Broadcast => "all".to_string(),
                #[allow(unreachable_patterns)]
                _ => "group".to_string(),
            };
            ListItem::new(format!("{} -> {}: {}", m.from, to, m.text))
        })
        .collect();
    let title = if app.open { "Comments" } else { "Messages" };
    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn command_line(f: &mut Frame, app: &App, area: Rect) {
    let (title, body) = match app.mode {
        Mode::Normal => {
            let hint =
                "hjkl/arrows move  Enter open  m message  c comment  s start  d done  q quit";
            let text = if app.status.is_empty() {
                hint.to_string()
            } else {
                app.status.clone()
            };
            ("Keys", text)
        }
        Mode::Message => (
            "Message (@name text, Enter sends, Esc cancels)",
            format!("{}_", app.input),
        ),
        Mode::Comment => (
            "Comment (Enter sends, Esc cancels)",
            format!("{}_", app.input),
        ),
    };
    f.render_widget(
        Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use rteams_core::{State, Status};

    #[test]
    fn renders_board_detail_and_commandline() {
        let mut s = State::default();
        let mut t: rteams_core::Task = serde_json::from_value(serde_json::json!({
            "id": "t1", "subject": "Ship it", "description": "do the thing", "owner": "bob",
            "status": "pending", "blocked_by": [], "reviewer": null, "blocked_from": null,
            "blocked_reason": null, "archived": false, "created_seq": 1
        }))
        .unwrap();
        t.status = Status::InProgress;
        s.tasks.push(t);
        let mut app = App::new(s);
        app.col = 1;
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for needle in [
            "To do",
            "In progress (1)",
            "Done",
            "t1 Ship it",
            "do the thing",
            "Messages",
            "q quit",
        ] {
            assert!(text.contains(needle), "missing {needle}");
        }
    }
}
