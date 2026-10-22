//! rteams-services: headless logic for the rteams desktop app (the Rust port of
//! agent-teams-ai). No egui here; the GUI renders what these modules compute.

pub mod announcements;
pub mod atomic;
pub mod attachments;
pub mod claude_root;
pub mod config;
pub mod docpreview;
pub mod editor;
pub mod ext;
pub mod ext_skills;
pub mod graph_model;
pub mod http;
pub mod http_server;
pub mod i18n;
pub mod ledger;
pub mod logtail;
pub mod orgs;
pub mod paths;
pub mod projects;
pub mod prompts;
pub mod queue;
pub mod readrecovery;
pub mod recovery;
pub mod schedule;
pub mod sessions;
pub mod ssh;
pub mod task_folders;
pub mod team_import;
pub mod templates;
pub mod terminal;
pub mod tmux;
pub mod trust;
pub mod updater;
pub mod usage;
pub mod viz;
pub mod worksync;
