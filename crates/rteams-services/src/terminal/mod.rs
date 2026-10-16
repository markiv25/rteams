//! Integrated terminal: pty sessions (`session`) and persisted settings (`settings`).

pub mod session;
pub mod settings;

pub use session::{Cell, Color, ScreenSnapshot, TermSize, TerminalSession};
pub use settings::{TabSpec, TerminalSettings};
