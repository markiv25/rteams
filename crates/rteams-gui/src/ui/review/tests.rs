use super::*;
use crate::view::testing::{fixture, render, tr};
use crate::view::{Intents, View};

fn repo_with_changes(home: &Path) -> std::path::PathBuf {
    let repo = home.join("proj");
    git::init_repo(&repo).unwrap();
    let base: String = (1..=30).map(|i| format!("line{i}\n")).collect();
    std::fs::write(repo.join("b.txt"), &base).unwrap();
    std::fs::write(repo.join("c.txt"), "c\n").unwrap();
    git::initial_commit(&repo).unwrap();
    let edited = base
        .replace("line3\n", "THREE\n")
        .replace("line25\n", "TWENTYFIVE\n");
    std::fs::write(repo.join("b.txt"), edited).unwrap();
    std::fs::write(repo.join("c.txt"), "changed\n").unwrap();
    repo
}

#[test]
fn accept_reject_comment_and_persist() {
    let (home, _data) = fixture("review1");
    let repo = repo_with_changes(&home);
    let rc = ReviewCtx {
        home: &home,
        team: "t",
        repo: &repo,
        task: "1",
    };
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into(), "c.txt".into()]);
    d.load(&rc);
    assert_eq!(d.files().len(), 2);
    assert_eq!(d.counts(), (0, 0, 3));
    let mut out = Intents::default();
    // j moves to the next hunk across files.
    d.step(1);
    d.step(1);
    assert_eq!((d.sel_file, d.sel_hunk), (1, 0));
    d.apply(&rc, Act::Accept(0, 0), &mut out);
    assert_eq!(d.counts(), (1, 0, 2));
    d.apply(&rc, Act::Reject(0, 1), &mut out);
    let b = std::fs::read_to_string(repo.join("b.txt")).unwrap();
    assert!(b.contains("THREE") && b.contains("line25") && !b.contains("TWENTYFIVE"));
    assert_eq!(d.counts(), (1, 1, 1));
    d.comment_at = Some(("b.txt".into(), 3));
    d.comment_text = "rename?".into();
    d.apply(&rc, Act::AddComment, &mut out);
    d.apply(&rc, Act::RejectFile(1), &mut out);
    assert_eq!(std::fs::read_to_string(repo.join("c.txt")).unwrap(), "c\n");
    // State survives a reload from disk (accepted hunk keeps its decision).
    let mut d2 = ReviewDialog::default();
    d2.open(vec!["b.txt".into(), "c.txt".into()]);
    d2.load(&rc);
    assert_eq!(d2.files().len(), 1);
    assert_eq!(d2.counts().0, 1);
    assert_eq!(d2.state.comments.len(), 1);
    assert!(store::path(&home, "t", "1").exists());
    d2.apply(&rc, Act::Post, &mut out);
    assert_eq!(out.0.len(), 1);
    d2.apply(&rc, Act::RejectAll, &mut out);
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn renders_headless_unified_and_split() {
    let (home, data) = fixture("review2");
    let repo = repo_with_changes(&home);
    let view = View {
        state: data.state(),
        data: &data,
        actor: Some("boss"),
        agents: &[],
        tr: tr(),
    };
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into(), "c.txt".into(), "missing.txt".into()]);
    d.comment_at = Some(("b.txt".into(), 3));
    let rc = ReviewCtx {
        home: &home,
        team: "t",
        repo: &repo,
        task: "1",
    };
    for split in [false, true] {
        d.split = split;
        render(|ui| d.show(ui.ctx(), &view, &rc, &mut Intents::default()));
    }
    let mut empty = ReviewDialog::default();
    empty.open(vec![]);
    render(|ui| empty.show(ui.ctx(), &view, &rc, &mut Intents::default()));
    let _ = std::fs::remove_dir_all(home);
}

fn rc<'a>(home: &'a Path, repo: &'a Path) -> ReviewCtx<'a> {
    ReviewCtx {
        home,
        team: "t",
        repo,
        task: "1",
    }
}

#[test]
fn conflict_pure_detection() {
    assert!(conflict::changed_since(Some(1), Some(2)));
    assert!(conflict::changed_since(Some(1), None));
    assert!(!conflict::changed_since(Some(1), Some(1)));
    let diff = git::parse_diff("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,1 +1,1 @@\n-a\n+b\n");
    let key = diff[0].hunks[0].key();
    let mut st = ReviewState::default();
    st.hunks.insert(hunk_id("x", &key), Decision::Accepted);
    assert!(conflict::stale_decisions(&st, &diff).is_empty());
    // Accepted hunk gone from the diff, rejected hunk back in it.
    st.hunks.insert(hunk_id("x", "gone"), Decision::Accepted);
    st.hunks.insert(hunk_id("y", &key), Decision::Rejected);
    let cs = conflict::stale_decisions(&st, &diff);
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].pending, Pending::Nothing);
    st.hunks.insert(hunk_id("x", &key), Decision::Rejected);
    let cs = conflict::stale_decisions(&st, &diff);
    assert!(cs
        .iter()
        .any(|c| c.pending == Pending::RejectHunk { key: key.clone() }));
    assert_eq!(conflict::stale_ids(&st, &diff).len(), 2);
}

