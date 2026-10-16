//! tmux: detection, install strategy per platform, and an on-demand installer runner.

pub mod platform;
pub mod runner;
pub mod strategy;

pub use platform::{current_os, detect_tmux, Os};
pub use runner::{run, InstallEvent, InstallRun};
pub use strategy::{display, install_command, Strategy};
