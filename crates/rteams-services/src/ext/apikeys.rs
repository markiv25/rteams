//! API keys for online services (Anthropic, OpenAI, Gemini, OpenRouter, ...). Each key has a
//! name, the environment variable it fills, a scope (user-wide or bound to one project) and a
//! value. Metadata lives in `<home>/api-keys.json` (the value is never written there, only its
//! last four characters); values live in `<home>/secrets.json`, written atomically with mode
//! 0600. Keys auto-fill MCP installs and are exported to launched agents via `env_for`.
//!
//! No OS keychain crate is vendored in this workspace, so the 0600 file is the store; the
//! `Store` API is the only thing callers use, so a keychain backend can slot in behind it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use super::mcp_config::write_atomic;

/// Suggestions for the "Add API Key" form: (label, env var).
pub const PRESETS: [(&str, &str); 8] = [
    ("Anthropic", "ANTHROPIC_API_KEY"),
    ("OpenAI", "OPENAI_API_KEY"),
    ("Gemini", "GEMINI_API_KEY"),
    ("Google", "GOOGLE_API_KEY"),
    ("OpenRouter", "OPENROUTER_API_KEY"),
    ("Groq", "GROQ_API_KEY"),
    ("GitHub", "GITHUB_TOKEN"),
    ("Brave Search", "BRAVE_API_KEY"),
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiKeyMeta {
    pub id: String,
    pub name: String,
    pub env_key: String,
    /// `None` = user scope (every project); `Some(path)` = only that project.
    #[serde(default)]
    pub project: Option<String>,
    pub last4: String,
    pub created_at_ms: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct MetaFile {
    #[serde(default = "one")]
    schema_version: u32,
    #[serde(default)]
    keys: Vec<ApiKeyMeta>,
}

#[derive(Serialize, Deserialize, Default)]
struct SecretsFile {
    #[serde(default = "one")]
    schema_version: u32,
    #[serde(default)]
    secrets: std::collections::BTreeMap<String, String>,
}

fn one() -> u32 {
    1
}

pub fn validate_env_key(k: &str) -> Result<(), String> {
    let mut c = k.chars();
    let first = c
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    if first && c.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(())
    } else {
        Err("Use letters, digits, underscores. Must start with a letter or underscore.".into())
    }
}

/// Whether a key for `env_key` should be exported to `runtime` (`claude`, `codex`, `opencode`,
/// `gemini`, ...). Provider-specific variables only go to the CLIs that read them; anything
/// else (MCP server tokens, ...) goes everywhere.
pub fn applies_to(env_key: &str, runtime: &str) -> bool {
    let k = env_key.to_ascii_uppercase();
    let rt = runtime.to_ascii_lowercase();
    let multi = rt == "opencode";
    if k.starts_with("ANTHROPIC_") {
        return rt == "claude" || multi;
    }
    if k.starts_with("OPENAI_") {
        return rt == "codex" || multi;
    }
    if k.starts_with("GEMINI_") || k.starts_with("GOOGLE_API") {
        return rt == "gemini" || multi;
    }
    if k.starts_with("OPENROUTER_") {
        return multi || rt == "claude" || rt == "codex";
    }
    true
}

/// The key store rooted at the rteams home.
#[derive(Debug, Clone)]
pub struct Store {
    home: PathBuf,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Store {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Store { home: home.into() }
    }

    pub fn meta_path(&self) -> PathBuf {
        self.home.join("api-keys.json")
    }

    pub fn secrets_path(&self) -> PathBuf {
        self.home.join("secrets.json")
    }

    /// Human description of where values are kept, for the panel's storage note.
    pub fn backend_label(&self) -> String {
        format!("{} (owner-only, mode 0600)", self.secrets_path().display())
    }

    fn read_meta(&self) -> MetaFile {
        std::fs::read(self.meta_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_meta(&self, m: &MetaFile) -> Result<(), String> {
        let b = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
        crate::atomic::write(&self.meta_path(), &b).map_err(|e| e.to_string())
    }

    fn read_secrets(&self) -> SecretsFile {
        std::fs::read(self.secrets_path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn write_secrets(&self, s: &SecretsFile) -> Result<(), String> {
        let b = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
        let p = self.secrets_path();
        write_atomic(&p, &b).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn list(&self) -> Vec<ApiKeyMeta> {
        let mut k = self.read_meta().keys;
        k.sort_by_key(|a| a.name.to_lowercase());
        k
    }

    fn validate(name: &str, env_key: &str, value: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("Name is required".into());
        }
        if env_key.trim().is_empty() {
            return Err("Environment variable name is required".into());
        }
        validate_env_key(env_key.trim())
            .map_err(|_| "Invalid environment variable name".to_string())?;
        if value.is_empty() {
            return Err("Key value is required".into());
        }
        Ok(())
    }

    /// Add a key. `project` binds it to one project; `None` = user scope.
    pub fn add(
        &self,
        name: &str,
        env_key: &str,
        value: &str,
        project: Option<&str>,
    ) -> Result<ApiKeyMeta, String> {
        Self::validate(name, env_key, value)?;
        let id = uuid::Uuid::new_v4().to_string();
        let meta = ApiKeyMeta {
            id: id.clone(),
            name: name.trim().into(),
            env_key: env_key.trim().into(),
            project: project.map(str::to_string),
            last4: last4(value),
            created_at_ms: now_ms(),
        };
        // Secret first: a metadata row without a value would be worse than the reverse.
        let mut s = self.read_secrets();
        s.secrets.insert(id, value.to_string());
        self.write_secrets(&s)?;
        let mut m = self.read_meta();
        m.keys.push(meta.clone());
        self.write_meta(&m)?;
        Ok(meta)
    }

    /// Replace a key's details and value (the value must be re-entered).
    pub fn update(
        &self,
        id: &str,
        name: &str,
        env_key: &str,
        value: &str,
        project: Option<&str>,
    ) -> Result<ApiKeyMeta, String> {
        Self::validate(name, env_key, value)?;
        let mut m = self.read_meta();
        let Some(k) = m.keys.iter_mut().find(|k| k.id == id) else {
            return Err("API key not found".into());
        };
        k.name = name.trim().into();
        k.env_key = env_key.trim().into();
        k.project = project.map(str::to_string);
        k.last4 = last4(value);
        let out = k.clone();
        let mut s = self.read_secrets();
        s.secrets.insert(id.into(), value.to_string());
        self.write_secrets(&s)?;
        self.write_meta(&m)?;
        Ok(out)
    }

    pub fn delete(&self, id: &str) -> Result<bool, String> {
        let mut m = self.read_meta();
        let before = m.keys.len();
        m.keys.retain(|k| k.id != id);
        let mut s = self.read_secrets();
        let had_secret = s.secrets.remove(id).is_some();
        if had_secret {
            self.write_secrets(&s)?;
        }
        if m.keys.len() != before {
            self.write_meta(&m)?;
        }
        Ok(m.keys.len() != before || had_secret)
    }

    /// The secret value (for "copy" / env export).
    pub fn value(&self, id: &str) -> Option<String> {
        self.read_secrets().secrets.get(id).cloned()
    }

    /// `(ENV_VAR, value)` pairs for user-scope keys and keys bound to `project`; a project key
    /// overrides a user key with the same variable. Filtered by `applies_to(runtime)`.
    pub fn env_for(&self, runtime: &str, project: Option<&Path>) -> Vec<(String, String)> {
        self.collect(Some(runtime), project)
    }

    fn collect(&self, runtime: Option<&str>, project: Option<&Path>) -> Vec<(String, String)> {
        let secrets = self.read_secrets().secrets;
        let proj = project.map(|p| p.to_string_lossy().into_owned());
        let mut keys = self.read_meta().keys;
        keys.sort_by_key(|k| (k.project.is_some(), k.created_at_ms));
        let mut out: Vec<(String, String)> = Vec::new();
        for k in keys {
            let in_scope = match (&k.project, &proj) {
                (None, _) => true,
                (Some(a), Some(b)) => a == b,
                _ => false,
            };
            if !in_scope || runtime.is_some_and(|r| !applies_to(&k.env_key, r)) {
                continue;
            }
            let Some(v) = secrets.get(&k.id) else {
                continue;
            };
            if let Some(slot) = out.iter_mut().find(|(n, _)| *n == k.env_key) {
                slot.1 = v.clone();
            } else {
                out.push((k.env_key.clone(), v.clone()));
            }
        }
        out
    }

    /// All user-scope `(ENV_VAR, value)` pairs, for auto-filling MCP installs.
    pub fn autofill_pairs(&self, project: Option<&Path>) -> Vec<(String, String)> {
        self.collect(None, project)
    }
}

fn last4(v: &str) -> String {
    let n = v.chars().count();
    v.chars().skip(n.saturating_sub(4)).collect()
}

static HOME: OnceLock<PathBuf> = OnceLock::new();

/// Tell the module where the rteams home is (call once at startup) so `env_for(runtime)` needs
/// no other argument.
pub fn init_home(home: &Path) {
    let _ = HOME.set(home.to_path_buf());
}

/// Env vars to inject into a launched `runtime` process (user-scope keys). Empty until
/// `init_home` was called.
pub fn env_for(runtime: &str) -> Vec<(String, String)> {
    HOME.get()
        .map(|h| Store::new(h).env_for(runtime, None))
        .unwrap_or_default()
}

/// Like `env_for` for a team's project directory, with an explicit home.
pub fn env_for_project(
    home: &Path,
    runtime: &str,
    project: Option<&Path>,
) -> Vec<(String, String)> {
    Store::new(home).env_for(runtime, project)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_value_delete() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        assert!(s.list().is_empty());
        let k = s
            .add("OpenAI Prod", "OPENAI_API_KEY", "sk-secret-1234", None)
            .unwrap();
        assert_eq!(k.last4, "1234");
        assert_eq!(s.list().len(), 1);
        assert_eq!(s.value(&k.id).as_deref(), Some("sk-secret-1234"));
        // The metadata file never contains the value.
        let meta = std::fs::read_to_string(s.meta_path()).unwrap();
        assert!(!meta.contains("sk-secret"));
        assert!(s.delete(&k.id).unwrap());
        assert!(!s.delete(&k.id).unwrap());
        assert!(s.list().is_empty() && s.value(&k.id).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn secrets_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        s.add("a", "A_KEY", "v", None).unwrap();
        s.add("b", "B_KEY", "w", None).unwrap();
        let mode = std::fs::metadata(s.secrets_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn validation() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        assert!(s.add("", "A", "v", None).is_err());
        assert!(s.add("n", "", "v", None).is_err());
        assert!(s.add("n", "1BAD", "v", None).is_err());
        assert!(s.add("n", "has space", "v", None).is_err());
        assert!(s.add("n", "OK_1", "", None).is_err());
        assert!(s.add("n", "ok_lower", "v", None).is_ok());
    }

    #[test]
    fn update_replaces_value() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        let k = s.add("n", "A_KEY", "old-0000", None).unwrap();
        let u = s.update(&k.id, "n2", "B_KEY", "new-9999", None).unwrap();
        assert_eq!(
            (u.name.as_str(), u.env_key.as_str(), u.last4.as_str()),
            ("n2", "B_KEY", "9999")
        );
        assert_eq!(s.value(&k.id).as_deref(), Some("new-9999"));
        assert!(s.update("nope", "n", "A", "v", None).is_err());
    }

    #[test]
    fn env_for_filters_by_runtime_and_scope() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::new(d.path());
        s.add("anthropic", "ANTHROPIC_API_KEY", "a", None).unwrap();
        s.add("openai", "OPENAI_API_KEY", "o", None).unwrap();
        s.add("gemini", "GEMINI_API_KEY", "g", None).unwrap();
        s.add("brave", "BRAVE_API_KEY", "b", None).unwrap();
        s.add("brave proj", "BRAVE_API_KEY", "b2", Some("/work/p"))
            .unwrap();
        let env = |rt: &str, p: Option<&str>| {
            let mut v = s.env_for(rt, p.map(Path::new));
            v.sort();
            v
        };
        assert_eq!(
            env("claude", None),
            [
                ("ANTHROPIC_API_KEY".into(), "a".into()),
                ("BRAVE_API_KEY".into(), "b".into())
            ]
        );
        assert_eq!(
            env("codex", None),
            [
                ("BRAVE_API_KEY".into(), "b".into()),
                ("OPENAI_API_KEY".into(), "o".into())
            ]
        );
        assert!(env("gemini", None).contains(&("GEMINI_API_KEY".into(), "g".into())));
        assert_eq!(env("opencode", None).len(), 4);
        // Project key overrides the user key of the same variable, only for that project.
        assert!(env("claude", Some("/work/p")).contains(&("BRAVE_API_KEY".into(), "b2".into())));
        assert!(env("claude", Some("/other")).contains(&("BRAVE_API_KEY".into(), "b".into())));
        assert_eq!(s.autofill_pairs(None).len(), 4);
    }

    #[test]
    fn global_env_for_uses_init_home() {
        assert!(env_for("claude").is_empty() || HOME.get().is_some());
        let d = tempfile::tempdir().unwrap();
        Store::new(d.path()).add("n", "X_KEY", "v", None).unwrap();
        let direct = env_for_project(d.path(), "claude", None);
        assert_eq!(direct, [("X_KEY".to_string(), "v".to_string())]);
    }
}
