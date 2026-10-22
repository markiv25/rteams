use std::path::PathBuf;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use rteams_store::FileStore;
use rteams_tui::app::{App, Key};
use rteams_tui::io::{default_actor, Writer};
use rteams_tui::ui::draw;

fn usage() -> ! {
    eprintln!("usage: rteams-tui --home DIR --team NAME [--as NAME]");
    std::process::exit(2);
}

fn map_key(c: KeyCode) -> Option<Key> {
    Some(match c {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        _ => return None,
    })
}

fn main() {
    let (mut home, mut team, mut actor) = (None, None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--home" => home = args.next().map(PathBuf::from),
            "--team" => team = args.next(),
            "--as" => actor = args.next(),
            _ => usage(),
        }
    }
    let (Some(home), Some(team)) = (home, team) else {
        usage()
    };
    let mut reader = match FileStore::open_read_only(&home, &team) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot open team {team}: {e}");
            std::process::exit(1);
        }
    };
    let actor = default_actor(reader.loaded_state(), actor);
    let writer = match Writer::open(&home, &team, actor) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let mut app = App::new(reader.loaded_state().clone());
    let mut term = ratatui::init();
    let mut dirty = true;
    while !app.quit {
        if dirty {
            let _ = term.draw(|f| draw(f, &app));
            dirty = false;
        }
        if event::poll(Duration::from_millis(500)).unwrap_or(false) {
            if let Ok(Event::Key(k)) = event::read() {
                if k.kind == KeyEventKind::Press {
                    if let Some(effect) = map_key(k.code).and_then(|k| app.key(k)) {
                        app.status = match writer.run(&effect) {
                            Ok(()) => "done".into(),
                            Err(e) => format!("error: {e}"),
                        };
                    }
                }
            }
            dirty = true;
        }
        // Tail the log (also picks up our own writes).
        if reader.refresh().is_ok() {
            app.set_state(reader.loaded_state().clone());
            dirty = true;
        }
    }
    ratatui::restore();
}
