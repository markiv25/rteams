//! Import a skill from a folder, a `.zip` or a git URL (`git clone --depth 1` into a temp dir).
//! Importing only *prepares* files in memory; `plan_import` turns them into a reviewable `Plan`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::plan::Plan;
use super::scanner::find_skill_file;
use super::validator;

pub const MAX_FILES: usize = 500;
pub const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Folder(PathBuf),
    Zip(PathBuf),
    Git(String),
}

#[derive(Debug)]
pub enum ImportError {
    MissingSkillFile,
    SymbolicLinks,
    TooManyFiles,
    TooLarge,
    InvalidFolderName,
    MustBeDirectory,
    Git(String),
    Io(String),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportError::MissingSkillFile => write!(f, "This folder does not look like a skill yet. It needs a SKILL.md, Skill.md, or skill.md file."),
            ImportError::SymbolicLinks => write!(f, "This folder contains symbolic links. Import the real files instead of links."),
            ImportError::TooManyFiles => write!(f, "This skill folder is too large to import at once. Remove extra files and try again."),
            ImportError::TooLarge => write!(f, "This skill folder is too large to import safely. Trim large assets and try again."),
            ImportError::InvalidFolderName => write!(f, "Pick a simpler destination folder name using letters, numbers, dots, dashes, or underscores."),
            ImportError::MustBeDirectory => write!(f, "Choose a folder to import, not a single file."),
            ImportError::Git(s) => write!(f, "git: {s}"),
            ImportError::Io(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<std::io::Error> for ImportError {
    fn from(e: std::io::Error) -> Self {
        ImportError::Io(e.to_string())
    }
}

/// Files ready to be planned.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub files: BTreeMap<String, Vec<u8>>,
    /// Suggested destination folder name.
    pub default_folder: String,
    /// Other skill folders found in the same source (zip / repo with several skills), relative.
    pub candidates: Vec<String>,
}

/// Self-deleting temp directory.
struct TempDir(PathBuf);
impl TempDir {
    fn new() -> std::io::Result<TempDir> {
        let p = std::env::temp_dir().join(format!("rteams-skill-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&p)?;
        Ok(TempDir(p))
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Read a skill folder into memory, enforcing limits and refusing symlinks.
pub fn read_folder(dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, ImportError> {
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(ImportError::SymbolicLinks);
    }
    if !meta.is_dir() {
        return Err(ImportError::MustBeDirectory);
    }
    if find_skill_file(dir).is_none() {
        return Err(ImportError::MissingSkillFile);
    }
    let mut out = BTreeMap::new();
    let mut total = 0u64;
    fn walk(
        base: &Path,
        dir: &Path,
        out: &mut BTreeMap<String, Vec<u8>>,
        total: &mut u64,
        depth: usize,
    ) -> Result<(), ImportError> {
        if depth > 10 {
            return Err(ImportError::TooManyFiles);
        }
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == ".DS_Store" {
                continue;
            }
            let ft = e.file_type()?;
            let p = e.path();
            if ft.is_symlink() {
                return Err(ImportError::SymbolicLinks);
            }
            if ft.is_dir() {
                walk(base, &p, out, total, depth + 1)?;
            } else if ft.is_file() {
                if out.len() >= MAX_FILES {
                    return Err(ImportError::TooManyFiles);
                }
                *total += e.metadata()?.len();
                if *total > MAX_TOTAL_BYTES {
                    return Err(ImportError::TooLarge);
                }
                let rel = p
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, std::fs::read(&p)?);
            }
        }
        Ok(())
    }
    walk(dir, dir, &mut out, &mut total, 0)?;
    Ok(out)
}

/// Directories under `root` (depth <= 3, including root) that hold a skill file.
pub fn find_skill_dirs(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        if find_skill_file(dir).is_some() {
            out.push(dir.to_path_buf());
            return;
        }
        if depth == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subs: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n != ".git" && n != "node_modules")
            })
            .collect();
        subs.sort();
        for s in subs {
            walk(&s, depth - 1, out);
        }
    }
    walk(root, 3, &mut out);
    out
}

