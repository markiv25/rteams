//! Quick Open (fuzzy file names) and search in files.

use std::fs;
use std::io::Read;
use std::path::Path;

use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use regex::RegexBuilder;

pub const MAX_MATCHES: usize = 500;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_QUICK_FILES: usize = 50_000;

fn walker(root: &Path) -> WalkBuilder {
    let mut b = WalkBuilder::new(root);
    b.hidden(false)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| e.file_name() != ".git");
    b
}

/// Every file below `root` (relative, `/` separated), gitignore-aware. Cache this and
/// invalidate on watcher events.
pub fn list_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walker(root).build().flatten() {
        if e.file_type().is_some_and(|t| t.is_file()) {
            if let Ok(rel) = e.path().strip_prefix(root) {
                out.push(super::files::rel_string(rel));
                if out.len() >= MAX_QUICK_FILES {
                    break;
                }
            }
        }
    }
    out.sort();
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickHit {
    pub rel: String,
    pub score: i64,
    /// Char indices of the matched characters in `rel`, for highlighting.
    pub indices: Vec<usize>,
}

/// Fuzzy rank `files` for `query`; empty query returns the first `limit` files.
pub fn quick_open(files: &[String], query: &str, limit: usize) -> Vec<QuickHit> {
    let q = query.trim();
    if q.is_empty() {
        return files
            .iter()
            .take(limit)
            .map(|f| QuickHit {
                rel: f.clone(),
                score: 0,
                indices: vec![],
            })
            .collect();
    }
    let m = SkimMatcherV2::default().smart_case();
    let mut hits: Vec<QuickHit> = files
        .iter()
        .filter_map(|f| {
            let (mut score, indices) = m.fuzzy_indices(f, q)?;
            // Prefer matches in the file name over the directory part.
            let name_start = f.rfind('/').map_or(0, |i| i + 1);
            if let Some((s, _)) = m.fuzzy_indices(&f[name_start..], q) {
                score += s;
            }
            Some(QuickHit {
                rel: f.clone(),
                score,
                indices,
            })
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.rel.len().cmp(&b.rel.len()))
    });
    hits.truncate(limit);
    hits
}

#[derive(Debug, Clone, Default)]
pub struct SearchOpts {
    pub query: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    /// Comma/newline separated globs; empty = everything.
    pub include: String,
    /// Comma/newline separated globs to skip.
    pub exclude: String,
    /// 0 = `MAX_MATCHES`.
    pub max_results: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    pub rel: String,
    /// 1-based.
    pub line: usize,
    /// 1-based char column of the match start.
    pub col: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub matches: Vec<Match>,
    pub file_count: usize,
    pub truncated: bool,
    pub error: Option<String>,
}

fn globs(s: &str) -> Vec<String> {
    s.split([',', '\n'])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(String::from)
        .collect()
}

