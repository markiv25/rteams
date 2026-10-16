//! Integrated terminal: emulator widget, tab strip, settings and the bottom drawer.
//! The shell owns one `TerminalDrawer`; see `drawer` for the API.

pub mod drawer;
pub mod keys;
pub mod settings_panel;
pub mod tabs;
pub mod widget;

#[allow(unused_imports)]
pub use drawer::TerminalDrawer;
