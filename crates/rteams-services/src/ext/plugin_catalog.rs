//! The Claude Code plugin catalog: the official marketplace manifest
//! (`anthropics/claude-plugins-official`, `.claude-plugin/marketplace.json`), cached for 15
//! minutes with a stale fallback, plus install counts from Claude Code's own catalog cache when
//! it exists. Filtering and sorting match the Plugins panel (search, category, capability,
//! installed-only, sort by popularity / name / category).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

pub const MARKETPLACE_URL: &str = "https://raw.githubusercontent.com/anthropics/claude-plugins-official/main/.claude-plugin/marketplace.json";
const OFFICIAL_RAW: &str =
    "https://raw.githubusercontent.com/anthropics/claude-plugins-official/main";
pub const CACHE_TTL_MS: u64 = 15 * 60_000;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginCatalogItem {
    /// `name@marketplace`, the id `claude plugin install` takes.
    pub plugin_id: String,
    pub name: String,
    pub display_name: String,
    pub marketplace: String,
    pub description: String,
    pub category: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub homepage: Option<String>,
    /// Git URL for external plugins.
    pub source_url: Option<String>,
    /// Path inside the marketplace repo (`./plugins/x`) or a git-subdir path.
    pub source_path: Option<String>,
    pub tags: Vec<String>,
    pub has_lsp: bool,
    pub has_mcp: bool,
    pub has_agents: bool,
    pub has_commands: bool,
    pub has_hooks: bool,
    pub has_skills: bool,
    pub is_external: bool,
    pub installs: Option<u64>,
}

/// The capability labels the filter offers.
pub const CAPABILITIES: [&str; 6] = ["lsp", "mcp", "agents", "commands", "hooks", "skills"];

impl PluginCatalogItem {
    pub fn capabilities(&self) -> Vec<&'static str> {
        let flags = [
            self.has_lsp,
            self.has_mcp,
            self.has_agents,
            self.has_commands,
            self.has_hooks,
            self.has_skills,
        ];
        CAPABILITIES
            .iter()
            .zip(flags)
            .filter(|(_, f)| *f)
            .map(|(n, _)| *n)
            .collect()
    }
}

fn nonempty(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Null) | None => false,
        Some(_) => true,
    }
}

fn str_of(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn strip_git(u: &str) -> String {
    u.strip_suffix(".git").unwrap_or(u).to_string()
}

/// Parse a `marketplace.json` document.
pub fn parse_marketplace(v: &Value) -> Vec<PluginCatalogItem> {
    let marketplace = str_of(v, "name").unwrap_or_else(|| "claude-plugins-official".into());
    v.get("plugins")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|raw| {
            let name = str_of(raw, "name")?;
            let (is_external, source_url, source_path) = match raw.get("source") {
                Some(Value::String(p)) => (false, None, Some(p.clone())),
                Some(src @ Value::Object(_)) => (
                    true,
                    str_of(src, "url").map(|u| strip_git(&u)),
                    str_of(src, "path"),
                ),
                _ => (false, None, None),
            };
            let author = match raw.get("author") {
                Some(Value::String(s)) => Some(s.clone()),
                Some(a) => str_of(a, "name"),
                None => None,
            };
            let tags = raw
                .get("tags")
                .or_else(|| raw.get("keywords"))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            Some(PluginCatalogItem {
                plugin_id: format!("{name}@{marketplace}"),
                display_name: str_of(raw, "displayName").unwrap_or_else(|| name.clone()),
                name,
                marketplace: marketplace.clone(),
                description: str_of(raw, "description").unwrap_or_default(),
                category: str_of(raw, "category").unwrap_or_else(|| "other".into()),
                author,
                version: str_of(raw, "version"),
                homepage: str_of(raw, "homepage").or_else(|| source_url.clone()),
                source_url,
                source_path,
                tags,
                has_lsp: nonempty(raw.get("lspServers")),
                has_mcp: nonempty(raw.get("mcpServers")),
                has_agents: nonempty(raw.get("agents")),
                has_commands: nonempty(raw.get("commands")),
                has_hooks: nonempty(raw.get("hooks")),
                has_skills: nonempty(raw.get("skills")),
                is_external,
                installs: None,
            })
        })
        .collect()
}

