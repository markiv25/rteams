//! Opt-in SSH remote access by shelling out to the system `ssh` (BatchMode, so it never
//! prompts; password auth is unsupported). Profiles live in `<home>/ssh-profiles.json`.
//! Command construction is pure and unit-tested; only `run` touches the network.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::atomic;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Auth {
    #[default]
    Agent,
    /// Defer to `~/.ssh/config`.
    Auto,
    /// Not supported by BatchMode; kept for catalog parity.
    Password,
    PrivateKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    pub key_path: String,
}

impl Default for SshProfile {
    fn default() -> Self {
        SshProfile {
            id: String::new(),
            name: String::new(),
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: Auth::Agent,
            key_path: String::new(),
        }
    }
}

pub fn profiles_path(home: &Path) -> PathBuf {
    home.join("ssh-profiles.json")
}

pub fn load_profiles(home: &Path) -> Vec<SshProfile> {
    atomic::read_json(&profiles_path(home))
        .and_then(|v| serde_json::from_value(v.get("profiles").cloned().unwrap_or(v)).ok())
        .unwrap_or_default()
}

pub fn save_profiles(home: &Path, profiles: &[SshProfile]) -> std::io::Result<()> {
    let v = serde_json::json!({ "schema_version": 1, "profiles": profiles });
    atomic::write_json(&profiles_path(home), &v)
}

impl SshProfile {
    /// Reject values that could be parsed by `ssh` as options or smuggle shell syntax.
    pub fn validate(&self) -> Result<(), String> {
        let bad = |s: &str| {
            s.is_empty()
                || s.starts_with('-')
                || s.chars().any(|c| c.is_whitespace() || c.is_control())
        };
        if bad(&self.host) {
            return Err("invalid host".into());
        }
        if !self.user.is_empty() && bad(&self.user) {
            return Err("invalid user".into());
        }
        if self.port == 0 {
            return Err("invalid port".into());
        }
        if self.auth == Auth::PrivateKey && self.key_path.trim().is_empty() {
            return Err("private key path required".into());
        }
        if self.auth == Auth::Password {
            return Err(
                "password authentication is not supported (use an SSH agent or key)".into(),
            );
        }
        Ok(())
    }

    /// `user@host` or `host`.
    pub fn target(&self) -> String {
        if self.user.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.user, self.host)
        }
    }
}

/// Arguments (without the `ssh` program) up to and including the target.
pub fn base_args(p: &SshProfile) -> Vec<String> {
    let mut a: Vec<String> = ["-o", "BatchMode=yes", "-o", "ConnectTimeout=5"]
        .map(String::from)
        .into();
    if p.port != 22 {
        a.extend(["-p".into(), p.port.to_string()]);
    }
    if p.auth == Auth::PrivateKey {
        a.extend([
            "-i".into(),
            p.key_path.clone(),
            "-o".into(),
            "IdentitiesOnly=yes".into(),
        ]);
    }
    a.push("--".into());
    a.push(p.target());
    a
}

/// `ssh ... host true`.
pub fn test_args(p: &SshProfile) -> Vec<String> {
    let mut a = base_args(p);
    a.push("true".into());
    a
}

/// Single-quote for a POSIX shell.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `ssh ... host ls -1 -- '<dir>'` (a leading `~/` stays unquoted so the remote shell expands it).
pub fn ls_args(p: &SshProfile, dir: &str) -> Vec<String> {
    let mut a = base_args(p);
    let quoted = match dir.strip_prefix("~/") {
        Some(rest) => format!("~/{}", shell_quote(rest)),
        None => shell_quote(dir),
    };
    a.push(format!("ls -1 -- {quoted}"));
    a
}

/// Run `ssh` with `args`, killing it after `timeout`. Returns stdout on exit status 0.
pub fn run(args: &[String], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new("ssh")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run ssh: {e}"))?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                let mut err = String::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut out);
                }
                if let Some(mut s) = child.stderr.take() {
                    let _ = s.read_to_string(&mut err);
                }
                return if status.success() {
                    Ok(out)
                } else {
                    Err(err.trim().to_string())
                };
            }
            Ok(None) if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(40)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

pub fn test_connection(p: &SshProfile) -> Result<(), String> {
    p.validate()?;
    run(&test_args(p), Duration::from_secs(10)).map(|_| ())
}

pub fn list_remote_dir(p: &SshProfile, dir: &str) -> Result<Vec<String>, String> {
    p.validate()?;
    let out = run(&ls_args(p, dir), Duration::from_secs(15))?;
    Ok(out
        .lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect())
}

/// Host aliases from `~/.ssh/config` text (`Host a b`), skipping wildcards and negations.
pub fn parse_config_hosts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.splitn(2, char::is_whitespace);
        if it.next().is_some_and(|k| k.eq_ignore_ascii_case("host")) {
            for h in it.next().unwrap_or("").split_whitespace() {
                if !h.contains(['*', '?', '!']) && !out.iter().any(|x| x == h) {
                    out.push(h.to_string());
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> SshProfile {
        SshProfile {
            host: "box".into(),
            user: "me".into(),
            port: 2222,
            ..Default::default()
        }
    }

    #[test]
    fn command_construction() {
        let a = test_args(&p());
        assert_eq!(
            a.join(" "),
            "-o BatchMode=yes -o ConnectTimeout=5 -p 2222 -- me@box true"
        );
        let mut k = p();
        k.port = 22;
        k.auth = Auth::PrivateKey;
        k.key_path = "/k/id".into();
        assert!(base_args(&k)
            .join(" ")
            .contains("-i /k/id -o IdentitiesOnly=yes"));
        assert!(!base_args(&k).contains(&"-p".to_string()));
    }

    #[test]
    fn ls_quotes_remote_path() {
        let a = ls_args(&p(), "~/.claude/projects");
        assert_eq!(a.last().unwrap(), "ls -1 -- ~/'.claude/projects'");
        assert_eq!(
            ls_args(&p(), "/a b'c").last().unwrap(),
            "ls -1 -- '/a b'\\''c'"
        );
    }

    #[test]
    fn validation_blocks_option_injection() {
        let mut x = p();
        x.host = "-oProxyCommand=evil".into();
        assert!(x.validate().is_err());
        x.host = "a b".into();
        assert!(x.validate().is_err());
        x.host = "ok".into();
        assert!(x.validate().is_ok());
        x.auth = Auth::Password;
        assert!(x.validate().is_err());
        x.auth = Auth::PrivateKey;
        assert!(x.validate().is_err());
    }

    #[test]
    fn profiles_round_trip_in_temp_home() {
        let d = tempfile::tempdir().unwrap();
        assert!(load_profiles(d.path()).is_empty());
        let mut a = p();
        a.id = "1".into();
        save_profiles(d.path(), &[a.clone()]).unwrap();
        assert_eq!(load_profiles(d.path()), vec![a]);
    }

    #[test]
    fn config_host_parser() {
        let t = "Host *\n  User x\nHost dev  prod-*  staging\nhost Dev\n";
        assert_eq!(parse_config_hosts(t), vec!["dev", "staging", "Dev"]);
    }
}