fn prepare_tree(root: &Path, fallback_name: &str) -> Result<Prepared, ImportError> {
    let dirs = find_skill_dirs(root);
    let Some(first) = dirs.first() else {
        return Err(ImportError::MissingSkillFile);
    };
    let files = read_folder(first)?;
    let default_folder = if first == root {
        fallback_name.to_string()
    } else {
        first
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let candidates = dirs
        .iter()
        .skip(1)
        .filter_map(|d| d.strip_prefix(root).ok())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    Ok(Prepared {
        files,
        default_folder: sanitize_folder(&default_folder),
        candidates,
    })
}

fn sanitize_folder(s: &str) -> String {
    let s: String = s
        .trim_end_matches(".zip")
        .trim_end_matches(".git")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.trim_start_matches('.').to_string()
}

pub fn read_zip(path: &Path, dest: &Path) -> Result<(), ImportError> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| ImportError::Io(e.to_string()))?;
    if z.len() > MAX_FILES * 2 {
        return Err(ImportError::TooManyFiles);
    }
    let mut total = 0u64;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| ImportError::Io(e.to_string()))?;
        let Some(rel) = e.enclosed_name() else {
            continue;
        };
        if rel.components().any(|c| c.as_os_str() == "__MACOSX") {
            continue;
        }
        if e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(ImportError::SymbolicLinks);
        }
        let out = dest.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        let mut buf = vec![];
        (&mut e).take(MAX_TOTAL_BYTES + 1).read_to_end(&mut buf)?;
        total += buf.len() as u64;
        if total > MAX_TOTAL_BYTES {
            return Err(ImportError::TooLarge);
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(&out, buf)?;
    }
    Ok(())
}

/// Only plain remote URLs (no options, no local paths).
pub fn valid_git_url(u: &str) -> bool {
    let u = u.trim();
    !u.starts_with('-')
        && !u.contains(char::is_whitespace)
        && (u.starts_with("https://")
            || u.starts_with("http://")
            || u.starts_with("ssh://")
            || u.starts_with("git@"))
}

