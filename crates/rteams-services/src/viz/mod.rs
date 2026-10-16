//! viz: chat visualizations. Agents write a ```viz fenced JSON spec (bar / line / pie / table);
//! this module extracts and validates them, the GUI renders them.

pub mod parse;
pub mod spec;

pub use parse::{extract, extract_all, parse, Block};
pub use spec::{ChartKind, ChartSpec, Series, TableSpec, VizSpec};

/// Instruction text to teach agents the format (append to the agent prompt when chat
/// visualizations are enabled).
pub const INSTRUCTIONS: &str = "You may add a visualization only when it clarifies the answer. \
Use one fenced block per visualization, static data only, unique series names, and always close \
the fence. Format: ```viz {\"type\":\"bar|line|pie|table\",\"title\":\"...\",\"labels\":[...],\
\"series\":[{\"name\":\"...\",\"values\":[...]}]} ``` (tables use \"columns\":[...] and \
\"rows\":[[...]] instead of labels/series; pie takes exactly one series).";
