//! The MCP server catalog: the Official MCP Registry
//! (`https://registry.modelcontextprotocol.io/v0.1/servers`, cursor paging, only the latest and
//! active entries, 15 minute cache, 10 MiB cap) merged after a bundled list of popular servers
//! that is always available offline.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const REGISTRY_URL: &str = "https://registry.modelcontextprotocol.io/v0.1/servers";
pub const CACHE_TTL_MS: u64 = 15 * 60_000;
const MAX_BODY: usize = 10 * 1024 * 1024;
const BUILTIN: &str = include_str!("mcp_catalog.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PackageKind {
    Npm,
    Pypi,
    Oci,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum InstallSpec {
    /// Run a published package (`npx -y pkg`, `uvx pkg`, `docker run`).
    Package {
        kind: PackageKind,
        identifier: String,
        version: Option<String>,
    },
    /// Connect to a remote endpoint; `sse` = legacy SSE transport.
    Remote { url: String, sse: bool },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVarDef {
    pub name: String,
    pub description: Option<String>,
    pub is_secret: bool,
    pub is_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderDef {
    pub key: String,
    pub description: Option<String>,
    pub is_required: bool,
    pub is_secret: bool,
    pub value_template: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpCatalogItem {
    /// Reverse-DNS registry name; also the stable id.
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository_url: Option<String>,
    pub website_url: Option<String>,
    pub icon_url: Option<String>,
    pub version: Option<String>,
    /// `builtin` or `official`.
    pub source: String,
    pub install: Option<InstallSpec>,
    pub env_vars: Vec<EnvVarDef>,
    pub auth_headers: Vec<HeaderDef>,
    pub requires_auth: bool,
    pub status: Option<String>,
    pub published_at: Option<String>,
    pub updated_at: Option<String>,
    pub stars: Option<u64>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn derive_install(server: &Value) -> Option<InstallSpec> {
    let pkgs = server.get("packages").and_then(Value::as_array);
    let pick = |ty: &str| {
        pkgs.and_then(|p| {
            p.iter()
                .find(|x| x.get("registryType").and_then(Value::as_str) == Some(ty))
        })
    };
    for (ty, kind) in [
        ("npm", PackageKind::Npm),
        ("pypi", PackageKind::Pypi),
        ("oci", PackageKind::Oci),
    ] {
        // A package whose transport is not stdio cannot be spawned.
        if let Some(p) = pick(ty).filter(|p| {
            p.get("transport")
                .and_then(|t| t.get("type"))
                .and_then(Value::as_str)
                .is_none_or(|t| t == "stdio")
        }) {
            let version = s(p, "version").filter(|v| v != "latest");
            return Some(InstallSpec::Package {
                kind,
                identifier: s(p, "identifier")?,
                version,
            });
        }
    }
    let remote = server.get("remotes")?.as_array()?.first()?;
    Some(InstallSpec::Remote {
        url: s(remote, "url")?,
        sse: s(remote, "type").as_deref() == Some("sse"),
    })
}

/// Normalize one registry entry (`{server, _meta}`); `None` when it is not the latest / active.
pub fn normalize_entry(entry: &Value, source: &str) -> Option<McpCatalogItem> {
    let server = entry.get("server")?;
    let meta = entry
        .get("_meta")
        .and_then(|m| m.get("io.modelcontextprotocol.registry/official"));
    if meta.is_some_and(|m| m.get("isLatest").and_then(Value::as_bool) == Some(false)) {
        return None;
    }
    let status = meta.and_then(|m| s(m, "status"));
    if status.as_deref().is_some_and(|st| st != "active") {
        return None;
    }
    let id = s(server, "name")?;
    let mut env_vars = Vec::new();
    for pkg in server
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for ev in pkg
            .get("environmentVariables")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(name) = s(ev, "name") {
                if !env_vars.iter().any(|e: &EnvVarDef| e.name == name) {
                    env_vars.push(EnvVarDef {
                        name,
                        description: s(ev, "description"),
                        is_secret: b(ev, "isSecret"),
                        is_required: b(ev, "isRequired"),
                    });
                }
            }
        }
    }
    let mut auth_headers: Vec<HeaderDef> = Vec::new();
    for remote in server
        .get("remotes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for h in remote
            .get("headers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let key = s(h, "name")
                .map(|k| k.trim().to_string())
                .unwrap_or_default();
            if !key.is_empty() && !auth_headers.iter().any(|x| x.key == key) {
                auth_headers.push(HeaderDef {
                    key,
                    description: s(h, "description"),
                    is_required: b(h, "isRequired"),
                    is_secret: b(h, "isSecret"),
                    value_template: s(h, "value"),
                });
            }
        }
    }
    let icons = server.get("icons").and_then(Value::as_array);
    let icon_url = icons.and_then(|i| {
        i.iter()
            .find(|x| s(x, "theme").as_deref() == Some("dark"))
            .or(i.first())
            .and_then(|x| s(x, "src"))
    });
    let name = s(server, "title")
        .or_else(|| id.rsplit('/').next().map(str::to_string))
        .unwrap_or_else(|| id.clone());
    Some(McpCatalogItem {
        requires_auth: auth_headers.iter().any(|h| h.is_required),
        name,
        description: s(server, "description").unwrap_or_default(),
        repository_url: server.get("repository").and_then(|r| s(r, "url")),
        website_url: s(server, "websiteUrl"),
        icon_url,
        version: s(server, "version"),
        source: source.into(),
        install: derive_install(server),
        env_vars,
        auth_headers,
        status,
        published_at: meta.and_then(|m| s(m, "publishedAt")),
        updated_at: meta.and_then(|m| s(m, "updatedAt")),
        stars: server.get("stars").and_then(Value::as_u64),
        id,
    })
}

/// Parse one registry response page into items (deduplicated by id) plus the next cursor.
pub fn parse_page(v: &Value, source: &str) -> (Vec<McpCatalogItem>, Option<String>) {
    let mut seen = std::collections::HashSet::new();
    let items = v
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| normalize_entry(e, source))
        .filter(|i| seen.insert(i.id.clone()))
        .collect();
    let next = v.get("metadata").and_then(|m| s(m, "nextCursor"));
    (items, next)
}

/// The bundled popular servers (works offline).
pub fn builtin() -> Vec<McpCatalogItem> {
    serde_json::from_str::<Value>(BUILTIN)
        .map(|v| parse_page(&v, "builtin").0)
        .unwrap_or_default()
}

/// Case-insensitive filter over name, id and description.
pub fn filter<'a>(items: &'a [McpCatalogItem], query: &str) -> Vec<&'a McpCatalogItem> {
    let q = query.trim().to_lowercase();
    items
        .iter()
        .filter(|i| {
            q.is_empty()
                || i.name.to_lowercase().contains(&q)
                || i.id.to_lowercase().contains(&q)
                || i.description.to_lowercase().contains(&q)
        })
        .collect()
}

