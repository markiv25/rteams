//! Canonical JSON (`stableJson`): object keys sorted, no whitespace. Equal payloads hash equal
//! regardless of key order.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn stable_json(v: &Value) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                write(&m[k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write(x, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// Hex sha256 of the stable JSON.
pub fn hash_json(v: &Value) -> String {
    hash_str(&stable_json(v))
}

pub fn hash_str(s: &str) -> String {
    Sha256::digest(s.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn key_order_does_not_matter() {
        let a = json!({"b": [1, {"y": 1, "x": 2}], "a": "s"});
        let b = json!({"a": "s", "b": [1, {"x": 2, "y": 1}]});
        assert_eq!(stable_json(&a), r#"{"a":"s","b":[1,{"x":2,"y":1}]}"#);
        assert_eq!(hash_json(&a), hash_json(&b));
        assert_ne!(hash_json(&a), hash_json(&json!({"a": "t"})));
    }
}
