//! Installed Claude Code plugins and their lifecycle, driven through the `claude plugin` CLI
//! (`list --json`, `install`, `uninstall`, `enable`, `disable`, `update`). Only subcommands that
//! exist in `claude plugin --help` are used.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::cli;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPlugin {
    /// `name@marketplace`.
    pub id: String,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    /// `user` | `project` | `local`.
    pub scope: String,
    pub enabled: bool,
    pub install_path: Option<PathBuf>,
    pub project_path: Option<PathBuf>,
}

/// Parse `claude plugin list --json` output.
pub fn parse_list(json: &str) -> Vec<InstalledPlugin> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    // Tolerate both a bare array and `{ "installed": [...] }` / `{ "plugins": [...] }`.
    let arr = v
        .as_array()
        .or_else(|| v["installed"].as_array())
        .or_else(|| v["plugins"].as_array());
    arr.into_iter()
        .flatten()
        .filter_map(|p| {
            let id = p["id"].as_str()?.to_string();
            let (name, marketplace) = id
                .split_once('@')
                .map_or((id.clone(), String::new()), |(n, m)| {
                    (n.to_string(), m.to_string())
                });
            Some(InstalledPlugin {
                name,
                marketplace,
                version: p["version"].as_str().map(str::to_string),
                scope: p["scope"].as_str().unwrap_or("user").to_string(),
                enabled: p["enabled"].as_bool().unwrap_or(true),
                install_path: p["installPath"].as_str().map(PathBuf::from),
                project_path: p["projectPath"].as_str().map(PathBuf::from),
                id,
            })
        })
        .collect()
}

pub const SCOPES: [&str; 3] = ["user", "project", "local"];

fn check_scope(scope: &str) -> Result<(), String> {
    if SCOPES.contains(&scope) {
        Ok(())
    } else {
        Err(format!("Invalid scope: {scope}"))
    }
}

/// Plugin ids are `name` or `name@marketplace`; reject anything that could be read as a flag.
pub fn validate_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 200
        && !id.starts_with('-')
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '@'));
    if ok {
        Ok(())
    } else {
        Err(format!("Invalid plugin id: {id}"))
    }
}

/// Argument vectors, exposed for tests.
pub fn install_args(id: &str, scope: &str) -> Vec<String> {
    ["plugin", "install", "--scope", scope, id]
        .map(String::from)
        .to_vec()
}

pub fn uninstall_args(id: &str, scope: &str) -> Vec<String> {
    ["plugin", "uninstall", "--scope", scope, id]
        .map(String::from)
        .to_vec()
}

pub fn toggle_args(id: &str, scope: &str, enable: bool) -> Vec<String> {
    [
        "plugin",
        if enable { "enable" } else { "disable" },
        "--scope",
        scope,
        id,
    ]
    .map(String::from)
    .to_vec()
}

/// What the plugin CLI is run with.
#[derive(Debug, Clone)]
pub struct PluginCli {
    pub bin: PathBuf,
    /// Project directory for `project` / `local` scope.
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}

impl PluginCli {
    pub fn detect(cwd: Option<&Path>) -> Option<PluginCli> {
        Some(PluginCli {
            bin: cli::find_claude()?,
            cwd: cwd.map(Path::to_path_buf),
            env: Vec::new(),
        })
    }

    fn run(&self, args: Vec<String>, secs: u64) -> Result<String, String> {
        let o = cli::run(
            &self.bin,
            &args,
            self.cwd.as_deref(),
            &self.env,
            Duration::from_secs(secs),
        );
        if o.ok {
            Ok(o.stdout)
        } else {
            Err(o.error_text())
        }
    }

    pub fn list_installed(&self) -> Result<Vec<InstalledPlugin>, String> {
        Ok(parse_list(&self.run(
            ["plugin", "list", "--json"].map(String::from).to_vec(),
            30,
        )?))
    }

    pub fn install(&self, id: &str, scope: &str) -> Result<String, String> {
        validate_id(id)?;
        check_scope(scope)?;
        self.run(install_args(id, scope), 120)
    }

    pub fn uninstall(&self, id: &str, scope: &str) -> Result<String, String> {
        validate_id(id)?;
        check_scope(scope)?;
        self.run(uninstall_args(id, scope), 60)
    }

    pub fn set_enabled(&self, id: &str, scope: &str, enable: bool) -> Result<String, String> {
        validate_id(id)?;
        check_scope(scope)?;
        self.run(toggle_args(id, scope, enable), 60)
    }

    pub fn update(&self, id: &str) -> Result<String, String> {
        validate_id(id)?;
        self.run(["plugin", "update", id].map(String::from).to_vec(), 120)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cli_list() {
        let j = r#"[
          {"id":"a-skills@market","version":"2.9.0","scope":"user","enabled":true,"installPath":"/x/a"},
          {"id":"ponytail@ponytail","version":"4.8.3","scope":"project","enabled":false,"projectPath":"/p"},
          {"nope":1}
        ]"#;
        let l = parse_list(j);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].name, "a-skills");
        assert_eq!(l[0].marketplace, "market");
        assert!(l[0].enabled);
        assert!(!l[1].enabled);
        assert_eq!(l[1].project_path.as_deref(), Some(Path::new("/p")));
        assert!(parse_list("not json").is_empty());
        assert_eq!(parse_list(r#"{"installed":[{"id":"x@y"}]}"#).len(), 1);
    }

    #[test]
    fn builds_cli_args_and_validates() {
        assert_eq!(
            install_args("x@y", "project"),
            ["plugin", "install", "--scope", "project", "x@y"]
        );
        assert_eq!(toggle_args("x", "user", false)[1], "disable");
        assert!(validate_id("--help").is_err());
        assert!(validate_id("a b").is_err());
        assert!(validate_id("name@market").is_ok());
        let c = PluginCli {
            bin: PathBuf::from("/no/such/claude"),
            cwd: None,
            env: vec![],
        };
        assert!(c.install("x", "weird").is_err());
        assert!(c
            .install("x@y", "user")
            .unwrap_err()
            .contains("cannot start"));
    }

    #[cfg(unix)]
    #[test]
    fn runs_through_a_fake_claude() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("claude");
        std::fs::write(
            &p,
            "#!/bin/sh\nif [ \"$2\" = list ]; then echo '[{\"id\":\"z@m\",\"scope\":\"user\"}]'; else echo \"$@\"; fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let c = PluginCli {
            bin: p,
            cwd: None,
            env: vec![],
        };
        assert_eq!(c.list_installed().unwrap()[0].id, "z@m");
        assert!(c
            .install("z@m", "user")
            .unwrap()
            .contains("plugin install --scope user z@m"));
    }
}
