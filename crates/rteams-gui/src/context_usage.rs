//! Per-agent context-window usage, parsed live from each runtime's output stream and shown as
//! a small meter on the member row.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// The latest context size of one agent and the window it counts against (when known).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub tokens: u64,
    pub window: Option<u64>,
}

impl Usage {
    /// Share of the window in use, 0.0..=1.0+ (None without a window).
    pub fn fraction(&self) -> Option<f32> {
        self.window
            .filter(|w| *w > 0)
            .map(|w| self.tokens as f32 / w as f32)
    }
}

/// Window size for a member: an explicit limit wins, then `[1m]` models, else 200k.
pub fn window_for(model: Option<&str>, limit: Option<u32>) -> u64 {
    if let Some(l) = limit.filter(|l| *l > 0) {
        return l as u64;
    }
    if model.is_some_and(|m| m.trim().ends_with("[1m]")) {
        1_000_000
    } else {
        200_000
    }
}

/// Current context tokens carried by one Claude stream-json line: input + cache creation +
/// cache read of an `assistant` message. Sub-agent (sidechain) messages and lines without
/// usage give None.
pub fn parse_claude(line: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["type"].as_str() != Some("assistant") || !v["parent_tool_use_id"].is_null() {
        return None;
    }
    let u = v["message"]["usage"].as_object()?;
    let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let total = n("input_tokens") + n("cache_creation_input_tokens") + n("cache_read_input_tokens");
    (total > 0).then_some(total)
}

/// Context tokens from a Codex `turn.completed` line (`usage.input_tokens`, cached included).
pub fn parse_codex(line: &str) -> Option<u64> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    if v["type"].as_str() != Some("turn.completed") {
        return None;
    }
    v["usage"]["input_tokens"].as_u64().filter(|t| *t > 0)
}

/// Dispatch on the runtime id; runtimes without usage in their stream yield None.
pub fn parse(runtime: &str, line: &str) -> Option<u64> {
    match runtime {
        "claude" => parse_claude(line),
        "codex" => parse_codex(line),
        _ => None,
    }
}

type Registry = Mutex<BTreeMap<(String, String), Usage>>;
static REGISTRY: Registry = Mutex::new(BTreeMap::new());

/// Record usage for `(team, member)` from an output line, if it carries any.
pub fn ingest(team: &str, member: &str, runtime: &str, window: Option<u64>, line: &str) {
    if let Some(tokens) = parse(runtime, line) {
        if let Ok(mut r) = REGISTRY.lock() {
            r.insert((team.into(), member.into()), Usage { tokens, window });
        }
    }
}

/// Forget a member's usage (a fresh launch starts a new context).
pub fn reset(team: &str, member: &str) {
    if let Ok(mut r) = REGISTRY.lock() {
        r.remove(&(team.to_string(), member.to_string()));
    }
}

pub fn get(team: &str, member: &str) -> Option<Usage> {
    REGISTRY
        .lock()
        .ok()?
        .get(&(team.to_string(), member.to_string()))
        .copied()
}

/// "84k", "1.2M".
pub fn short(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{}k", (n + 500) / 1000)
    } else {
        n.to_string()
    }
}

pub fn label(u: &Usage) -> String {
    match u.window {
        Some(w) => format!("ctx {} / {}", short(u.tokens), short(w)),
        None => format!("ctx {}", short(u.tokens)),
    }
}

pub fn tooltip(u: &Usage) -> String {
    match (u.window, u.fraction()) {
        (Some(w), Some(f)) => format!(
            "Context window: {} of {} tokens in use ({:.0}%)",
            u.tokens,
            w,
            f * 100.0
        ),
        _ => format!("Context window: {} tokens in use", u.tokens),
    }
}

/// Compact meter: label above a thin bar; warning color above 80%.
pub fn meter(ui: &mut egui::Ui, u: &Usage) -> egui::Response {
    use crate::theme;
    let frac = u.fraction().unwrap_or(0.0);
    let color = if frac > 0.8 {
        theme::WARNING
    } else {
        theme::TEXT_2
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label(u), egui::FontId::proportional(10.0), color);
    let w = galley.size().x.max(60.0);
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 20.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter().galley(rect.left_top(), galley, color);
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.bottom() - 4.0),
            egui::vec2(w, 3.0),
        );
        ui.painter().rect_filled(bar, 1.5, theme::tint(color, 0.25));
        if u.window.is_some() {
            let fill = egui::Rect::from_min_size(bar.min, egui::vec2(w * frac.min(1.0), 3.0));
            ui.painter().rect_filled(fill, 1.5, color);
        }
    }
    resp.on_hover_text(tooltip(u))
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = r#"{"type":"assistant","message":{"model":"claude-opus-5-5","content":[],"usage":{"input_tokens":2,"cache_creation_input_tokens":1546,"cache_read_input_tokens":171025,"output_tokens":8}}}"#;

    #[test]
    fn parses_usage_lines() {
        assert_eq!(parse_claude(A), Some(172_573));
        assert_eq!(parse("claude", A), Some(172_573));
        assert_eq!(
            parse_claude(r#"{"type":"assistant","message":{"content":[]}}"#),
            None
        );
        assert_eq!(
            parse_claude(r#"{"type":"result","usage":{"input_tokens":5}}"#),
            None
        );
        assert_eq!(parse_claude("not json"), None);
        let side = A.replacen("{\"type\"", "{\"parent_tool_use_id\":\"x\",\"type\"", 1);
        assert_eq!(parse_claude(&side), None);
        let cx = r#"{"type":"turn.completed","usage":{"input_tokens":9000,"output_tokens":3}}"#;
        assert_eq!(parse("codex", cx), Some(9000));
        assert_eq!(
            parse("codex", r#"{"type":"turn.completed","usage":{}}"#),
            None
        );
        assert_eq!(parse("gemini", A), None);
    }

    #[test]
    fn window_selection() {
        assert_eq!(window_for(Some("opus[1m]"), None), 1_000_000);
        assert_eq!(window_for(Some("opus"), None), 200_000);
        assert_eq!(window_for(None, None), 200_000);
        assert_eq!(window_for(Some("opus[1m]"), Some(500_000)), 500_000);
    }

    #[test]
    fn registry_and_labels() {
        ingest("ctxt", "m", "claude", Some(200_000), A);
        let u = get("ctxt", "m").unwrap();
        assert_eq!(label(&u), "ctx 173k / 200k");
        assert!(tooltip(&u).contains("172573 of 200000"));
        reset("ctxt", "m");
        assert!(get("ctxt", "m").is_none());
        assert_eq!(short(1_200_000), "1.2M");
    }

    #[test]
    fn meter_renders() {
        crate::view::testing::render(|ui| {
            meter(
                ui,
                &Usage {
                    tokens: 190_000,
                    window: Some(200_000),
                },
            );
            meter(
                ui,
                &Usage {
                    tokens: 5,
                    window: None,
                },
            );
        });
    }
}