/// Install counts (`name@marketplace` -> unique installs) from Claude Code's catalog cache.
pub fn read_install_counts(user_home: &Path) -> HashMap<String, u64> {
    let p = user_home.join(".claude/plugins/plugin-catalog-cache.json");
    let Some(v) = std::fs::read(p)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return HashMap::new();
    };
    v["catalog"]["plugins"]
        .as_object()
        .map(|m| {
            m.iter()
                .filter_map(|(k, e)| Some((k.clone(), e["unique_installs"].as_u64()?)))
                .collect()
        })
        .unwrap_or_default()
}

pub fn apply_install_counts(items: &mut [PluginCatalogItem], counts: &HashMap<String, u64>) {
    for i in items {
        i.installs = counts.get(&i.plugin_id).copied();
    }
}

/// Raw README URL for a plugin, when it can be derived.
pub fn readme_url(item: &PluginCatalogItem) -> Option<String> {
    if let Some(url) = &item.source_url {
        let rest = url.split("github.com/").nth(1)?;
        let mut it = rest.split('/');
        let (owner, repo) = (it.next()?, it.next()?);
        let sub = item
            .source_path
            .as_deref()
            .map(|p| p.trim_start_matches("./").trim_matches('/'))
            .filter(|p| !p.is_empty());
        return Some(match sub {
            Some(p) => {
                format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/{p}/README.md")
            }
            None => format!("https://raw.githubusercontent.com/{owner}/{repo}/HEAD/README.md"),
        });
    }
    let p = item.source_path.as_deref()?.trim_start_matches("./");
    Some(format!("{OFFICIAL_RAW}/{p}/README.md"))
}

/// Fetch a plugin README (blocking; worker thread).
pub fn fetch_readme(item: &PluginCatalogItem) -> Result<String, String> {
    let url = readme_url(item).ok_or("no README location")?;
    crate::http::get_text(&url, Duration::from_secs(10)).map_err(|e| e.to_string())
}

#[derive(Debug, Clone, Default)]
pub struct PluginCatalog {
    pub items: Vec<PluginCatalogItem>,
    pub fetched_at_ms: u64,
    pub stale: bool,
    pub error: Option<String>,
}

pub fn cache_path(home: &Path) -> PathBuf {
    home.join("ext-cache").join("plugin-marketplace.json")
}

fn local_manifest(user_home: &Path) -> Option<Value> {
    let p = user_home.join(
        ".claude/plugins/marketplaces/claude-plugins-official/.claude-plugin/marketplace.json",
    );
    serde_json::from_slice(&std::fs::read(p).ok()?).ok()
}

/// Load the catalog. Order: fresh cache, network (`fetch` returns the manifest text), stale
/// cache, the marketplace checkout Claude Code keeps locally, else empty with an error.
pub fn load_with(
    home: &Path,
    user_home: &Path,
    now_ms: u64,
    force: bool,
    fetch: impl FnOnce() -> Result<String, String>,
) -> PluginCatalog {
    #[derive(serde::Serialize, serde::Deserialize)]
    struct Cache {
        fetched_at_ms: u64,
        manifest: Value,
    }
    let cache: Option<Cache> = std::fs::read(cache_path(home))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let counts = read_install_counts(user_home);
    let build = |m: &Value, fetched: u64, stale: bool, error: Option<String>| {
        let mut items = parse_marketplace(m);
        apply_install_counts(&mut items, &counts);
        PluginCatalog {
            items,
            fetched_at_ms: fetched,
            stale,
            error,
        }
    };
    if let Some(c) = &cache {
        if !force && now_ms.saturating_sub(c.fetched_at_ms) < CACHE_TTL_MS {
            return build(&c.manifest, c.fetched_at_ms, false, None);
        }
    }
    let fetched =
        fetch().and_then(|t| serde_json::from_str::<Value>(&t).map_err(|e| e.to_string()));
    match fetched {
        Ok(m) if m.get("plugins").is_some() => {
            if let Ok(b) = serde_json::to_vec(&Cache {
                fetched_at_ms: now_ms,
                manifest: m.clone(),
            }) {
                let _ = crate::atomic::write(&cache_path(home), &b);
            }
            build(&m, now_ms, false, None)
        }
        other => {
            let e = match other {
                Err(e) => e,
                Ok(_) => "marketplace manifest has no plugins".into(),
            };
            if let Some(c) = cache {
                build(&c.manifest, c.fetched_at_ms, true, Some(e))
            } else if let Some(m) = local_manifest(user_home) {
                build(&m, 0, true, Some(e))
            } else {
                PluginCatalog {
                    stale: true,
                    error: Some(e),
                    ..Default::default()
                }
            }
        }
    }
}

