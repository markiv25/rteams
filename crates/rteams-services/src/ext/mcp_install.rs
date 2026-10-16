//! Turning a catalog entry (or a hand-typed custom server) into a config entry and writing it.
//! Inputs are validated the way the original does before anything touches a config file:
//! server name `^[\w.-]{1,100}$`, env keys `^[A-Z_][A-Z0-9_]{0,100}$` and header keys that are
//! RFC 7230 tokens.

use std::collections::BTreeMap;

use super::mcp_config::{self, ClientPaths, Runtime, Scope, ServerSpec, Transport};
use super::mcp_registry::{InstallSpec, McpCatalogItem, PackageKind};

pub fn validate_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if ok {
        Ok(())
    } else {
        Err("Invalid server name. Use alphanumeric characters, dashes, underscores, dots.".into())
    }
}

pub fn validate_env_key(k: &str) -> Result<(), String> {
    let mut chars = k.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_uppercase() || c == '_');
    let rest_ok = chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if first_ok && rest_ok && k.len() <= 101 {
        Ok(())
    } else {
        Err(format!("Invalid environment variable name: {k}"))
    }
}

pub fn validate_header_key(k: &str) -> Result<(), String> {
    const TOKEN: &str = "!#$%&'*+-.^_`|~";
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || TOKEN.contains(c))
    {
        Ok(())
    } else {
        Err(format!("Invalid header name: {k}"))
    }
}

pub fn validate_url(u: &str) -> Result<(), String> {
    if (u.starts_with("https://") || u.starts_with("http://")) && !u.contains(char::is_whitespace) {
        Ok(())
    } else {
        Err("Server URL must start with http:// or https://".into())
    }
}

fn validate_maps(
    env: &BTreeMap<String, String>,
    headers: &BTreeMap<String, String>,
) -> Result<(), String> {
    for k in env.keys() {
        validate_env_key(k)?;
    }
    for k in headers.keys() {
        validate_header_key(k)?;
    }
    Ok(())
}

/// A sensible server name for a catalog id (`io.github.x/my-server` -> `my-server`).
pub fn default_server_name(item: &McpCatalogItem) -> String {
    let base = item.id.rsplit('/').next().unwrap_or(&item.id);
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    clean.chars().take(100).collect()
}

/// The command line for a published package.
pub fn package_command(
    kind: PackageKind,
    identifier: &str,
    version: Option<&str>,
) -> (String, Vec<String>) {
    match kind {
        PackageKind::Npm => {
            let id = match version {
                Some(v) => format!("{identifier}@{v}"),
                None => identifier.to_string(),
            };
            ("npx".into(), vec!["-y".into(), id])
        }
        PackageKind::Pypi => {
            let id = match version {
                Some(v) => format!("{identifier}=={v}"),
                None => identifier.to_string(),
            };
            ("uvx".into(), vec![id])
        }
        PackageKind::Oci => {
            let id = match version {
                Some(v) if !identifier.contains(':') => format!("{identifier}:{v}"),
                _ => identifier.to_string(),
            };
            (
                "docker".into(),
                vec!["run".into(), "-i".into(), "--rm".into(), id],
            )
        }
    }
}

/// Build the config entry for `item` with the user's `env` / `headers` values and any
/// `extra_args` (e.g. the directories for the filesystem server).
pub fn spec_for(
    item: &McpCatalogItem,
    env: &BTreeMap<String, String>,
    headers: &BTreeMap<String, String>,
    extra_args: &[String],
) -> Result<ServerSpec, String> {
    validate_maps(env, headers)?;
    let Some(install) = &item.install else {
        return Err("Manual setup required".into());
    };
    Ok(match install {
        InstallSpec::Package {
            kind,
            identifier,
            version,
        } => {
            let (command, mut args) = package_command(*kind, identifier, version.as_deref());
            if *kind == PackageKind::Oci {
                // `docker run -i --rm [-e K]... image`: env names go before the image.
                let image = args.pop().unwrap_or_default();
                for k in env.keys() {
                    args.push("-e".into());
                    args.push(k.clone());
                }
                args.push(image);
            }
            args.extend(extra_args.iter().cloned());
            ServerSpec {
                transport: Transport::Stdio,
                command,
                args,
                env: env.clone(),
                ..Default::default()
            }
        }
        InstallSpec::Remote { url, sse } => {
            validate_url(url)?;
            ServerSpec {
                transport: if *sse {
                    Transport::Sse
                } else {
                    Transport::Http
                },
                url: url.clone(),
                headers: headers.clone(),
                ..Default::default()
            }
        }
    })
}

