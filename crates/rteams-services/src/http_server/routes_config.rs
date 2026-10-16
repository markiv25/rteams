//! `GET /api/config`: the app config with every secret redacted.

use serde_json::Value;

use super::{Ctx, Req, Resp};
use crate::config::AppConfig;

fn secret_key(k: &str) -> bool {
    let k = k.to_ascii_lowercase();
    ["token", "secret", "password", "apikey", "api_key", "key"]
        .iter()
        .any(|w| k.contains(w))
}

/// Replace the value of every secret-looking key, at any depth.
pub fn redact(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, val) in m.iter_mut() {
                if secret_key(k)
                    && !val.is_null()
                    && *val != Value::Bool(true)
                    && *val != Value::Bool(false)
                {
                    *val = Value::String("***".into());
                } else {
                    redact(val);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(redact),
        _ => {}
    }
}

pub fn handle(ctx: &Ctx, r: &Req) -> Option<Resp> {
    match (r.method.as_str(), r.segs().as_slice()) {
        ("GET", ["api", "config"]) => {
            let mut v = serde_json::to_value(AppConfig::load(&ctx.home)).unwrap_or(Value::Null);
            redact(&mut v);
            Some(Resp::ok(v))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_nested_secrets() {
        let mut v = serde_json::json!({"general":{"server_token":"abc","port":1},"x":[{"api_key":"k"}],"keep":"y"});
        redact(&mut v);
        assert_eq!(v["general"]["server_token"], "***");
        assert_eq!(v["general"]["port"], 1);
        assert_eq!(v["x"][0]["api_key"], "***");
        assert_eq!(v["keep"], "y");
    }
}
