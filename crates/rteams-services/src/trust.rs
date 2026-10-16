//! Workspace trust. Non-interactive agent runs never show the "Do you trust this folder?"
//! dialog, so trust is written ahead of time: Claude's `~/.claude.json`
//! (`projects["<path>"].hasTrustDialogAccepted = true`, every other key preserved) and, for
//! Codex, `-c projects."<path>".trust_level="trusted"` launch arguments.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use crate::atomic;

const MAX_BYTES: u64 = 1024 * 1024;
static LOCK: Mutex<()> = Mutex::new(());

/// `~/.claude.json`.
pub fn claude_json_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude.json"))
}

/// A path is persistable when it is absolute and neither the filesystem root nor the home dir.
pub fn persistable(path: &Path) -> bool {
    path.is_absolute() && path.parent().is_some() && dirs::home_dir().is_none_or(|h| h != path)
}

fn key_for(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
}

/// Mark `project` trusted in the Claude config at `file`. Returns `Ok(true)` when it changed.
pub fn ensure_claude_trusted_at(file: &Path, project: &Path) -> Result<bool, String> {
    if !persistable(project) {
        return Err(format!(
            "{} cannot be trusted persistently",
            project.display()
        ));
    }
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut root: Value = match std::fs::metadata(file) {
        Ok(m) if m.len() > MAX_BYTES => return Err("~/.claude.json is too large".into()),
        Ok(_) => {
            let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
            serde_json::from_slice(&bytes).map_err(|e| format!("~/.claude.json: {e}"))?
        }
        Err(_) => json!({}),
    };
    let obj = root
        .as_object_mut()
        .ok_or("~/.claude.json is not an object")?;
    let projects = obj
        .entry("projects")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("`projects` is not an object")?;
    let entry = projects
        .entry(key_for(project))
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("project entry is not an object")?;
    if entry.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) {
        return Ok(false);
    }
    entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    atomic::write_json(file, &root).map_err(|e| e.to_string())?;
    Ok(true)
}

/// `ensure_claude_trusted_at` for the real `~/.claude.json`.
pub fn ensure_claude_trusted(project: &Path) -> Result<bool, String> {
    let f = claude_json_path().ok_or("no home directory")?;
    ensure_claude_trusted_at(&f, project)
}

pub fn is_claude_trusted_at(file: &Path, project: &Path) -> bool {
    atomic::read_json(file)
        .and_then(|v| {
            v.pointer(&format!(
                "/projects/{}/hasTrustDialogAccepted",
                escape_pointer(&key_for(project))
            ))
            .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn escape_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Arguments that make Codex trust `project` for this launch.
pub fn codex_trust_args(project: &Path) -> Vec<String> {
    let p = key_for(project).replace('\\', "\\\\").replace('"', "\\\"");
    vec![
        "-c".into(),
        format!("projects.\"{p}\".trust_level=\"trusted\""),
    ]
}

/// Text of the launch notice shown once per project.
pub fn launch_notice(project: &Path) -> String {
    format!("Trusting {} for this team's agents", project.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_other_keys_and_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join(".claude.json");
        let proj = d.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            &file,
            r#"{"numStartups":3,"theme":"dark","projects":{"/other":{"allowedTools":["x"],"hasTrustDialogAccepted":false}}}"#,
        )
        .unwrap();
        assert!(ensure_claude_trusted_at(&file, &proj).unwrap());
        assert!(!ensure_claude_trusted_at(&file, &proj).unwrap());
        let v = atomic::read_json(&file).unwrap();
        assert_eq!(v["numStartups"], 3);
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["projects"]["/other"]["allowedTools"][0], "x");
        assert_eq!(v["projects"]["/other"]["hasTrustDialogAccepted"], false);
        assert_eq!(
            v["projects"][key_for(&proj)]["hasTrustDialogAccepted"],
            true
        );
        assert!(is_claude_trusted_at(&file, &proj));
    }

    #[test]
    fn creates_file_and_refuses_root_and_home() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("new.json");
        assert!(ensure_claude_trusted_at(&file, Path::new("/")).is_err());
        if let Some(h) = dirs::home_dir() {
            assert!(ensure_claude_trusted_at(&file, &h).is_err());
        }
        assert!(ensure_claude_trusted_at(&file, Path::new("rel")).is_err());
        assert!(ensure_claude_trusted_at(&file, d.path()).unwrap());
    }

    #[test]
    fn rejects_corrupt_without_overwriting() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("c.json");
        std::fs::write(&file, "{not json").unwrap();
        assert!(ensure_claude_trusted_at(&file, d.path()).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{not json");
    }

    #[test]
    fn codex_args() {
        let a = codex_trust_args(Path::new("/no/such/p\"q"));
        assert_eq!(a[0], "-c");
        assert_eq!(a[1], "projects.\"/no/such/p\\\"q\".trust_level=\"trusted\"");
    }
}