/// `load_with` against GitHub and the real `~`.
pub fn load(home: &Path, force: bool) -> PluginCatalog {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let uh = dirs::home_dir().unwrap_or_default();
    load_with(home, &uh, now, force, || {
        crate::http::get_text(MARKETPLACE_URL, Duration::from_secs(15)).map_err(|e| e.to_string())
    })
}

// ---------------------------------------------------------------------------------------------
// Filtering / sorting
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PluginSort {
    #[default]
    Popular,
    NameAsc,
    NameDesc,
    Category,
}

#[derive(Debug, Clone, Default)]
pub struct PluginFilter {
    pub query: String,
    pub categories: Vec<String>,
    pub capabilities: Vec<String>,
    pub installed_only: bool,
}

impl PluginFilter {
    pub fn active_count(&self) -> usize {
        self.categories.len() + self.capabilities.len() + usize::from(self.installed_only)
    }
}

/// Matching items, sorted. `installed` holds the installed plugin ids.
pub fn apply<'a>(
    items: &'a [PluginCatalogItem],
    f: &PluginFilter,
    sort: PluginSort,
    installed: &dyn Fn(&str) -> bool,
) -> Vec<&'a PluginCatalogItem> {
    let q = f.query.trim().to_lowercase();
    let mut out: Vec<&PluginCatalogItem> = items
        .iter()
        .filter(|i| {
            (q.is_empty()
                || i.name.to_lowercase().contains(&q)
                || i.display_name.to_lowercase().contains(&q)
                || i.description.to_lowercase().contains(&q)
                || i.tags.iter().any(|t| t.to_lowercase().contains(&q)))
                && (f.categories.is_empty() || f.categories.contains(&i.category))
                && (f.capabilities.is_empty() || {
                    let caps = i.capabilities();
                    f.capabilities.iter().all(|c| caps.contains(&c.as_str()))
                })
                && (!f.installed_only || installed(&i.plugin_id))
        })
        .collect();
    match sort {
        PluginSort::Popular => out.sort_by(|a, b| {
            b.installs
                .unwrap_or(0)
                .cmp(&a.installs.unwrap_or(0))
                .then_with(|| a.name.cmp(&b.name))
        }),
        PluginSort::NameAsc => out.sort_by_key(|i| i.name.to_lowercase()),
        PluginSort::NameDesc => out.sort_by_key(|i| std::cmp::Reverse(i.name.to_lowercase())),
        PluginSort::Category => out.sort_by(|a, b| {
            a.category
                .cmp(&b.category)
                .then_with(|| a.name.cmp(&b.name))
        }),
    }
    out
}