/// Look an item up by id (the install flow re-resolves by id and never trusts UI-supplied specs).
pub fn find<'a>(items: &'a [McpCatalogItem], id: &str) -> Option<&'a McpCatalogItem> {
    items.iter().find(|i| i.id == id)
}

// ---------------------------------------------------------------------------------------------
// Fetch + cache
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub items: Vec<McpCatalogItem>,
    /// Unix ms the registry part was fetched.
    pub fetched_at_ms: u64,
    /// The network failed; showing cached or bundled data.
    pub stale: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fetched_at_ms: u64,
    items: Vec<McpCatalogItem>,
}

pub fn cache_path(home: &Path) -> PathBuf {
    home.join("ext-cache").join("mcp-registry.json")
}

fn read_cache(home: &Path) -> Option<CacheFile> {
    serde_json::from_slice(&std::fs::read(cache_path(home)).ok()?).ok()
}

fn write_cache(home: &Path, c: &CacheFile) {
    if let Ok(b) = serde_json::to_vec(c) {
        let _ = crate::atomic::write(&cache_path(home), &b);
    }
}

/// Bundled servers first, then registry ones the bundle lacks.
pub fn merge(builtin: Vec<McpCatalogItem>, remote: Vec<McpCatalogItem>) -> Vec<McpCatalogItem> {
    let mut out = builtin;
    for r in remote {
        if !out.iter().any(|b| b.id == r.id) {
            out.push(r);
        }
    }
    out
}