/// Custom stdio server from an npm package (`npx -y pkg[@version]`).
pub fn custom_npm(
    package: &str,
    version: &str,
    env: &BTreeMap<String, String>,
) -> Result<ServerSpec, String> {
    let package = package.trim();
    if package.is_empty() {
        return Err("npm package name is required".into());
    }
    validate_maps(env, &BTreeMap::new())?;
    let v = version.trim();
    let (command, args) = package_command(
        PackageKind::Npm,
        package,
        (!v.is_empty() && v != "latest").then_some(v),
    );
    Ok(ServerSpec {
        transport: Transport::Stdio,
        command,
        args,
        env: env.clone(),
        ..Default::default()
    })
}

/// Custom stdio server from an arbitrary command line.
pub fn custom_command(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
) -> Result<ServerSpec, String> {
    if command.trim().is_empty() {
        return Err("Command is required".into());
    }
    validate_maps(env, &BTreeMap::new())?;
    Ok(ServerSpec {
        transport: Transport::Stdio,
        command: command.trim().into(),
        args: args.to_vec(),
        env: env.clone(),
        ..Default::default()
    })
}

/// Custom remote server.
pub fn custom_http(
    url: &str,
    sse: bool,
    headers: &BTreeMap<String, String>,
) -> Result<ServerSpec, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("Server URL is required".into());
    }
    validate_url(url)?;
    validate_maps(&BTreeMap::new(), headers)?;
    Ok(ServerSpec {
        transport: if sse { Transport::Sse } else { Transport::Http },
        url: url.into(),
        headers: headers.clone(),
        ..Default::default()
    })
}

/// Required env vars / headers of `item` that have no (non-empty) value yet.
pub fn missing_required(
    item: &McpCatalogItem,
    env: &BTreeMap<String, String>,
    headers: &BTreeMap<String, String>,
) -> Vec<String> {
    let empty =
        |m: &BTreeMap<String, String>, k: &str| m.get(k).is_none_or(|v| v.trim().is_empty());
    let mut out: Vec<String> = item
        .env_vars
        .iter()
        .filter(|e| e.is_required && empty(env, &e.name))
        .map(|e| e.name.clone())
        .collect();
    out.extend(
        item.auth_headers
            .iter()
            .filter(|h| h.is_required && empty(headers, &h.key))
            .map(|h| h.key.clone()),
    );
    out
}

/// Pre-fill `item`'s env vars from saved API keys (`env_key -> value`).
pub fn autofill_env(item: &McpCatalogItem, keys: &[(String, String)]) -> BTreeMap<String, String> {
    item.env_vars
        .iter()
        .filter_map(|e| {
            keys.iter()
                .find(|(k, _)| *k == e.name)
                .map(|(_, v)| (e.name.clone(), v.clone()))
        })
        .collect()
}

/// Validate and write the server into `rt`'s config.
pub fn install(
    paths: &ClientPaths,
    rt: Runtime,
    scope: Scope,
    name: &str,
    spec: &ServerSpec,
) -> Result<(), String> {
    validate_name(name)?;
    validate_maps(&spec.env, &spec.headers)?;
    if spec.transport == Transport::Stdio && spec.command.is_empty() {
        return Err("Command is required".into());
    }
    if spec.transport != Transport::Stdio {
        validate_url(&spec.url)?;
    }
    mcp_config::upsert(paths, rt, scope, name, spec).map_err(|e| e.0)
}

