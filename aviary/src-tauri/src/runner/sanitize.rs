//! Bounds runner-owned values before an adapter turns them into display data.
//!
//! Runner protocols routinely carry environment maps, auth headers and entire
//! tool inputs. Aviary persists only selected summaries, but sanitising the
//! source recursively first prevents a future summary helper from accidentally
//! selecting a secret-bearing nested field.

use serde_json::{Map, Value};

const MAX_DEPTH: usize = 8;
const MAX_ARRAY_ITEMS: usize = 48;
const MAX_OBJECT_FIELDS: usize = 64;
const MAX_STRING_BYTES: usize = 4 * 1024;
const MAX_TOTAL_STRING_BYTES: usize = 48 * 1024;
const REDACTED: &str = "[redacted]";
const TRUNCATED: &str = "[truncated]";

pub fn value(value: &Value) -> Value {
    let mut remaining = MAX_TOTAL_STRING_BYTES;
    sanitise(value, 0, &mut remaining)
}

pub fn text(value: &str) -> String {
    truncate_utf8(value, MAX_STRING_BYTES)
}

pub fn compact(value: &Value) -> String {
    let sanitised = self::value(value);
    let encoded = serde_json::to_string(&sanitised).unwrap_or_else(|_| "{}".to_string());
    truncate_utf8(&encoded, MAX_STRING_BYTES)
}

fn sanitise(value: &Value, depth: usize, remaining: &mut usize) -> Value {
    if depth >= MAX_DEPTH {
        return Value::String(TRUNCATED.to_string());
    }

    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(input) => Value::String(take_string(input, remaining)),
        Value::Array(values) => {
            let mut out = values
                .iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|item| sanitise(item, depth + 1, remaining))
                .collect::<Vec<_>>();
            if values.len() > MAX_ARRAY_ITEMS {
                out.push(Value::String(TRUNCATED.to_string()));
            }
            Value::Array(out)
        }
        Value::Object(values) => {
            let mut out = Map::new();
            for (key, child) in values.iter().take(MAX_OBJECT_FIELDS) {
                let key = truncate_utf8(key, 256);
                if sensitive_key(&key) {
                    out.insert(key, Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key, sanitise(child, depth + 1, remaining));
                }
            }
            if values.len() > MAX_OBJECT_FIELDS {
                out.insert(TRUNCATED.to_string(), Value::Bool(true));
            }
            Value::Object(out)
        }
    }
}

fn take_string(input: &str, remaining: &mut usize) -> String {
    if *remaining == 0 {
        return TRUNCATED.to_string();
    }
    let allowed = (*remaining).min(MAX_STRING_BYTES);
    let out = truncate_utf8(input, allowed);
    *remaining = remaining.saturating_sub(out.len());
    out
}

fn sensitive_key(key: &str) -> bool {
    let normalised = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    normalised == "env"
        || normalised.contains("environment")
        || normalised.contains("envvar")
        || normalised.contains("secret")
        || normalised.contains("token")
        || normalised.contains("password")
        || normalised.contains("passwd")
        || normalised.contains("authorization")
        || normalised == "auth"
        || normalised.ends_with("auth")
        || normalised.contains("cookie")
        || normalised.contains("privatekey")
        || normalised.contains("apikey")
        || normalised.contains("accesskey")
        || normalised.contains("credential")
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }
    if max_bytes == 0 {
        return String::new();
    }
    let suffix = "…";
    let content_limit = max_bytes.saturating_sub(suffix.len());
    let mut boundary = content_limit.min(input.len());
    while boundary > 0 && !input.is_char_boundary(boundary) {
        boundary -= 1;
    }
    if boundary == 0 && max_bytes < suffix.len() {
        return String::new();
    }
    let mut out = input[..boundary].to_string();
    if max_bytes >= suffix.len() {
        out.push_str(suffix);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recursively_redacts_sensitive_and_environment_keys() {
        let input = json!({
            "safe": {"name": "visible", "Authorization": "Bearer no"},
            "environmentVariables": {"HOME": "/tmp", "API_KEY": "no"},
            "nested": [{"private-key": "no", "ok": true}],
        });
        let output = value(&input);
        assert_eq!(output["safe"]["name"], "visible");
        assert_eq!(output["safe"]["Authorization"], REDACTED);
        assert_eq!(output["environmentVariables"], REDACTED);
        assert_eq!(output["nested"][0]["private-key"], REDACTED);
    }

    #[test]
    fn bounds_unicode_without_splitting_code_points() {
        let input = "🪶".repeat(MAX_STRING_BYTES);
        let output = text(&input);
        assert!(output.len() <= MAX_STRING_BYTES);
        assert!(output.ends_with('…'));
        assert!(std::str::from_utf8(output.as_bytes()).is_ok());
    }

    #[test]
    fn bounds_depth_arrays_and_total_payload() {
        let mut nested = json!("bottom");
        for _ in 0..(MAX_DEPTH + 4) {
            nested = json!({"value": nested});
        }
        let input = json!({
            "nested": nested,
            "array": (0..200).map(|i| json!({"value": "x".repeat(3000), "i": i})).collect::<Vec<_>>()
        });
        let output = value(&input);
        let encoded = serde_json::to_vec(&output).unwrap();
        assert!(encoded.len() < 80 * 1024);
        assert_eq!(
            output["array"].as_array().unwrap().len(),
            MAX_ARRAY_ITEMS + 1
        );
        assert!(encoded
            .windows(TRUNCATED.len())
            .any(|window| window == TRUNCATED.as_bytes()));
    }
}
