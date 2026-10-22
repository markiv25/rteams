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
        if let Some(("host", rest)) = split_directive(line).as_ref().map(|(k, v)| (&**k, v)) {
            for h in rest.split_whitespace() {
                if !h.contains(['*', '?', '!']) && !out.iter().any(|x| x == h) {
                    out.push(h.to_string());
                }
            }
        }
    }
    out
}

/// `Key value` or `Key=value` -> (lowercased key, value). Comments and blanks give None.
fn split_directive(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let i = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let key = line[..i].to_ascii_lowercase();
    let val = line[i..].trim_start_matches(|c: char| c.is_whitespace() || c == '=');
    let val = val.trim().trim_matches('"');
    Some((key, val.to_string()))
}

/// One `Host` section of an ssh config (all of its patterns share the settings).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostSection {
    pub patterns: Vec<String>,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

/// What `ssh <alias>` would use, merged from every matching section (first value wins).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedHost {
    pub alias: String,
    pub host_name: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
}

/// Split config text into `Host` sections (settings before the first `Host` are ignored).
pub fn parse_sections(text: &str) -> Vec<HostSection> {
    let mut out: Vec<HostSection> = Vec::new();
    for line in text.lines() {
        let Some((k, v)) = split_directive(line) else {
            continue;
        };
        if k == "host" {
            out.push(HostSection {
                patterns: v.split_whitespace().map(str::to_string).collect(),
                ..Default::default()
            });
            continue;
        }
        if k == "match" {
            // Match blocks are not evaluated; keep their settings out of Host sections.
            out.push(HostSection::default());
            continue;
        }
        let Some(sec) = out.last_mut() else { continue };
        match k.as_str() {
            "hostname" => sec.host_name.get_or_insert(v),
            "user" => sec.user.get_or_insert(v),
            "identityfile" => sec.identity_file.get_or_insert(v),
            "port" => {
                if sec.port.is_none() {
                    sec.port = v.parse().ok();
                }
                continue;
            }
            _ => continue,
        };
    }
    out
}

/// Glob match with `*` and `?`.
fn glob(pat: &str, s: &str) -> bool {
    let (p, s): (Vec<char>, Vec<char>) = (pat.chars().collect(), s.chars().collect());
    fn go(p: &[char], s: &[char]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|i| go(rest, &s[i..])),
            Some(('?', rest)) => !s.is_empty() && go(rest, &s[1..]),
            Some((c, rest)) => s.first() == Some(c) && go(rest, &s[1..]),
        }
    }
    go(&p, &s)
}

fn section_matches(sec: &HostSection, alias: &str) -> bool {
    let mut hit = false;
    for p in &sec.patterns {
        if let Some(neg) = p.strip_prefix('!') {
            if glob(neg, alias) {
                return false;
            }
        } else if glob(p, alias) {
            hit = true;
        }
    }
    hit
}

/// Resolve `alias`. None when no section names it explicitly (a lone `Host *` does not make an
/// unknown alias "known").
pub fn resolve_host(text: &str, alias: &str) -> Option<ResolvedHost> {
    let secs = parse_sections(text);
    let matching: Vec<&HostSection> = secs.iter().filter(|s| section_matches(s, alias)).collect();
    let explicit = matching
        .iter()
        .any(|s| s.patterns.iter().any(|p| p == alias));
    if !explicit {
        return None;
    }
    let mut r = ResolvedHost {
        alias: alias.to_string(),
        ..Default::default()
    };
    for s in matching {
        r.host_name = r.host_name.or_else(|| s.host_name.clone());
        r.user = r.user.or_else(|| s.user.clone());
        r.port = r.port.or(s.port);
        r.identity_file = r.identity_file.or_else(|| s.identity_file.clone());
    }
    Some(r)
}

const MAX_INCLUDE_DEPTH: u8 = 4;

/// Read an ssh config file, expanding `Include` lines in place (relative paths resolve against
/// `ssh_dir`, `~/` against `user_home`; `*`/`?` allowed in the last path segment). Depth-limited.
pub fn read_config_expanded(path: &Path, ssh_dir: &Path, user_home: &Path) -> String {
    expand(path, ssh_dir, user_home, 0)
}

fn expand(path: &Path, ssh_dir: &Path, user_home: &Path, depth: u8) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut out = String::new();
    for line in text.lines() {
        match split_directive(line) {
            Some((k, v)) if k == "include" && depth < MAX_INCLUDE_DEPTH => {
                for spec in v.split_whitespace() {
                    for f in include_files(spec, ssh_dir, user_home) {
                        out.push_str(&expand(&f, ssh_dir, user_home, depth + 1));
                        out.push('\n');
                    }
                }
            }
            Some((k, _)) if k == "include" => {}
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

fn include_files(spec: &str, ssh_dir: &Path, user_home: &Path) -> Vec<PathBuf> {
    let full = if let Some(rest) = spec.strip_prefix("~/") {
        user_home.join(rest)
    } else if Path::new(spec).is_absolute() {
        PathBuf::from(spec)
    } else {
        ssh_dir.join(spec)
    };
    let (Some(dir), Some(name)) = (full.parent(), full.file_name().and_then(|n| n.to_str())) else {
        return vec![];
    };
    if !name.contains(['*', '?']) {
        return vec![full];
    }
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| glob(name, n)))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    v.sort();
    v
}

