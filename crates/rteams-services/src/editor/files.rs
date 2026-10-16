//! Project file service: every operation takes `(root, rel)` and refuses to leave `root`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

/// Largest file that opens normally for editing.
pub const EDIT_LIMIT: u64 = 2 * 1024 * 1024;
/// Largest file that opens at all (large-file mode between `EDIT_LIMIT` and this).
pub const LARGE_LIMIT: u64 = 20 * 1024 * 1024;
/// Bytes shown for files beyond `LARGE_LIMIT`.
pub const PREVIEW_BYTES: u64 = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum EditorError {
    #[error("path is outside the project")]
    Outside,
    #[error("writing inside .git is not allowed")]
    GitDir,
    #[error("{0}")]
    Name(NameError),
    #[error("already exists")]
    Exists,
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("trash: {0}")]
    Trash(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    Invalid,
    InvalidCharacters,
    TooLong,
}

impl NameError {
    /// i18n key of the original validation message.
    pub fn key(self) -> &'static str {
        match self {
            NameError::Empty => "team.editor.newFile.validation.nameRequired",
            NameError::Invalid => "team.editor.newFile.validation.invalidName",
            NameError::InvalidCharacters => "team.editor.newFile.validation.invalidCharacters",
            NameError::TooLong => "team.editor.newFile.validation.nameTooLong",
        }
    }
}

impl std::fmt::Display for NameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NameError::Empty => "Name cannot be empty",
            NameError::Invalid => "Invalid name",
            NameError::InvalidCharacters => "Name contains invalid characters",
            NameError::TooLong => "Name is too long",
        })
    }
}

