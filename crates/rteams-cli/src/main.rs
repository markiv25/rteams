//! `rteams [serve] [--home DIR] [--team NAME]`: MCP over stdio. Logs never go to stdout.
//!
//! With a home (`--home` or `$RTEAMS_HOME`) the server is backed by `<home>/<team>/events.jsonl`,
//! so every process pointed at the same home and team shares one durable state. The team is
//! `--team` / `$RTEAMS_TEAM`, default `default`. Without a home the state is in-memory and
//! private to this process.

use std::path::PathBuf;
use std::process::ExitCode;

use rteams_server::Server;
use rteams_store::FileStore;

const USAGE: &str = "usage: rteams [serve] [--home DIR] [--team NAME]\n\
  --home DIR   shared state root (env RTEAMS_HOME); omit for in-memory state\n\
  --team NAME  team directory under the home (env RTEAMS_TEAM, default `default`)";

#[derive(Debug, PartialEq, Eq)]
struct Config {
    home: Option<PathBuf>,
    team: String,
}

fn parse(
    args: impl IntoIterator<Item = String>,
    env_home: Option<String>,
    env_team: Option<String>,
) -> Result<Config, String> {
    let mut home = env_home.filter(|s| !s.is_empty()).map(PathBuf::from);
    let mut team = env_team.filter(|s| !s.is_empty());
    let mut args = args.into_iter().peekable();
    if args.peek().is_some_and(|a| a == "serve") {
        args.next();
    }
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg, None),
        };
        match flag.as_str() {
            "--home" | "--team" => {
                let value = inline
                    .or_else(|| args.next())
                    .ok_or_else(|| format!("{flag} needs a value"))?;
                if flag == "--home" {
                    home = Some(PathBuf::from(value));
                } else {
                    team = Some(value);
                }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Config {
        home,
        team: team.unwrap_or_else(|| "default".into()),
    })
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cfg = match parse(
        std::env::args().skip(1),
        std::env::var("RTEAMS_HOME").ok(),
        std::env::var("RTEAMS_TEAM").ok(),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rteams: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let server = match &cfg.home {
        Some(home) => match FileStore::open(home, &cfg.team) {
            Ok(store) => {
                eprintln!("rteams: state in {}", store.dir().display());
                Server::with_store(store)
            }
            Err(e) => {
                eprintln!("rteams: cannot open store: {e}");
                return ExitCode::FAILURE;
            }
        },
        None => Server::new(),
    };
    match server.serve_stdio().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("rteams: {e}");
            ExitCode::FAILURE
        }
    }
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
    fn defaults_to_memory_and_default_team() {
        let c = p(&[], None, None).unwrap();
        assert_eq!((c.home, c.team.as_str()), (None, "default"));
    }

    #[test]
    fn env_then_flags_override() {
        let c = p(&["serve"], Some("/e"), Some("et")).unwrap();
        assert_eq!(c.home, Some("/e".into()));
        assert_eq!(c.team, "et");
        let c = p(&["--home", "/f", "--team=ft"], Some("/e"), Some("et")).unwrap();
        assert_eq!(c.home, Some("/f".into()));
        assert_eq!(c.team, "ft");
    }

    #[test]
    fn empty_env_is_unset_and_bad_args_fail() {
        assert_eq!(p(&[], Some(""), Some("")).unwrap().home, None);
        assert!(p(&["--home"], None, None).is_err());
        assert!(p(&["--nope"], None, None).is_err());
    }
}