/// Fetch up to `max_pages` pages of `limit` servers from the registry (blocking; run it on a
/// worker thread).
pub fn fetch_registry(max_pages: usize, limit: usize) -> Result<Vec<McpCatalogItem>, String> {
    let mut items: Vec<McpCatalogItem> = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..max_pages.max(1) {
        let mut url = format!("{REGISTRY_URL}?limit={limit}");
        if let Some(c) = &cursor {
            url.push_str("&cursor=");
            url.push_str(&urlencode(c));
        }
        let body =
            crate::http::get_bytes(&url, Duration::from_secs(15)).map_err(|e| e.to_string())?;
        if body.len() > MAX_BODY {
            return Err("registry response too large".into());
        }
        let v: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let (page, next) = parse_page(&v, "official");
        for i in page {
            if !items.iter().any(|x| x.id == i.id) {
                items.push(i);
            }
        }
        match next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    Ok(items)
}

fn urlencode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The catalog: fresh cache if younger than 15 min (unless `force`), else a network fetch, else
/// the stale cache, else just the bundled list. `fetch` is injectable for tests.
pub fn load_with(
    home: &Path,
    now_ms: u64,
    force: bool,
    fetch: impl FnOnce() -> Result<Vec<McpCatalogItem>, String>,
) -> Catalog {
    let cache = read_cache(home);
    if let Some(c) = &cache {
        if !force && now_ms.saturating_sub(c.fetched_at_ms) < CACHE_TTL_MS {
            return Catalog {
                items: merge(builtin(), c.items.clone()),
                fetched_at_ms: c.fetched_at_ms,
                ..Default::default()
            };
        }
    }
    match fetch() {
        Ok(items) => {
            write_cache(
                home,
                &CacheFile {
                    fetched_at_ms: now_ms,
                    items: items.clone(),
                },
            );
            Catalog {
                items: merge(builtin(), items),
                fetched_at_ms: now_ms,
                ..Default::default()
            }
        }
        Err(e) => Catalog {
            items: merge(
                builtin(),
                cache.as_ref().map(|c| c.items.clone()).unwrap_or_default(),
            ),
            fetched_at_ms: cache.map_or(0, |c| c.fetched_at_ms),
            stale: true,
            error: Some(e),
        },
    }
}

/// `load_with` against the live registry.
pub fn load(home: &Path, force: bool) -> Catalog {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    load_with(home, now, force, || fetch_registry(5, 100))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builtin_catalog_parses() {
        let items = builtin();
        assert!(items.len() >= 20, "got {}", items.len());
        let fs = find(&items, "io.modelcontextprotocol/filesystem").unwrap();
        assert_eq!(fs.name, "Filesystem");
        assert_eq!(
            fs.install,
            Some(InstallSpec::Package {
                kind: PackageKind::Npm,
                identifier: "@modelcontextprotocol/server-filesystem".into(),
                version: None
            })
        );
        let gh = find(&items, "com.github/github").unwrap();
        assert!(gh.requires_auth);
        assert!(matches!(
            gh.install,
            Some(InstallSpec::Remote { sse: false, .. })
        ));
        let brave = find(&items, "com.brave/brave-search").unwrap();
        assert!(brave.env_vars[0].is_secret && brave.env_vars[0].is_required);
        let fetch = find(&items, "io.modelcontextprotocol/fetch").unwrap();
        assert!(matches!(
            fetch.install,
            Some(InstallSpec::Package {
                kind: PackageKind::Pypi,
                ..
            })
        ));
        assert!(items
            .iter()
            .all(|i| i.install.is_some() && i.source == "builtin"));
    }

    fn entry(name: &str, latest: bool, status: &str) -> Value {
        json!({"server":{"name":name,"description":"d","remotes":[{"type":"sse","url":"https://x/sse"}]},
               "_meta":{"io.modelcontextprotocol.registry/official":{"isLatest":latest,"status":status}}})
    }

    #[test]
    fn registry_page_filters_and_dedups() {
        let page = json!({"servers":[
            entry("a/one", true, "active"),
            entry("a/one", true, "active"),
            entry("a/old", false, "active"),
            entry("a/dead", true, "deleted"),
            {"server":{"name":"a/pkg","title":"Pkg","packages":[{"registryType":"npm","identifier":"p","version":"1.2.3","transport":{"type":"stdio"}}]}}
        ],"metadata":{"nextCursor":"abc"}});
        let (items, next) = parse_page(&page, "official");
        assert_eq!(next.as_deref(), Some("abc"));
        assert_eq!(
            items.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            ["a/one", "a/pkg"]
        );
        assert_eq!(items[0].name, "one");
        assert!(matches!(
            items[0].install,
            Some(InstallSpec::Remote { sse: true, .. })
        ));
        assert_eq!(
            items[1].install,
            Some(InstallSpec::Package {
                kind: PackageKind::Npm,
                identifier: "p".into(),
                version: Some("1.2.3".into())
            })
        );
    }

    #[test]
    fn filter_searches_name_and_description() {
        let items = builtin();
        assert!(filter(&items, "github")
            .iter()
            .any(|i| i.id == "com.github/github"));
        assert!(filter(&items, "BROWSER").len() >= 2);
        assert_eq!(filter(&items, "").len(), items.len());
    }

    #[test]
    fn cache_ttl_and_stale_fallback() {
        let d = tempfile::tempdir().unwrap();
        let extra = normalize_entry(&entry("z/extra", true, "active"), "official").unwrap();
        let mut calls = 0;
        let c = load_with(d.path(), 1_000, false, || {
            calls += 1;
            Ok(vec![extra.clone()])
        });
        assert_eq!(calls, 1);
        assert!(!c.stale && c.items.iter().any(|i| i.id == "z/extra"));
        // Fresh cache: no fetch.
        let c = load_with(d.path(), 2_000, false, || panic!("should use cache"));
        assert!(c.items.iter().any(|i| i.id == "z/extra"));
        // Expired + network down: stale cache with the error.
        let c = load_with(d.path(), 1_000 + CACHE_TTL_MS + 1, false, || {
            Err("offline".into())
        });
        assert!(c.stale && c.error.as_deref() == Some("offline"));
        assert!(c.items.iter().any(|i| i.id == "z/extra"));
        // Never fetched + offline: bundled only.
        let e = tempfile::tempdir().unwrap();
        let c = load_with(e.path(), 5, false, || Err("offline".into()));
        assert!(c.stale && c.items.len() == builtin().len());
    }

    #[test]
    fn urlencode_escapes() {
        assert_eq!(urlencode("a b/c="), "a%20b%2Fc%3D");
    }
}