pub fn search_content(root: &Path, o: &SearchOpts) -> SearchResults {
    let mut res = SearchResults::default();
    if o.query.is_empty() {
        return res;
    }
    let pat = if o.regex {
        o.query.clone()
    } else {
        regex::escape(&o.query)
    };
    let pat = if o.whole_word {
        format!(r"\b(?:{pat})\b")
    } else {
        pat
    };
    let re = match RegexBuilder::new(&pat)
        .case_insensitive(!o.case_sensitive)
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            res.error = Some(e.to_string());
            return res;
        }
    };
    let cap = if o.max_results == 0 {
        MAX_MATCHES
    } else {
        o.max_results
    };
    let mut wb = walker(root);
    let (inc, exc) = (globs(&o.include), globs(&o.exclude));
    if !inc.is_empty() || !exc.is_empty() {
        let mut ob = OverrideBuilder::new(root);
        for g in &inc {
            let _ = ob.add(g);
        }
        for g in &exc {
            let _ = ob.add(&format!("!{g}"));
        }
        if let Ok(ov) = ob.build() {
            wb.overrides(ov);
        }
    }
    'files: for e in wb.build().flatten() {
        if !e.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = e.path();
        if fs::metadata(path).map_or(true, |m| m.len() > MAX_FILE_BYTES) {
            continue;
        }
        let mut bytes = Vec::new();
        if fs::File::open(path)
            .and_then(|mut f| f.read_to_end(&mut bytes))
            .is_err()
        {
            continue;
        }
        if super::files::looks_binary(&bytes) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let rel = path
            .strip_prefix(root)
            .map(super::files::rel_string)
            .unwrap_or_default();
        let mut hit_file = false;
        for (i, line) in text.lines().enumerate() {
            if let Some(m) = re.find(line) {
                hit_file = true;
                if res.matches.len() >= cap {
                    res.truncated = true;
                    break 'files;
                }
                let shown: String = line.trim().chars().take(240).collect();
                res.matches.push(Match {
                    rel: rel.clone(),
                    line: i + 1,
                    col: line[..m.start()].chars().count() + 1,
                    text: shown,
                });
            }
        }
        if hit_file {
            res.file_count += 1;
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        fs::create_dir_all(r.join("src")).unwrap();
        fs::write(r.join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir_all(r.join("ignored")).unwrap();
        fs::write(r.join("ignored/x.rs"), "needle").unwrap();
        fs::write(
            r.join("src/main.rs"),
            "fn main() {\n    let Needle = 1;\n    needle();\n}\n",
        )
        .unwrap();
        fs::write(r.join("src/lib.rs"), "pub fn needles() {}\n").unwrap();
        fs::write(r.join("notes.md"), "a needle here\n").unwrap();
        fs::write(r.join("bin.dat"), b"needle\0\0").unwrap();
        d
    }

    #[test]
    fn quick_open_ranks_and_skips_ignored() {
        let d = fixture();
        let files = list_files(d.path());
        assert!(!files.iter().any(|f| f.starts_with("ignored/")));
        let hits = quick_open(&files, "mn", 10);
        assert_eq!(hits[0].rel, "src/main.rs");
        assert_eq!(quick_open(&files, "", 2).len(), 2);
    }

    #[test]
    fn content_search_modes() {
        let d = fixture();
        let r = d.path();
        let base = SearchOpts {
            query: "needle".into(),
            ..Default::default()
        };
        let all = search_content(r, &base);
        assert_eq!(all.matches.len(), 4, "{:?}", all.matches); // Needle, needle(), needles, notes
        assert_eq!(all.file_count, 3);
        let cs = search_content(
            r,
            &SearchOpts {
                case_sensitive: true,
                ..base.clone()
            },
        );
        assert_eq!(cs.matches.len(), 3);
        let ww = search_content(
            r,
            &SearchOpts {
                whole_word: true,
                ..base.clone()
            },
        );
        assert_eq!(ww.matches.len(), 3);
        let rx = search_content(
            r,
            &SearchOpts {
                query: r"ne+dles?\(".into(),
                regex: true,
                ..base.clone()
            },
        );
        assert_eq!(rx.matches.len(), 2);
        assert!(rx.matches[0].col > 0);
        let inc = search_content(
            r,
            &SearchOpts {
                include: "*.md".into(),
                ..base.clone()
            },
        );
        assert_eq!(inc.matches.len(), 1);
        let exc = search_content(
            r,
            &SearchOpts {
                exclude: "src/**".into(),
                ..base.clone()
            },
        );
        assert_eq!(exc.matches.len(), 1);
        let cap = search_content(
            r,
            &SearchOpts {
                max_results: 2,
                ..base.clone()
            },
        );
        assert!(cap.truncated && cap.matches.len() == 2);
        let bad = search_content(
            r,
            &SearchOpts {
                query: "(".into(),
                regex: true,
                ..base
            },
        );
        assert!(bad.error.is_some());
    }
}
