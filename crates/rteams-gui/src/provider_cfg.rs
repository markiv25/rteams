//! Provider settings that live outside the runtimes: per-model limit overrides (stored in
//! `AppConfig.extra` as `providers.model_limits.<runtime>.<model>`), the local provider scan
//! (Ollama / LM Studio) and the API-key connect helpers over the shared key store.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use rteams_services::config::AppConfig;
use rteams_services::ext::apikeys::Store;
use serde_json::{json, Map, Value};

static HOME: OnceLock<PathBuf> = OnceLock::new();

/// Tell the module where the rteams home is (call once at startup).
pub fn set_home(home: &Path) {
    let _ = HOME.set(home.to_path_buf());
}

pub fn home() -> Option<PathBuf> {
    HOME.get().cloned()
}

// --- model limits --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ModelLimits {
    /// Context window in tokens.
    pub context: Option<u32>,
    /// Max output tokens.
    pub max_output: Option<u32>,
}

impl ModelLimits {
    pub fn is_empty(&self) -> bool {
        self.context.is_none() && self.max_output.is_none()
    }
}

fn limits_key(runtime: &str, model: &str) -> String {
    format!("providers.model_limits.{runtime}.{model}")
}

pub fn get_limits(extra: &Map<String, Value>, runtime: &str, model: &str) -> ModelLimits {
    let Some(v) = extra.get(&limits_key(runtime, model)) else {
        return ModelLimits::default();
    };
    let n = |k: &str| {
        v.get(k)
            .and_then(Value::as_u64)
            .filter(|n| *n > 0)
            .and_then(|n| u32::try_from(n).ok())
    };
    ModelLimits {
        context: n("context"),
        max_output: n("max_output"),
    }
}

/// Store (or, when empty, remove) the override.
pub fn set_limits(extra: &mut Map<String, Value>, runtime: &str, model: &str, l: ModelLimits) {
    let key = limits_key(runtime, model);
    if l.is_empty() {
        extra.remove(&key);
    } else {
        extra.insert(
            key,
            json!({ "context": l.context, "max_output": l.max_output }),
        );
    }
}

/// Precedence: member / launch-dialog value first, the provider+model override only as a
/// fallback when that is unset.
pub fn effective_context(
    extra: &Map<String, Value>,
    runtime: &str,
    model: Option<&str>,
    explicit: Option<u32>,
) -> Option<u32> {
    explicit.or_else(|| model.and_then(|m| get_limits(extra, runtime, m).context))
}

/// Max-output override for a launch, if one is configured.
pub fn max_output(extra: &Map<String, Value>, runtime: &str, model: Option<&str>) -> Option<u32> {
    model.and_then(|m| get_limits(extra, runtime, m).max_output)
}

pub fn save_limits(runtime: &str, model: &str, l: ModelLimits) -> std::io::Result<()> {
    let Some(h) = home() else {
        return Ok(());
    };
    let mut cfg = AppConfig::load(&h);
    set_limits(&mut cfg.extra, runtime, model, l);
    cfg.save(&h)
}

// --- local providers -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalProvider {
    /// Runtime id (`ollama`, `lmstudio`).
    pub id: &'static str,
    pub label: &'static str,
    pub url: &'static str,
    pub running: bool,
    pub models: Vec<String>,
}

pub const LOCAL: [(&str, &str, &str, &str); 2] = [
    (
        "ollama",
        "Ollama",
        "http://127.0.0.1:11434",
        "http://127.0.0.1:11434/api/tags",
    ),
    (
        "lmstudio",
        "LM Studio",
        "http://127.0.0.1:1234",
        "http://127.0.0.1:1234/v1/models",
    ),
];

/// `{"models":[{"name":"llama3:8b"},...]}`
pub fn parse_ollama(json: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    names(&v, "models", "name")
}

/// `{"data":[{"id":"qwen2.5-coder"},...]}`
pub fn parse_lmstudio(json: &str) -> Vec<String> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    names(&v, "data", "id")
}

fn names(v: &Value, list: &str, field: &str) -> Vec<String> {
    v.get(list)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get(field).and_then(Value::as_str))
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Probe one local server with a short timeout (blocking; worker threads only).
pub fn scan_one(id: &str) -> LocalProvider {
    let &(id, label, url, probe) = LOCAL.iter().find(|l| l.0 == id).unwrap_or(&LOCAL[0]);
    let body = rteams_services::http::get_text(probe, Duration::from_secs(1)).ok();
    let models = body.as_deref().map_or_else(Vec::new, |b| {
        if id == "ollama" {
            parse_ollama(b)
        } else {
            parse_lmstudio(b)
        }
    });
    LocalProvider {
        id,
        label,
        url,
        running: body.is_some(),
        models,
    }
}