/// Remove the server from `rt`'s config.
pub fn uninstall(
    paths: &ClientPaths,
    rt: Runtime,
    scope: Scope,
    name: &str,
) -> Result<bool, String> {
    validate_name(name)?;
    mcp_config::remove(paths, rt, scope, name).map_err(|e| e.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext::mcp_registry::{builtin, find};

    #[test]
    fn validators() {
        assert!(validate_name("my-server_1.x").is_ok());
        assert!(validate_name("bad name").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"a".repeat(101)).is_err());
        assert!(validate_env_key("API_KEY").is_ok());
        assert!(validate_env_key("api_key").is_err());
        assert!(validate_env_key("1KEY").is_err());
        assert!(validate_header_key("X-Api-Key").is_ok());
        assert!(validate_header_key("Bad Header").is_err());
        assert!(validate_url("https://a.test/mcp").is_ok());
        assert!(validate_url("ftp://a").is_err());
    }

    #[test]
    fn builds_npx_uvx_and_remote_specs() {
        let items = builtin();
        let fs = find(&items, "io.modelcontextprotocol/filesystem").unwrap();
        let spec = spec_for(fs, &BTreeMap::new(), &BTreeMap::new(), &["/tmp".into()]).unwrap();
        assert_eq!(spec.command, "npx");
        assert_eq!(
            spec.args,
            ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
        );
        let fetch = find(&items, "io.modelcontextprotocol/fetch").unwrap();
        let spec = spec_for(fetch, &BTreeMap::new(), &BTreeMap::new(), &[]).unwrap();
        assert_eq!(
            (spec.command.as_str(), spec.args.as_slice()),
            ("uvx", &["mcp-server-fetch".to_string()][..])
        );
        let gh = find(&items, "com.github/github").unwrap();
        let mut h = BTreeMap::new();
        h.insert("Authorization".to_string(), "Bearer t".to_string());
        let spec = spec_for(gh, &BTreeMap::new(), &h, &[]).unwrap();
        assert_eq!(spec.transport, Transport::Http);
        assert_eq!(spec.headers["Authorization"], "Bearer t");
        assert_eq!(default_server_name(gh), "github");
        assert_eq!(
            missing_required(gh, &BTreeMap::new(), &BTreeMap::new()),
            ["Authorization"]
        );
        assert!(missing_required(gh, &BTreeMap::new(), &h).is_empty());
    }

    #[test]
    fn rejects_bad_input_and_autofills() {
        let items = builtin();
        let brave = find(&items, "com.brave/brave-search").unwrap();
        let mut bad = BTreeMap::new();
        bad.insert("lower".to_string(), "x".to_string());
        assert!(spec_for(brave, &bad, &BTreeMap::new(), &[]).is_err());
        let filled = autofill_env(
            brave,
            &[
                ("BRAVE_API_KEY".into(), "k".into()),
                ("OTHER".into(), "z".into()),
            ],
        );
        assert_eq!(filled.len(), 1);
        assert_eq!(filled["BRAVE_API_KEY"], "k");
        assert!(custom_npm("", "", &BTreeMap::new()).is_err());
        assert_eq!(
            custom_npm("pkg", "1.0.0", &BTreeMap::new()).unwrap().args,
            ["-y", "pkg@1.0.0"]
        );
        assert_eq!(
            custom_npm("pkg", "latest", &BTreeMap::new()).unwrap().args,
            ["-y", "pkg"]
        );
        assert!(custom_http("", false, &BTreeMap::new()).is_err());
    }

    #[test]
    fn oci_puts_env_names_before_image() {
        let mut item = builtin().remove(0);
        item.install = Some(InstallSpec::Package {
            kind: PackageKind::Oci,
            identifier: "ghcr.io/x/y".into(),
            version: Some("1".into()),
        });
        let mut env = BTreeMap::new();
        env.insert("TOKEN".to_string(), "t".to_string());
        let s = spec_for(&item, &env, &BTreeMap::new(), &[]).unwrap();
        assert_eq!(s.command, "docker");
        assert_eq!(
            s.args,
            ["run", "-i", "--rm", "-e", "TOKEN", "ghcr.io/x/y:1"]
        );
    }

    #[test]
    fn install_writes_and_uninstall_removes() {
        let d = tempfile::tempdir().unwrap();
        let p = ClientPaths::new(d.path(), None);
        let spec = custom_npm("pkg", "", &BTreeMap::new()).unwrap();
        assert!(install(&p, Runtime::Claude, Scope::User, "bad name", &spec).is_err());
        install(&p, Runtime::Claude, Scope::User, "pkg", &spec).unwrap();
        assert_eq!(mcp_config::list(&p, Runtime::Claude).0.len(), 1);
        assert!(uninstall(&p, Runtime::Claude, Scope::User, "pkg").unwrap());
        assert!(mcp_config::list(&p, Runtime::Claude).0.is_empty());
    }
}