/// Distinct categories with counts, most common first.
pub fn category_counts(items: &[PluginCatalogItem]) -> Vec<(String, usize)> {
    let mut m: HashMap<&str, usize> = HashMap::new();
    for i in items {
        *m.entry(i.category.as_str()).or_default() += 1;
    }
    let mut v: Vec<(String, usize)> = m.into_iter().map(|(k, n)| (k.to_string(), n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn manifest() -> Value {
        json!({"name":"claude-plugins-official","plugins":[
          {"name":"alpha","description":"Does alpha things","category":"development","author":{"name":"A Co"},
           "source":"./plugins/alpha","lspServers":{"x":{}},"skills":["./s"]},
          {"name":"beta","description":"Database helper","category":"database","version":"1.0.0",
           "source":{"source":"url","url":"https://github.com/o/beta.git"},"mcpServers":{"m":{}},"tags":["sql"]},
          {"name":"gamma","description":"Misc","source":{"source":"git-subdir","url":"https://github.com/o/mono","path":"pkgs/gamma"}},
          {"description":"no name"}
        ]})
    }

    #[test]
    fn parses_manifest() {
        let items = parse_marketplace(&manifest());
        assert_eq!(items.len(), 3);
        let a = &items[0];
        assert_eq!(a.plugin_id, "alpha@claude-plugins-official");
        assert_eq!(a.author.as_deref(), Some("A Co"));
        assert_eq!(a.capabilities(), ["lsp", "skills"]);
        assert!(!a.is_external);
        let b = &items[1];
        assert!(b.is_external && b.has_mcp);
        assert_eq!(b.homepage.as_deref(), Some("https://github.com/o/beta"));
        assert_eq!(items[2].category, "other");
    }

    #[test]
    fn readme_urls() {
        let items = parse_marketplace(&manifest());
        assert_eq!(
            readme_url(&items[0]).unwrap(),
            "https://raw.githubusercontent.com/anthropics/claude-plugins-official/main/plugins/alpha/README.md"
        );
        assert_eq!(
            readme_url(&items[1]).unwrap(),
            "https://raw.githubusercontent.com/o/beta/HEAD/README.md"
        );
        assert_eq!(
            readme_url(&items[2]).unwrap(),
            "https://raw.githubusercontent.com/o/mono/HEAD/pkgs/gamma/README.md"
        );
    }

    #[test]
    fn filter_and_sort() {
        let mut items = parse_marketplace(&manifest());
        let mut counts = HashMap::new();
        counts.insert("beta@claude-plugins-official".to_string(), 50u64);
        counts.insert("alpha@claude-plugins-official".to_string(), 10u64);
        apply_install_counts(&mut items, &counts);
        let none = |_: &str| false;
        let all = apply(&items, &PluginFilter::default(), PluginSort::Popular, &none);
        assert_eq!(
            all.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["beta", "alpha", "gamma"]
        );
        let by_name = apply(
            &items,
            &PluginFilter::default(),
            PluginSort::NameDesc,
            &none,
        );
        assert_eq!(by_name[0].name, "gamma");
        let f = PluginFilter {
            query: "sql".into(),
            ..Default::default()
        };
        assert_eq!(apply(&items, &f, PluginSort::Popular, &none).len(), 1);
        let f = PluginFilter {
            capabilities: vec!["lsp".into()],
            ..Default::default()
        };
        assert_eq!(
            apply(&items, &f, PluginSort::Popular, &none)[0].name,
            "alpha"
        );
        let f = PluginFilter {
            categories: vec!["database".into()],
            installed_only: true,
            ..Default::default()
        };
        assert!(apply(&items, &f, PluginSort::Popular, &none).is_empty());
        assert_eq!(
            apply(&items, &f, PluginSort::Popular, &|id| id
                .starts_with("beta"))
            .len(),
            1
        );
        assert_eq!(f.active_count(), 2);
        assert_eq!(category_counts(&items).len(), 3);
    }

    #[test]
    fn cache_and_fallbacks() {
        let d = tempfile::tempdir().unwrap();
        let uh = tempfile::tempdir().unwrap();
        let text = manifest().to_string();
        let c = load_with(d.path(), uh.path(), 1000, false, || Ok(text.clone()));
        assert_eq!(c.items.len(), 3);
        let c = load_with(d.path(), uh.path(), 2000, false, || panic!("cached"));
        assert_eq!(c.items.len(), 3);
        let c = load_with(d.path(), uh.path(), 1000 + CACHE_TTL_MS + 1, false, || {
            Err("offline".into())
        });
        assert!(c.stale && c.items.len() == 3 && c.error.is_some());
        let e = tempfile::tempdir().unwrap();
        let c = load_with(e.path(), uh.path(), 1, false, || Err("offline".into()));
        assert!(c.items.is_empty() && c.error.is_some());
        // Local marketplace checkout as last resort.
        let mp = uh
            .path()
            .join(".claude/plugins/marketplaces/claude-plugins-official/.claude-plugin");
        std::fs::create_dir_all(&mp).unwrap();
        std::fs::write(mp.join("marketplace.json"), &text).unwrap();
        let c = load_with(e.path(), uh.path(), 1, false, || Err("offline".into()));
        assert_eq!(c.items.len(), 3);
    }

    #[test]
    fn reads_install_counts_from_claude_cache() {
        let uh = tempfile::tempdir().unwrap();
        let dir = uh.path().join(".claude/plugins");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin-catalog-cache.json"),
            r#"{"catalog":{"plugins":{"a@m":{"unique_installs":7},"b@m":{}}}}"#,
        )
        .unwrap();
        let c = read_install_counts(uh.path());
        assert_eq!(c.get("a@m"), Some(&7));
        assert!(!c.contains_key("b@m"));
    }
}
