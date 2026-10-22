//! Message/comment attachments: files are copied into `<home>/<team>/attachments/<uuid>/<name>`.
//!
//! `Attachment.path` is stored as an ABSOLUTE path (what `store*` return), so agents, including
//! those reading through the rteams MCP server, can open it directly; `resolve` also accepts
//! older relative paths (relative to `<home>/<team>`) and always returns an absolute path.

use std::path::{Path, PathBuf};

use rteams_core::Attachment;

/// Largest attachment accepted (25 MB).
pub const MAX_BYTES: u64 = 25 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("attachment is a directory: {0}")]
    IsDirectory(String),
    #[error("attachment too large ({size} bytes, limit {max})")]
    TooLarge { size: u64, max: u64 },
    #[error("attachment is empty or unnamed")]
    Invalid,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, AttachError>;

fn team_dir(home: &Path, team: &str) -> PathBuf {
    home.join(team)
}

/// Strip path separators and control characters so the name stays a single file component.
fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').to_string();
    cleaned
}

fn write_new(
    home: &Path,
    team: &str,
    name: &str,
    write: impl FnOnce(&Path) -> Result<()>,
) -> Result<Attachment> {
    let name = sanitize(name);
    if name.is_empty() {
        return Err(AttachError::Invalid);
    }
    let dir = team_dir(home, team)
        .join("attachments")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let dest = dir.join(&name);
    if let Err(e) = write(&dest) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    let path = dest.canonicalize().unwrap_or(dest);
    Ok(Attachment {
        name,
        path: path.to_string_lossy().into_owned(),
    })
}

/// Copy `src_path` into the team's attachment store.
pub fn store(home: &Path, team: &str, src_path: &Path) -> Result<Attachment> {
    let meta = std::fs::metadata(src_path)?;
    if meta.is_dir() {
        return Err(AttachError::IsDirectory(src_path.display().to_string()));
    }
    if meta.len() > MAX_BYTES {
        return Err(AttachError::TooLarge {
            size: meta.len(),
            max: MAX_BYTES,
        });
    }
    let name = src_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    write_new(home, team, &name, |dest| {
        std::fs::copy(src_path, dest)?;
        Ok(())
    })
}

/// Store raw bytes (e.g. a pasted PNG) under `name`.
pub fn store_bytes(home: &Path, team: &str, name: &str, bytes: &[u8]) -> Result<Attachment> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(AttachError::TooLarge {
            size: bytes.len() as u64,
            max: MAX_BYTES,
        });
    }
    write_new(home, team, name, |dest| {
        std::fs::write(dest, bytes)?;
        Ok(())
    })
}

/// Absolute path of an attachment (relative paths are taken from `<home>/<team>`).
pub fn resolve(home: &Path, team: &str, a: &Attachment) -> PathBuf {
    let p = Path::new(&a.path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        team_dir(home, team).join(p)
    }
}

const IMAGE_EXTS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

pub fn is_image(a: &Attachment) -> bool {
    Path::new(&a.name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Decode a PNG/JPEG and scale it to fit `max_px` on the longest side. Returns (w, h, RGBA bytes).
pub fn thumbnail(path: &Path, max_px: u32) -> Option<(u32, u32, Vec<u8>)> {
    let img = image::open(path).ok()?;
    let img = if img.width().max(img.height()) > max_px {
        img.thumbnail(max_px, max_px)
    } else {
        img
    };
    let rgba = img.to_rgba8();
    Some((rgba.width(), rgba.height(), rgba.into_raw()))
}

/// Markdown for agents: an absolute path reference (images use the `![]()` form).
pub fn attachment_markdown(a: &Attachment) -> String {
    if is_image(a) {
        format!("![{}]({})", a.name, a.path)
    } else {
        format!("[{}]({})", a.name, a.path)
    }
}

#[cfg(test)]
mod tests;
