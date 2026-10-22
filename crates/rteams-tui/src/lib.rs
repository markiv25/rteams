//! rteams-tui: a terminal client over the same log the GUI uses. Reads come from a
//! read-only `FileStore` that is refreshed every 500 ms; every write is a tool call on
//! `rteams_server::Server` (same policy as the GUI). `App` is pure state: keys in,
//! `Effect`s out, so navigation and input are testable without a terminal.

pub mod app;
pub mod io;
pub mod ui;

pub use app::{columns, App, Effect, Mode, COLUMNS};
