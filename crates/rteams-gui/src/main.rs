//! `rteams-gui [--home DIR] [--team NAME]`: a native board over the shared event log.
//! Reads the same `<home>/<team>/events.jsonl` the MCP server writes; it never writes.

mod app;
mod board;
mod data;
mod detail;

use std::path::PathBuf;

const USAGE: &str = "usage: rteams-gui [--home DIR] [--team NAME]\n\
  --home DIR   shared state root (env RTEAMS_HOME, required)\n\
  --team NAME  team directory under the home (env RTEAMS_TEAM, default `default`)";

struct Config {
    home: PathBuf,
    team: String,
}

fn parse(
    args: impl IntoIterator<Item = String>,
    env_home: Option<String>,
    env_team: Option<String>,
) -> Result<Config, String> {
    let mut home = env_home.filter(|s| !s.is_empty()).map(PathBuf::from);
    let mut team = env_team
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string());
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--home" | "--team" => {
                let v = args.next().ok_or(format!("{a} needs a value"))?;
                if a == "--home" {
                    home = Some(PathBuf::from(v));
                } else {
                    team = v;
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let home = home.ok_or("no home: pass --home DIR or set RTEAMS_HOME")?;
    Ok(Config { home, team })
}

fn main() -> eframe::Result<()> {
    let cfg = match parse(
        std::env::args().skip(1),
        std::env::var("RTEAMS_HOME").ok(),
        std::env::var("RTEAMS_TEAM").ok(),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rteams-gui: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let title = format!("rteams: {}", cfg.team);
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title(title.clone())
            .with_inner_size([1200.0, 720.0])
            .with_min_inner_size([640.0, 360.0]),
        ..Default::default()
    };
    eframe::run_native(
        &title,
        options,
        Box::new(move |_cc| Ok(Box::new(app::App::new(cfg.home, cfg.team)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str], home: Option<&str>, team: Option<&str>) -> Result<Config, String> {
        parse(
            args.iter().map(|s| s.to_string()),
            home.map(String::from),
            team.map(String::from),
        )
    }

    #[test]
    fn flags_override_env_and_team_defaults() {
        let c = p(&[], Some("/h"), None).unwrap();
        assert_eq!((c.home, c.team), (PathBuf::from("/h"), "default".into()));
        let c = p(&["--home", "/x", "--team", "t"], Some("/h"), Some("e")).unwrap();
        assert_eq!((c.home, c.team), (PathBuf::from("/x"), "t".into()));
    }

    #[test]
    fn missing_home_and_bad_args_are_errors() {
        assert!(p(&[], None, None).is_err());
        assert!(p(&["--home"], None, None).is_err());
        assert!(p(&["--nope"], Some("/h"), None).is_err());
    }
}
