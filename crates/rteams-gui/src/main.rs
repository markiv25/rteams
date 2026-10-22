//! `rteams-gui [--home DIR] [--team NAME] [--as NAME]`: a native board over the shared event
//! log. Reads the same `<home>/<team>/events.jsonl` the MCP server writes, and writes through the
//! same tool layer (`rteams_server`), so the GUI follows the agents' rules.

mod actions;
mod app;
mod approvals;
mod board;
mod changes;
mod context_usage;
mod data;
mod diagnostics;
mod forms;
mod git;
mod icons;
mod installer;
mod launcher;
mod links;
mod logs;
mod member_dialog;
mod member_preview;
mod messages;
mod names;
mod notifications;
mod notify;
mod provider_cfg;
mod providers;
mod runs;
mod runtimes;
mod settings;
mod task_dialog;
mod task_logs;
mod team;
mod teams;
mod theme;
mod ui;
mod view;
mod wake;

use std::path::PathBuf;

const USAGE: &str = "usage: rteams-gui [--home DIR] [--team NAME] [--as NAME]\n\
  --home DIR   shared state root (env RTEAMS_HOME, required)\n\
  --team NAME  team directory under the home (env RTEAMS_TEAM, default `default`)\n\
  --as NAME    member you act as (env RTEAMS_ACTOR, default: the team lead)";

struct Config {
    home: PathBuf,
    team: String,
    actor: Option<String>,
}

fn parse(
    args: impl IntoIterator<Item = String>,
    env_home: Option<String>,
    env_team: Option<String>,
    env_actor: Option<String>,
) -> Result<Config, String> {
    let mut actor = env_actor.filter(|s| !s.is_empty());
    let mut home = env_home.filter(|s| !s.is_empty()).map(PathBuf::from);
    let mut team = env_team
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string());
    let mut args = args.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--home" | "--team" | "--as" => {
                let v = args.next().ok_or(format!("{a} needs a value"))?;
                match a.as_str() {
                    "--home" => home = Some(PathBuf::from(v)),
                    "--team" => team = v,
                    _ => actor = Some(v),
                }
            }
            other => return Err(format!("unknown argument {other:?}")),
        }
    }
    let home = home.ok_or("no home: pass --home DIR or set RTEAMS_HOME")?;
    Ok(Config { home, team, actor })
}

fn main() -> eframe::Result<()> {
    // Agents run in their own process groups; if the app is terminated (SIGTERM, Ctrl+C) they
    // must not keep running, spending tokens, with nobody watching.
    let _ = ctrlc::set_handler(|| {
        launcher::kill_all_agents();
        std::process::exit(130);
    });
    let cfg = match parse(
        std::env::args().skip(1),
        std::env::var("RTEAMS_HOME").ok(),
        std::env::var("RTEAMS_TEAM").ok(),
        std::env::var("RTEAMS_ACTOR").ok(),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rteams-gui: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let title = app::window_title(&cfg.team);
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
        Box::new(move |cc| {
            Ok(Box::new(app::App::new(
                &cc.egui_ctx,
                cfg.home,
                cfg.team,
                cfg.actor,
            )))
        }),
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
            None,
        )
    }

    #[test]
    fn flags_override_env_and_team_defaults() {
        let c = p(&[], Some("/h"), None).unwrap();
        assert_eq!((c.home, c.team), (PathBuf::from("/h"), "default".into()));
        let c = p(&["--home", "/x", "--team", "t"], Some("/h"), Some("e")).unwrap();
        assert_eq!((c.home, c.team), (PathBuf::from("/x"), "t".into()));
        assert_eq!(c.actor, None);
        let c = p(&["--as", "boss"], Some("/h"), None).unwrap();
        assert_eq!(c.actor.as_deref(), Some("boss"));
    }

    #[test]
    fn missing_home_and_bad_args_are_errors() {
        assert!(p(&[], None, None).is_err());
        assert!(p(&["--home"], None, None).is_err());
        assert!(p(&["--nope"], Some("/h"), None).is_err());
    }
}
