//! Editor services (headless): project files, git status, search, external-change conflicts,
//! tab sessions and a filesystem watcher.

pub mod conflict;
pub mod files;
pub mod gitops;
pub mod gitstatus;
pub mod search;
pub mod sessions;
pub mod watch;
