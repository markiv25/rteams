//! Health and version.

use serde_json::json;

use super::{Ctx, Req, Resp};

pub fn handle(_ctx: &Ctx, r: &Req) -> Option<Resp> {
    match (r.method.as_str(), r.segs().as_slice()) {
        ("GET", ["api", "health"]) => Some(Resp::ok(json!({ "ok": true }))),
        ("GET", ["api", "version"]) => Some(Resp::ok(
            json!({ "name": "rteams", "version": env!("CARGO_PKG_VERSION") }),
        )),
        _ => None,
    }
}