/// Full text of `<user_home>/.ssh/config` with includes expanded ("" when absent).
pub fn load_user_config(user_home: &Path) -> String {
    let dir = user_home.join(".ssh");
    read_config_expanded(&dir.join("config"), &dir, user_home)
}

/// A profile that defers to the ssh config for `alias` (auth `Auto`), pre-filled from it.
pub fn profile_from_host(r: &ResolvedHost) -> SshProfile {
    SshProfile {
        id: String::new(),
        name: r.alias.clone(),
        host: r.alias.clone(),
        port: r.port.unwrap_or(22),
        user: r.user.clone().unwrap_or_default(),
        auth: Auth::Auto,
        key_path: r.identity_file.clone().unwrap_or_default(),
    }
}

// ---- last connection: <home>/ssh-last.json ----

pub fn save_last_connection(home: &Path, profile_id: &str) -> std::io::Result<()> {
    atomic::write_json(
        &home.join("ssh-last.json"),
        &serde_json::json!({ "profile_id": profile_id }),
    )
}

pub fn load_last_connection(home: &Path) -> Option<String> {
    atomic::read_json(&home.join("ssh-last.json"))?
        .get("profile_id")?
        .as_str()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
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

    const CFG: &str = "Host *\n  User fallback\n  Port 2200\nHost web db\n  HostName 10.0.0.5\n  IdentityFile ~/.ssh/id_web\nHost web\n  User deploy\n  Port=2022\nHost prod-*\n  User ops\nHost *.corp !bad.corp\n  User corp\n";

    #[test]
    fn multiple_hosts_per_line_and_wildcards() {
        assert_eq!(parse_config_hosts(CFG), vec!["web", "db"]);
        let web = resolve_host(CFG, "web").unwrap();
        assert_eq!(web.host_name.as_deref(), Some("10.0.0.5"));
        assert_eq!(web.user.as_deref(), Some("fallback"));
        assert_eq!(web.port, Some(2200));
        assert_eq!(web.identity_file.as_deref(), Some("~/.ssh/id_web"));
        let db = resolve_host(CFG, "db").unwrap();
        assert_eq!(db.host_name.as_deref(), Some("10.0.0.5"));
        assert_eq!(db.port, Some(2200));
    }

    #[test]
    fn resolve_requires_explicit_host_and_honors_negation() {
        assert!(resolve_host(CFG, "prod-1").is_none());
        assert!(resolve_host(CFG, "nope").is_none());
        assert!(resolve_host("Host *.corp !bad.corp\n User c\n", "a.corp").is_none());
        let t = "Host a.corp bad.corp\n User x\nHost *.corp !bad.corp\n User c\n";
        assert_eq!(
            resolve_host(t, "a.corp").unwrap().user.as_deref(),
            Some("x")
        );
        assert!(section_matches(
            &parse_sections("Host *.corp !bad.corp")[0],
            "a.corp"
        ));
        assert!(!section_matches(
            &parse_sections("Host *.corp !bad.corp")[0],
            "bad.corp"
        ));
    }

    #[test]
    fn include_expands_relative_glob_and_tilde_without_loops() {
        let d = tempfile::tempdir().unwrap();
        let ssh = d.path().join(".ssh");
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "Include conf.d/*.conf\nInclude ~/.ssh/extra\nInclude config\nHost main\n HostName m.example\n",
        )
        .unwrap();
        std::fs::write(
            ssh.join("conf.d/a.conf"),
            "Host alpha\n  User al\n  Port 99\n",
        )
        .unwrap();
        std::fs::write(ssh.join("conf.d/b.txt"), "Host skipped\n").unwrap();
        std::fs::write(ssh.join("extra"), "Host beta\n  HostName b.example\n").unwrap();
        let text = load_user_config(d.path());
        assert_eq!(parse_config_hosts(&text), vec!["alpha", "beta", "main"]);
        let a = resolve_host(&text, "alpha").unwrap();
        assert_eq!((a.user.as_deref(), a.port), (Some("al"), Some(99)));
        assert_eq!(load_user_config(&d.path().join("missing")), "");
    }

    #[test]
    fn profile_from_resolved_defers_to_config() {
        let r = resolve_host(CFG, "web").unwrap();
        let p = profile_from_host(&r);
        assert_eq!((p.host.as_str(), p.port, p.auth), ("web", 2200, Auth::Auto));
        assert!(p.validate().is_ok());
    }

    #[test]
    fn last_connection_round_trip() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load_last_connection(d.path()), None);
        save_last_connection(d.path(), "abc").unwrap();
        assert_eq!(load_last_connection(d.path()).as_deref(), Some("abc"));
    }
}
