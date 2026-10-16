//! App config (`<home>/config.json`) with `schema_version`. Unknown fields (at the top level and
//! inside every section) survive a load/save round trip so newer or CLI-written keys are not lost.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::atomic;

pub const CURRENT_SCHEMA: u32 = 1;

type Extra = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// `light` | `dark` | `system`.
    pub theme: String,
    /// `system` or a locale code such as `de`.
    pub locale: String,
    /// Language agents are asked to reply in (`system` = follow `locale`).
    pub agent_language: String,
    pub native_title_bar: bool,
    pub launch_at_login: bool,
    pub expand_ai_groups: bool,
    pub server_enabled: bool,
    pub server_port: u16,
    pub server_token: String,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for General {
    fn default() -> Self {
        General {
            theme: "system".into(),
            locale: "system".into(),
            agent_language: "system".into(),
            native_title_bar: false,
            launch_at_login: false,
            expand_ai_groups: false,
            server_enabled: false,
            server_port: 3456,
            server_token: String::new(),
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub enabled: bool,
    pub sound: bool,
    /// Unix seconds until which notifications are snoozed.
    pub snooze_until: Option<i64>,
    pub ignored_repos: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Notifications {
    fn default() -> Self {
        Notifications {
            enabled: true,
            sound: true,
            snooze_until: None,
            ignored_repos: vec![],
            extra: Extra::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Recovery {
    pub transient: bool,
    pub rate_limits: bool,
    pub delay_s: u64,
    pub attempts: u32,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for Recovery {
    fn default() -> Self {
        Recovery {
            transient: true,
            rate_limits: true,
            delay_s: 30,
            attempts: 3,
            extra: Extra::new(),
        }
    }
}

/// Last window size in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub schema_version: u32,
    pub general: General,
    pub notifications: Notifications,
    /// Free-form per-feature sections (`usage`, `terminal`, `editor`, tab state ...), owned by
    /// the features that write them.
    pub usage: Value,
    pub terminal: Value,
    pub editor: Value,
    pub recovery: Recovery,
    pub window: Option<WindowSize>,
    pub custom_project_paths: Vec<PathBuf>,
    pub pinned_sessions: Vec<String>,
    pub hidden_sessions: Vec<String>,
    pub chat_visualizations: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            schema_version: CURRENT_SCHEMA,
            general: General::default(),
            notifications: Notifications::default(),
            usage: Value::Null,
            terminal: Value::Null,
            editor: Value::Null,
            recovery: Recovery::default(),
            window: None,
            custom_project_paths: vec![],
            pinned_sessions: vec![],
            hidden_sessions: vec![],
            chat_visualizations: false,
            extra: Extra::new(),
        }
    }
}

impl AppConfig {
    pub fn path(home: &Path) -> PathBuf {
        home.join("config.json")
    }

    /// Load `<home>/config.json`; missing or corrupt files yield defaults. Migrated on load.
    pub fn load(home: &Path) -> AppConfig {
        std::fs::read(Self::path(home))
            .ok()
            .and_then(|b| serde_json::from_slice::<AppConfig>(&b).ok())
            .map(|mut c| {
                c.migrate();
                c
            })
            .unwrap_or_default()
    }

    /// Atomically write `<home>/config.json`.
    pub fn save(&self, home: &Path) -> std::io::Result<()> {
        let v = serde_json::to_value(self).map_err(std::io::Error::other)?;
        atomic::write_json(&Self::path(home), &v)
    }

    /// Bring older files up to `CURRENT_SCHEMA`.
    pub fn migrate(&mut self) {
        if self.schema_version < CURRENT_SCHEMA {
            self.schema_version = CURRENT_SCHEMA;
        }
    }

    /// Pretty JSON for "Export Config" (the server token is blanked).
    pub fn export(&self) -> String {
        let mut c = self.clone();
        c.general.server_token.clear();
        serde_json::to_string_pretty(&c).unwrap_or_default()
    }

    /// Parse JSON from "Import Config", rejecting non-objects and newer schemas.
    pub fn import(json: &str) -> Result<AppConfig, String> {
        let v: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if !v.is_object() {
            return Err("config must be a JSON object".into());
        }
        let mut c: AppConfig = serde_json::from_value(v).map_err(|e| e.to_string())?;
        if c.schema_version > CURRENT_SCHEMA {
            return Err(format!("unsupported schema_version {}", c.schema_version));
        }
        c.migrate();
        Ok(c)
    }
}

/// Shared config with a version counter that UI code polls each frame.
#[derive(Clone)]
pub struct SharedConfig {
    inner: Arc<RwLock<AppConfig>>,
    version: Arc<AtomicU64>,
}

impl SharedConfig {
    pub fn new(cfg: AppConfig) -> Self {
        SharedConfig {
            inner: Arc::new(RwLock::new(cfg)),
            version: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn get(&self) -> AppConfig {
        self.inner.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Mutate under the write lock and bump the version.
    pub fn update<R>(&self, f: impl FnOnce(&mut AppConfig) -> R) -> R {
        let r = f(&mut self.inner.write().unwrap_or_else(|e| e.into_inner()));
        self.version.fetch_add(1, Ordering::SeqCst);
        r
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_missing_or_corrupt() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(AppConfig::load(d.path()), AppConfig::default());
        std::fs::write(AppConfig::path(d.path()), "{nope").unwrap();
        assert_eq!(AppConfig::load(d.path()).schema_version, CURRENT_SCHEMA);
    }

    #[test]
    fn round_trip_preserves_unknown_fields() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            AppConfig::path(d.path()),
            r#"{"schema_version":0,"future":{"a":[1,2]},"general":{"locale":"de","novel":true},
               "notifications":{"x":1},"window":{"width":800.0,"height":600.0}}"#,
        )
        .unwrap();
        let mut c = AppConfig::load(d.path());
        assert_eq!(c.schema_version, CURRENT_SCHEMA);
        assert_eq!(c.general.locale, "de");
        assert_eq!(
            c.window,
            Some(WindowSize {
                width: 800.0,
                height: 600.0
            })
        );
        c.general.theme = "dark".into();
        c.save(d.path()).unwrap();
        let v = atomic::read_json(&AppConfig::path(d.path())).unwrap();
        assert_eq!(v["future"]["a"][1], 2);
        assert_eq!(v["general"]["novel"], true);
        assert_eq!(v["notifications"]["x"], 1);
        assert_eq!(v["general"]["theme"], "dark");
        assert_eq!(AppConfig::load(d.path()), c);
    }

    #[test]
    fn export_import_and_shared() {
        let mut c = AppConfig::default();
        c.general.server_token = "secret".into();
        let out = c.export();
        assert!(!out.contains("secret"));
        assert!(AppConfig::import(&out).is_ok());
        assert!(AppConfig::import("[1]").is_err());
        assert!(AppConfig::import(r#"{"schema_version":99}"#).is_err());
        let s = SharedConfig::new(c);
        let v0 = s.version();
        s.update(|c| c.general.theme = "light".into());
        assert_eq!(s.get().general.theme, "light");
        assert!(s.version() > v0);
    }
}