pub fn scan_local() -> Vec<LocalProvider> {
    let hs: Vec<_> = LOCAL
        .iter()
        .map(|l| {
            let id = l.0;
            std::thread::spawn(move || scan_one(id))
        })
        .collect();
    hs.into_iter().filter_map(|h| h.join().ok()).collect()
}

// --- API key connect -----------------------------------------------------------------------

/// The stored key for `env_key` (user scope), if connected: `(id, last4)`.
pub fn connected_key(home: &Path, env_key: &str) -> Option<(String, String)> {
    Store::new(home)
        .list()
        .into_iter()
        .find(|k| k.env_key == env_key && k.project.is_none())
        .map(|k| (k.id, k.last4))
}

/// Save (replace) the user-scope key for `env_key` in the shared key store.
pub fn connect_key(home: &Path, name: &str, env_key: &str, value: &str) -> Result<(), String> {
    let s = Store::new(home);
    match connected_key(home, env_key) {
        Some((id, _)) => s.update(&id, name, env_key, value.trim(), None).map(|_| ()),
        None => s.add(name, env_key, value.trim(), None).map(|_| ()),
    }
}

pub fn forget_key(home: &Path, env_key: &str) -> Result<(), String> {
    if let Some((id, _)) = connected_key(home, env_key) {
        Store::new(home).delete(&id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ollama_and_lmstudio() {
        let o = r#"{"models":[{"name":"llama3.1:8b","size":1},{"name":"qwen2.5-coder:7b"}]}"#;
        assert_eq!(parse_ollama(o), ["llama3.1:8b", "qwen2.5-coder:7b"]);
        let l = r#"{"object":"list","data":[{"id":"qwen/qwen3-4b","object":"model"},{"id":""}]}"#;
        assert_eq!(parse_lmstudio(l), ["qwen/qwen3-4b"]);
        assert!(parse_ollama("not json").is_empty());
        assert!(parse_lmstudio("{}").is_empty());
    }

    #[test]
    fn limits_round_trip_and_precedence() {
        let mut x = Map::new();
        assert!(get_limits(&x, "zai", "glm-4.6").is_empty());
        let l = ModelLimits {
            context: Some(128_000),
            max_output: Some(8_000),
        };
        set_limits(&mut x, "zai", "glm-4.6", l);
        assert_eq!(get_limits(&x, "zai", "glm-4.6"), l);
        assert!(get_limits(&x, "zai", "glm-4.5").is_empty());
        // Explicit member/launch value wins; override is only the fallback.
        assert_eq!(
            effective_context(&x, "zai", Some("glm-4.6"), Some(200_000)),
            Some(200_000)
        );
        assert_eq!(
            effective_context(&x, "zai", Some("glm-4.6"), None),
            Some(128_000)
        );
        assert_eq!(effective_context(&x, "zai", None, None), None);
        assert_eq!(max_output(&x, "zai", Some("glm-4.6")), Some(8_000));
        set_limits(&mut x, "zai", "glm-4.6", ModelLimits::default());
        assert!(x.is_empty());
    }

    #[test]
    fn limits_survive_config_save() {
        let d = tempfile::tempdir().unwrap();
        let mut cfg = AppConfig::load(d.path());
        let l = ModelLimits {
            context: Some(32_000),
            max_output: None,
        };
        set_limits(&mut cfg.extra, "ollama", "llama3:8b", l);
        cfg.save(d.path()).unwrap();
        assert_eq!(
            get_limits(&AppConfig::load(d.path()).extra, "ollama", "llama3:8b"),
            l
        );
    }

    #[test]
    fn key_connect_and_forget() {
        let d = tempfile::tempdir().unwrap();
        assert!(connected_key(d.path(), "ZAI_API_KEY").is_none());
        connect_key(d.path(), "Z.AI", "ZAI_API_KEY", " sk-abcd1234 ").unwrap();
        assert_eq!(connected_key(d.path(), "ZAI_API_KEY").unwrap().1, "1234");
        connect_key(d.path(), "Z.AI", "ZAI_API_KEY", "sk-new-9999").unwrap();
        assert_eq!(Store::new(d.path()).list().len(), 1);
        forget_key(d.path(), "ZAI_API_KEY").unwrap();
        assert!(connected_key(d.path(), "ZAI_API_KEY").is_none());
    }
}