fn clone_repo(url: &str, dest: &Path) -> Result<(), ImportError> {
    if !valid_git_url(url) {
        return Err(ImportError::Git(
            "enter an https:// or git@ repository URL".into(),
        ));
    }
    let out = Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", "--", url.trim()])
        .arg(dest)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| ImportError::Git(e.to_string()))?;
    if !out.status.success() {
        return Err(ImportError::Git(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

/// Load a source into memory. Temp files are removed before returning.
pub fn prepare(source: &Source) -> Result<Prepared, ImportError> {
    match source {
        Source::Folder(p) => {
            let files = read_folder(p)?;
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok(Prepared {
                files,
                default_folder: sanitize_folder(&name),
                candidates: vec![],
            })
        }
        Source::Zip(p) => {
            let tmp = TempDir::new()?;
            read_zip(p, &tmp.0)?;
            let stem = p
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            prepare_tree(&tmp.0, &stem)
        }
        Source::Git(url) => {
            let tmp = TempDir::new()?;
            let repo = tmp.0.join("repo");
            clone_repo(url, &repo)?;
            let name = url
                .trim()
                .trim_end_matches('/')
                .rsplit(['/', ':'])
                .next()
                .unwrap_or("skill")
                .to_string();
            prepare_tree(&repo, &name)
        }
    }
}

/// Plan copying `prepared` into `<root>/<folder>`; existing files that are not part of the
/// import are planned for removal so the result matches the source exactly.
pub fn plan_import(prepared: &Prepared, root: &Path, folder: &str) -> Result<Plan, ImportError> {
    if !validator::is_valid_folder_name(folder) {
        return Err(ImportError::InvalidFolderName);
    }
    Ok(Plan::build(&root.join(folder), &prepared.files, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ext_skills::plan::ChangeKind;

    fn make_skill(dir: &Path) {
        std::fs::create_dir_all(dir.join("references")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: s\ndescription: d\n---\nbody\n",
        )
        .unwrap();
        std::fs::write(dir.join("references/a.md"), "ref\n").unwrap();
    }

    #[test]
    fn folder_import_plan_diff_and_apply() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("src-skill");
        make_skill(&src);
        let root = t.path().join("root");
        let prep = prepare(&Source::Folder(src.clone())).unwrap();
        assert_eq!(prep.default_folder, "src-skill");
        let plan = plan_import(&prep, &root, "s").unwrap();
        assert_eq!(plan.counts().added, 2);
        plan.apply().unwrap();
        assert!(root.join("s/references/a.md").exists());

        // re-import after the source changed: shows update and removal, without writing
        std::fs::write(
            src.join("SKILL.md"),
            "---\nname: s\ndescription: d2\n---\nbody\n",
        )
        .unwrap();
        std::fs::remove_file(src.join("references/a.md")).unwrap();
        std::fs::write(root.join("s/extra.txt"), "x").unwrap();
        let prep = prepare(&Source::Folder(src)).unwrap();
        let plan = plan_import(&prep, &root, "s").unwrap();
        let kinds: Vec<_> = plan
            .changes
            .iter()
            .map(|c| (c.rel.as_str(), c.kind))
            .collect();
        assert!(kinds.contains(&("SKILL.md", ChangeKind::Updated)));
        assert!(kinds.contains(&("references/a.md", ChangeKind::Removed)));
        assert!(kinds.contains(&("extra.txt", ChangeKind::Removed)));
        assert!(plan
            .changes
            .iter()
            .find(|c| c.rel == "SKILL.md")
            .unwrap()
            .diff
            .contains("+description: d2"));
        assert!(root.join("s/references/a.md").exists());
    }

    #[test]
    fn rejects_bad_sources() {
        let t = tempfile::tempdir().unwrap();
        assert!(matches!(
            prepare(&Source::Folder(t.path().to_path_buf())),
            Err(ImportError::MissingSkillFile)
        ));
        let f = t.path().join("f.txt");
        std::fs::write(&f, "x").unwrap();
        assert!(matches!(
            prepare(&Source::Folder(f)),
            Err(ImportError::MustBeDirectory)
        ));
        let prep = Prepared {
            files: BTreeMap::new(),
            default_folder: "x".into(),
            candidates: vec![],
        };
        assert!(matches!(
            plan_import(&prep, t.path(), "../x"),
            Err(ImportError::InvalidFolderName)
        ));
        assert!(
            !valid_git_url("--upload-pack=evil")
                && !valid_git_url("/local/path")
                && !valid_git_url("https://x y")
        );
        assert!(
            valid_git_url("https://github.com/a/b.git") && valid_git_url("git@github.com:a/b.git")
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("s");
        make_skill(&src);
        std::os::unix::fs::symlink("/etc/hosts", src.join("link")).unwrap();
        assert!(matches!(read_folder(&src), Err(ImportError::SymbolicLinks)));
    }

    #[test]
    fn zip_import_finds_nested_skill() {
        use std::io::Write;
        let t = tempfile::tempdir().unwrap();
        let zp = t.path().join("pack.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("pack/my-skill/SKILL.md", o).unwrap();
            z.write_all(b"---\nname: my-skill\ndescription: d\n---\nb")
                .unwrap();
            z.start_file("pack/my-skill/scripts/x.sh", o).unwrap();
            z.write_all(b"echo").unwrap();
            z.start_file("../evil.txt", o).unwrap();
            z.write_all(b"no").unwrap();
            z.finish().unwrap();
        }
        let prep = prepare(&Source::Zip(zp)).unwrap();
        assert_eq!(prep.default_folder, "my-skill");
        assert!(prep.files.contains_key("scripts/x.sh") && prep.files.len() == 2);
    }

    #[test]
    fn git_clone_from_local_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        // local paths are refused by valid_git_url, so just check the error path is clean
        assert!(matches!(
            prepare(&Source::Git("/nope".into())),
            Err(ImportError::Git(_))
        ));
    }
}