/// Validate a single path segment typed by the user.
pub fn validate_name(name: &str) -> Result<(), NameError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(NameError::Empty);
    }
    if n.len() > 255 {
        return Err(NameError::TooLong);
    }
    if n == "." || n == ".." {
        return Err(NameError::Invalid);
    }
    if n.chars()
        .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
    {
        return Err(NameError::InvalidCharacters);
    }
    Ok(())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn mtime_ms(path: &Path) -> Option<u64> {
    let m = fs::metadata(path).ok()?.modified().ok()?;
    Some(m.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// Resolve `rel` below `root`, rejecting `..`, absolute paths and symlink escapes.
pub fn resolve(root: &Path, rel: &Path) -> Result<PathBuf, EditorError> {
    for c in rel.components() {
        match c {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(EditorError::Outside),
        }
    }
    let croot = root.canonicalize()?;
    let full = croot.join(rel);
    // Canonicalize the deepest existing ancestor to catch symlinks pointing out of the root.
    let mut probe = full.as_path();
    loop {
        if let Ok(c) = probe.canonicalize() {
            if !c.starts_with(&croot) {
                return Err(EditorError::Outside);
            }
            break;
        }
        match probe.parent() {
            Some(p) => probe = p,
            None => return Err(EditorError::Outside),
        }
    }
    Ok(full)
}

fn resolve_write(root: &Path, rel: &Path) -> Result<PathBuf, EditorError> {
    if rel.components().any(|c| c.as_os_str() == ".git") {
        return Err(EditorError::GitDir);
    }
    resolve(root, rel)
}

/// Relative path with forward slashes (stable across platforms, used as keys).
pub fn rel_string(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub name: String,
    /// Path relative to the root, `/` separated.
    pub rel: String,
    pub is_dir: bool,
    pub size: u64,
}

/// Direct children of `rel` (lazy tree loading). Honors `.gitignore`; hides `.git`; folders first.
pub fn list_dir(root: &Path, rel: &Path) -> Result<Vec<TreeEntry>, EditorError> {
    let dir = resolve(root, rel)?;
    let prefix = rel_string(rel);
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(&dir)
        .max_depth(Some(1))
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git")
        .build();
    for ent in walker.flatten() {
        if ent.depth() == 0 {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        let md = ent.metadata().ok();
        let is_dir = ent.file_type().is_some_and(|t| t.is_dir());
        out.push(TreeEntry {
            rel: if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            },
            name,
            is_dir,
            size: md.map_or(0, |m| m.len()),
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    #[default]
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    /// Fallback for bytes that are not valid UTF-8; round-trips byte for byte.
    Latin1,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
            Encoding::Latin1 => "Latin-1",
        }
    }
}

pub fn decode(bytes: &[u8]) -> (String, Encoding) {
    if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        if let Ok(s) = std::str::from_utf8(rest) {
            return (s.to_string(), Encoding::Utf8Bom);
        }
    }
    for (bom, enc, le) in [
        ([0xFF, 0xFE], Encoding::Utf16Le, true),
        ([0xFE, 0xFF], Encoding::Utf16Be, false),
    ] {
        if let Some(rest) = bytes.strip_prefix(&bom) {
            let units = rest.as_chunks::<2>().0.iter().map(|&pair| {
                if le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            });
            return (
                char::decode_utf16(units)
                    .map(|r| r.unwrap_or('\u{FFFD}'))
                    .collect(),
                enc,
            );
        }
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => (s.to_string(), Encoding::Utf8),
        Err(_) => (bytes.iter().map(|&b| b as char).collect(), Encoding::Latin1),
    }
}

pub fn encode(text: &str, enc: Encoding) -> Vec<u8> {
    match enc {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            v
        }
        Encoding::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            text.encode_utf16()
                .for_each(|u| v.extend_from_slice(&u.to_le_bytes()));
            v
        }
        Encoding::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            text.encode_utf16()
                .for_each(|u| v.extend_from_slice(&u.to_be_bytes()));
            v
        }
        Encoding::Latin1 => text
            .chars()
            .map(|c| {
                if (c as u32) < 256 {
                    c as u32 as u8
                } else {
                    b'?'
                }
            })
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileContent {
    pub text: String,
    pub size: u64,
    pub mtime_ms: u64,
    /// sha256 of the bytes read (the preview prefix when `truncated`).
    pub hash: String,
    pub is_binary: bool,
    /// Only the first `PREVIEW_BYTES` are in `text`; the buffer must be read-only.
    pub truncated: bool,
    /// Between `EDIT_LIMIT` and `LARGE_LIMIT`: editable, but no highlighting/wrapping.
    pub large: bool,
    pub encoding: Encoding,
}

/// First 8 KiB contains a NUL (and is not UTF-16 with a BOM).
pub fn looks_binary(head: &[u8]) -> bool {
    if head.starts_with(&[0xFF, 0xFE]) || head.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    head.iter().take(8192).any(|&b| b == 0)
}

pub fn read_text(root: &Path, rel: &Path) -> Result<FileContent, EditorError> {
    read_path(&resolve(root, rel)?)
}

pub fn read_path(path: &Path) -> Result<FileContent, EditorError> {
    use io::Read;
    let md = fs::metadata(path)?;
    let size = md.len();
    let mtime = mtime_ms(path).unwrap_or(0);
    let truncated = size > LARGE_LIMIT;
    let mut bytes = Vec::new();
    let mut f = fs::File::open(path)?;
    if truncated {
        f.take(PREVIEW_BYTES).read_to_end(&mut bytes)?;
    } else {
        f.read_to_end(&mut bytes)?;
    }
    let is_binary = looks_binary(&bytes);
    let hash = sha256_hex(&bytes);
    let (text, encoding) = if is_binary {
        (String::new(), Encoding::Utf8)
    } else {
        decode(&bytes)
    };
    Ok(FileContent {
        text,
        size,
        mtime_ms: mtime,
        hash,
        is_binary,
        truncated,
        large: size > EDIT_LIMIT && !truncated,
        encoding,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Written {
        mtime_ms: u64,
        hash: String,
    },
    /// The file changed on disk since `expected_hash`; nothing was written.
    Conflict {
        disk_hash: Option<String>,
    },
}

/// Save `text`. With `expected_hash = Some(h)` the write only happens if the file on disk still
/// hashes to `h` (a missing file counts as unchanged-by-others only when `h` is `None`).
pub fn write_text(
    root: &Path,
    rel: &Path,
    text: &str,
    encoding: Encoding,
    expected_hash: Option<&str>,
) -> Result<WriteOutcome, EditorError> {
    let path = resolve_write(root, rel)?;
    if let Some(h) = expected_hash {
        match fs::read(&path) {
            Ok(b) if sha256_hex(&b) == h => {}
            Ok(b) => {
                return Ok(WriteOutcome::Conflict {
                    disk_hash: Some(sha256_hex(&b)),
                })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(WriteOutcome::Conflict { disk_hash: None })
            }
            Err(e) => return Err(e.into()),
        }
    }
    let bytes = encode(text, encoding);
    crate::atomic::write(&path, &bytes)?;
    Ok(WriteOutcome::Written {
        mtime_ms: mtime_ms(&path).unwrap_or(0),
        hash: sha256_hex(&bytes),
    })
}

pub fn create_file(root: &Path, parent_rel: &Path, name: &str) -> Result<PathBuf, EditorError> {
    validate_name(name).map_err(EditorError::Name)?;
    let rel = parent_rel.join(name.trim());
    let path = resolve_write(root, &rel)?;
    if path.exists() {
        return Err(EditorError::Exists);
    }
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    Ok(rel)
}

pub fn create_dir(root: &Path, parent_rel: &Path, name: &str) -> Result<PathBuf, EditorError> {
    validate_name(name).map_err(EditorError::Name)?;
    let rel = parent_rel.join(name.trim());
    let path = resolve_write(root, &rel)?;
    if path.exists() {
        return Err(EditorError::Exists);
    }
    fs::create_dir(&path)?;
    Ok(rel)
}

/// Rename in place; returns the new relative path.
pub fn rename(root: &Path, rel: &Path, new_name: &str) -> Result<PathBuf, EditorError> {
    validate_name(new_name).map_err(EditorError::Name)?;
    let from = resolve_write(root, rel)?;
    let new_rel = rel.parent().unwrap_or(Path::new("")).join(new_name.trim());
    let to = resolve_write(root, &new_rel)?;
    if to.exists() {
        return Err(EditorError::Exists);
    }
    fs::rename(from, to)?;
    Ok(new_rel)
}

/// Move into `dest_dir_rel` (empty = project root); returns the new relative path.
pub fn move_to(root: &Path, rel: &Path, dest_dir_rel: &Path) -> Result<PathBuf, EditorError> {
    let from = resolve_write(root, rel)?;
    let name = rel.file_name().ok_or(EditorError::Outside)?;
    let new_rel = dest_dir_rel.join(name);
    let to = resolve_write(root, &new_rel)?;
    if to == from {
        return Ok(new_rel);
    }
    if to.starts_with(&from) {
        return Err(EditorError::Outside);
    }
    if to.exists() {
        return Err(EditorError::Exists);
    }
    fs::rename(from, to)?;
    Ok(new_rel)
}

pub fn trash_path(root: &Path, rel: &Path) -> Result<(), EditorError> {
    let p = resolve_write(root, rel)?;
    trash::delete(p).map_err(|e| EditorError::Trash(e.to_string()))
}

/// Delete permanently (used when no trash is available and in tests).
pub fn delete_path(root: &Path, rel: &Path) -> Result<(), EditorError> {
    let p = resolve_write(root, rel)?;
    if p.is_dir() {
        fs::remove_dir_all(p)?
    } else {
        fs::remove_file(p)?
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn tree_lists_lazily_and_honors_gitignore() {
        let d = tmp();
        let r = d.path();
        fs::create_dir_all(r.join(".git")).unwrap();
        fs::create_dir_all(r.join("src/inner")).unwrap();
        fs::write(r.join(".gitignore"), "target/\n*.log\n").unwrap();
        fs::create_dir_all(r.join("target")).unwrap();
        fs::write(r.join("a.log"), "x").unwrap();
        fs::write(r.join("Zed.txt"), "x").unwrap();
        fs::write(r.join("src/main.rs"), "fn main(){}").unwrap();
        let top: Vec<_> = list_dir(r, Path::new(""))
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(top, vec!["src", ".gitignore", "Zed.txt"]);
        let sub: Vec<_> = list_dir(r, Path::new("src"))
            .unwrap()
            .into_iter()
            .map(|e| e.rel)
            .collect();
        assert_eq!(sub, vec!["src/inner", "src/main.rs"]);
    }

    #[test]
    fn rejects_escapes() {
        let d = tmp();
        assert!(matches!(
            resolve(d.path(), Path::new("../x")),
            Err(EditorError::Outside)
        ));
        assert!(matches!(
            resolve(d.path(), Path::new("/etc/passwd")),
            Err(EditorError::Outside)
        ));
        assert!(matches!(
            write_text(
                d.path(),
                Path::new(".git/config"),
                "x",
                Encoding::Utf8,
                None
            ),
            Err(EditorError::GitDir)
        ));
        #[cfg(unix)]
        {
            let out = tmp();
            std::os::unix::fs::symlink(out.path(), d.path().join("link")).unwrap();
            assert!(matches!(
                resolve(d.path(), Path::new("link/f")),
                Err(EditorError::Outside)
            ));
        }
    }

    #[test]
    fn read_write_conflict_and_binary() {
        let d = tmp();
        let r = d.path();
        let w = write_text(r, Path::new("a.txt"), "hello", Encoding::Utf8, None).unwrap();
        let WriteOutcome::Written { hash, .. } = w else {
            panic!()
        };
        let c = read_text(r, Path::new("a.txt")).unwrap();
        assert_eq!(
            (c.text.as_str(), c.hash.as_str(), c.is_binary),
            ("hello", hash.as_str(), false)
        );
        fs::write(r.join("a.txt"), "other").unwrap();
        let w2 = write_text(r, Path::new("a.txt"), "mine", Encoding::Utf8, Some(&hash)).unwrap();
        assert!(matches!(w2, WriteOutcome::Conflict { disk_hash: Some(_) }));
        assert_eq!(fs::read_to_string(r.join("a.txt")).unwrap(), "other");
        fs::write(r.join("b.bin"), [1u8, 0, 2]).unwrap();
        assert!(read_text(r, Path::new("b.bin")).unwrap().is_binary);
    }

    #[test]
    fn encodings_round_trip() {
        for (bytes, enc) in [
            (b"\xEF\xBB\xBFhi".to_vec(), Encoding::Utf8Bom),
            (encode("h\u{e9}llo", Encoding::Utf16Le), Encoding::Utf16Le),
            (encode("h\u{e9}llo", Encoding::Utf16Be), Encoding::Utf16Be),
            (b"caf\xe9".to_vec(), Encoding::Latin1),
        ] {
            let (t, e) = decode(&bytes);
            assert_eq!(e, enc);
            assert_eq!(encode(&t, e), bytes);
        }
    }

    #[test]
    fn create_rename_move_delete() {
        let d = tmp();
        let r = d.path();
        let dir = create_dir(r, Path::new(""), "sub").unwrap();
        let f = create_file(r, &dir, "n.txt").unwrap();
        assert!(matches!(
            create_file(r, &dir, "n.txt"),
            Err(EditorError::Exists)
        ));
        assert!(matches!(
            create_file(r, &dir, "a/b"),
            Err(EditorError::Name(NameError::InvalidCharacters))
        ));
        assert!(matches!(
            create_file(r, &dir, "  "),
            Err(EditorError::Name(NameError::Empty))
        ));
        let f2 = rename(r, &f, "m.txt").unwrap();
        assert_eq!(rel_string(&f2), "sub/m.txt");
        let f3 = move_to(r, &f2, Path::new("")).unwrap();
        assert!(r.join("m.txt").exists());
        delete_path(r, &f3).unwrap();
        assert!(!r.join("m.txt").exists());
    }
}
