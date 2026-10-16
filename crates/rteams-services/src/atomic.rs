//! Crash-safe writes: write a temp file in the same directory, fsync it, then rename over the
//! target, so readers see the old or the new file and never a torn one.

use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

/// Atomically replace `path` with `bytes`, creating parent directories.
pub fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

/// `write` for a JSON value, pretty-printed.
pub fn write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write(path, &bytes)
}

/// Read a JSON file; `None` if missing or unparsable.
pub fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_replace_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b/c.json");
        write_json(&p, &serde_json::json!({"x": 1})).unwrap();
        write_json(&p, &serde_json::json!({"x": 2})).unwrap();
        assert_eq!(read_json(&p).unwrap()["x"], 2);
        assert_eq!(
            fs::read_dir(p.parent().unwrap()).unwrap().count(),
            1,
            "no temp left"
        );
        assert!(read_json(&dir.path().join("missing")).is_none());
    }
}