#[test]
fn reject_conflict_keep_mine_and_theirs() {
    let (home, _d) = fixture("review3");
    let repo = repo_with_changes(&home);
    let rc = rc(&home, &repo);
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into()]);
    d.load(&rc);
    let mut out = Intents::default();
    // Someone edits an unrelated line after the diff loaded.
    let cur = std::fs::read_to_string(repo.join("b.txt")).unwrap();
    std::fs::write(repo.join("b.txt"), cur.replace("line10\n", "TEN\n")).unwrap();
    d.apply(&rc, Act::Reject(0, 1), &mut out);
    assert!(d.conflict.is_some());
    assert!(std::fs::read_to_string(repo.join("b.txt"))
        .unwrap()
        .contains("TWENTYFIVE"));
    // Keep mine: the hunk still applies, so it is rejected now.
    d.apply(&rc, Act::ConflictMine, &mut out);
    assert!(d.conflict.is_none());
    let b = std::fs::read_to_string(repo.join("b.txt")).unwrap();
    assert!(!b.contains("TWENTYFIVE") && b.contains("TEN\n"));
    // Take theirs: the pending action is dropped.
    let cur = std::fs::read_to_string(repo.join("b.txt")).unwrap();
    std::fs::write(repo.join("b.txt"), cur.replace("line11\n", "ELEVEN\n")).unwrap();
    d.apply(&rc, Act::Reject(0, 0), &mut out);
    assert!(d.conflict.is_some());
    d.apply(&rc, Act::ConflictTheirs, &mut out);
    assert!(std::fs::read_to_string(repo.join("b.txt"))
        .unwrap()
        .contains("THREE"));
    // A hunk that no longer applies stays a conflict after keep mine.
    let cur = std::fs::read_to_string(repo.join("b.txt")).unwrap();
    std::fs::write(repo.join("b.txt"), cur.replace("THREE\n", "3\n")).unwrap();
    d.apply(&rc, Act::Reject(0, 0), &mut out);
    d.apply(&rc, Act::ConflictMine, &mut out);
    assert!(d
        .status
        .as_deref()
        .unwrap_or("")
        .contains("no longer applies"));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn stale_saved_decision_shows_as_conflict_on_load() {
    let (home, _d) = fixture("review4");
    let repo = repo_with_changes(&home);
    let rc = rc(&home, &repo);
    let mut st = ReviewState::default();
    st.hunks
        .insert(hunk_id("b.txt", "deadbeef"), Decision::Accepted);
    st.save(&home, "t", "1").unwrap();
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into()]);
    d.load(&rc);
    assert_eq!(d.conflict.as_ref().map(Vec::len), Some(1));
    d.apply(&rc, Act::ConflictTheirs, &mut Intents::default());
    assert!(d.conflict.is_none() && d.state.hunks.is_empty());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn history_records_and_restores() {
    let (home, _d) = fixture("review5");
    let repo = repo_with_changes(&home);
    let rc = rc(&home, &repo);
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into(), "c.txt".into()]);
    d.load(&rc);
    let mut out = Intents::default();
    let original = std::fs::read_to_string(repo.join("b.txt")).unwrap();
    d.apply(&rc, Act::Reject(0, 1), &mut out);
    d.apply(&rc, Act::RejectFile(1), &mut out);
    assert_eq!(d.history.len(), 2);
    assert!(history::dir(&home, "t", "1").exists());
    // Restore the file reject (newest first): c.txt comes back.
    let id = d.history[0].id.clone();
    d.apply(&rc, Act::Restore(id), &mut out);
    assert_eq!(
        std::fs::read_to_string(repo.join("c.txt")).unwrap(),
        "changed\n"
    );
    assert_eq!(d.counts().1, 1, "only the hunk reject remains");
    // Restore the hunk reject.
    let id = d.history.last().unwrap().id.clone();
    d.apply(&rc, Act::Restore(id.clone()), &mut out);
    assert_eq!(
        std::fs::read_to_string(repo.join("b.txt")).unwrap(),
        original
    );
    // Restoring onto a changed file raises a conflict; keep mine forces it.
    d.apply(&rc, Act::Reject(0, 1), &mut out);
    std::fs::write(repo.join("b.txt"), "other\n").unwrap();
    let id = d.history[0].id.clone();
    d.apply(&rc, Act::Restore(id), &mut out);
    assert!(d.conflict.is_some());
    assert_eq!(
        std::fs::read_to_string(repo.join("b.txt")).unwrap(),
        "other\n"
    );
    d.apply(&rc, Act::ConflictMine, &mut out);
    assert_eq!(
        std::fs::read_to_string(repo.join("b.txt")).unwrap(),
        original
    );
    // New-file reject restores by recreating the file.
    std::fs::write(repo.join("n.txt"), "x\n").unwrap();
    let mut d2 = ReviewDialog::default();
    d2.open(vec!["n.txt".into()]);
    d2.load(&rc);
    d2.apply(&rc, Act::RejectFile(0), &mut out);
    assert!(!repo.join("n.txt").exists());
    let e = d2.history[0].clone();
    history::restore(&repo, &e, false).unwrap();
    assert_eq!(std::fs::read_to_string(repo.join("n.txt")).unwrap(), "x\n");
    assert!(history::fmt_ts(0).starts_with("1970-01-01 00:00:00"));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn preview_and_edit_in_review() {
    let (home, _d) = fixture("review6");
    let repo = repo_with_changes(&home);
    let rc = rc(&home, &repo);
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into()]);
    d.load(&rc);
    let mut out = Intents::default();
    d.apply(&rc, Act::Preview(0, 1), &mut out);
    let p = d.preview.as_ref().unwrap();
    assert!(
        p.text.contains("line25") && !p.text.contains("TWENTYFIVE") && p.text.contains("THREE")
    );
    assert!(std::fs::read_to_string(repo.join("b.txt"))
        .unwrap()
        .contains("TWENTYFIVE"));
    d.apply(&rc, Act::ConfirmPreview, &mut out);
    assert!(std::fs::read_to_string(repo.join("b.txt"))
        .unwrap()
        .contains("line25"));
    // Edit and save is recorded in history.
    d.apply(&rc, Act::EditStart(0), &mut out);
    d.edit.as_mut().unwrap().text.push_str("tail\n");
    d.apply(&rc, Act::EditSave, &mut out);
    assert!(std::fs::read_to_string(repo.join("b.txt"))
        .unwrap()
        .ends_with("tail\n"));
    assert_eq!(d.history[0].kind, Kind::Edit);
    // Saving over a file that changed meanwhile conflicts.
    d.apply(&rc, Act::EditStart(0), &mut out);
    std::fs::write(repo.join("b.txt"), "x\n").unwrap();
    d.apply(&rc, Act::EditSave, &mut out);
    assert!(d.conflict.is_some());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn rename_reject_and_reapply_in_dialog() {
    let (home, _d) = fixture("review7");
    let repo = home.join("proj");
    git::init_repo(&repo).unwrap();
    let body: String = (1..=10).map(|i| format!("row{i}\n")).collect();
    std::fs::write(repo.join("old.txt"), &body).unwrap();
    git::initial_commit(&repo).unwrap();
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["mv", "old.txt", "new.txt"])
        .status()
        .unwrap()
        .success());
    let rc = rc(&home, &repo);
    let mut d = ReviewDialog::default();
    d.open(vec!["new.txt".into()]);
    d.load(&rc);
    assert_eq!(d.renames.len(), 1);
    let mut out = Intents::default();
    d.apply(&rc, Act::RejectRename("new.txt".into()), &mut out);
    assert!(repo.join("old.txt").exists() && !repo.join("new.txt").exists());
    assert_eq!(d.state.renames.len(), 1);
    d.apply(&rc, Act::ReapplyRename("new.txt".into()), &mut out);
    assert!(repo.join("new.txt").exists() && d.state.renames.is_empty());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn renders_conflict_history_preview_and_edit() {
    let (home, data) = fixture("review8");
    let repo = repo_with_changes(&home);
    let view = View {
        state: data.state(),
        data: &data,
        actor: Some("boss"),
        agents: &[],
        tr: tr(),
    };
    let rc = rc(&home, &repo);
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into(), "c.txt".into()]);
    d.load(&rc);
    d.apply(&rc, Act::Reject(0, 1), &mut Intents::default());
    d.apply(&rc, Act::Preview(0, 0), &mut Intents::default());
    render(|ui| d.show(ui.ctx(), &view, &rc, &mut Intents::default()));
    d.apply(&rc, Act::ClosePreview, &mut Intents::default());
    d.apply(&rc, Act::EditStart(0), &mut Intents::default());
    render(|ui| d.show(ui.ctx(), &view, &rc, &mut Intents::default()));
    d.edit = None;
    d.conflict = Some(vec![Candidate {
        file: "b.txt".into(),
        reason: Reason::FileChanged,
        pending: Pending::RejectFile,
    }]);
    render(|ui| d.show(ui.ctx(), &view, &rc, &mut Intents::default()));
    d.conflict = None;
    d.show_history = true;
    d.hist_preview = d.history.first().map(|e| e.id.clone());
    assert!(!d.history.is_empty());
    render(|ui| d.show(ui.ctx(), &view, &rc, &mut Intents::default()));
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn totals_sum_added_and_removed_over_files() {
    let (home, _data) = fixture("review-totals");
    let repo = repo_with_changes(&home);
    let mut d = ReviewDialog::default();
    d.open(vec!["b.txt".into(), "c.txt".into()]);
    d.load(&rc(&home, &repo));
    // b.txt: two one-line replacements (+2 -2); c.txt: one replacement (+1 -1).
    assert_eq!(d.totals(), (2, 3, 3));
    let _ = std::fs::remove_dir_all(home);
}
