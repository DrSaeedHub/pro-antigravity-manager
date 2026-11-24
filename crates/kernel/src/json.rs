//! Lenient reading of JSON from outside programs (Antigravity's credential, Google's API).
//!
//! Missing fields, `null`, and numbers sent as strings are all normal; every reader returns
//! `None` (or a neutral value) instead of failing.

use jiff::Timestamp;
use jiff::civil::DateTime;
use jiff::tz::TimeZone;
use serde_json::Value;

/// The value of `key` in the object `node`, unless it is missing or `null`.
#[must_use]
pub fn get<'a>(node: &'a Value, key: &str) -> Option<&'a Value> {
    node.get(key).filter(|value| !value.is_null())
}

/// `key` as text. Numbers and booleans are turned into text; objects and arrays are not.
#[must_use]
pub fn text(node: &Value, key: &str) -> Option<String> {
    match get(node, key)? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

/// `key` as a number. A string holding a number counts.
#[must_use]
pub fn number(node: &Value, key: &str) -> Option<f64> {
    match get(node, key)? {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().parse().ok().filter(|n: &f64| n.is_finite()),
        _ => None,
    }
}

/// Whether `key` is the boolean `true`.
#[must_use]
pub fn flag(node: &Value, key: &str) -> bool {
    matches!(get(node, key), Some(Value::Bool(true)))
}

/// `key` as an array; empty when missing or not an array.
#[must_use]
pub fn items<'a>(node: &'a Value, key: &str) -> &'a [Value] {
    match get(node, key) {
        Some(Value::Array(values)) => values,
        _ => &[],
    }
}

/// `key` as an instant (RFC 3339; a time without an offset is taken as UTC).
#[must_use]
pub fn time(node: &Value, key: &str) -> Option<Timestamp> {
    parse_time(&text(node, key)?)
}

/// Parses an RFC 3339 instant, or a date and time without an offset as UTC.
#[must_use]
pub fn parse_time(text: &str) -> Option<Timestamp> {
    let text = text.trim();
    if let Ok(instant) = text.parse::<Timestamp>() {
        return Some(instant);
    }
    let civil: DateTime = text.parse().ok()?;
    TimeZone::UTC.to_timestamp(civil).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn get_skips_missing_and_null() {
        let node = json!({ "a": 1, "b": null });
        assert_eq!(get(&node, "a"), Some(&json!(1)));
        assert_eq!(get(&node, "b"), None);
        assert_eq!(get(&node, "c"), None);
        assert_eq!(get(&json!([1, 2]), "a"), None);
    }

    #[test]
    fn text_accepts_strings_numbers_and_booleans() {
        let node = json!({ "s": "hi", "n": 3, "f": 0.5, "b": true, "o": {}, "a": [] });
        assert_eq!(text(&node, "s").as_deref(), Some("hi"));
        assert_eq!(text(&node, "n").as_deref(), Some("3"));
        assert_eq!(text(&node, "f").as_deref(), Some("0.5"));
        assert_eq!(text(&node, "b").as_deref(), Some("true"));
        assert_eq!(text(&node, "o"), None);
        assert_eq!(text(&node, "a"), None);
        assert_eq!(text(&node, "missing"), None);
    }

    #[test]
    fn number_accepts_numbers_and_numeric_strings() {
        let node = json!({ "n": 0.25, "s": " 0.75 ", "bad": "x", "inf": "inf", "b": true });
        assert_eq!(number(&node, "n"), Some(0.25));
        assert_eq!(number(&node, "s"), Some(0.75));
        assert_eq!(number(&node, "bad"), None);
        assert_eq!(number(&node, "inf"), None);
        assert_eq!(number(&node, "b"), None);
        assert_eq!(number(&node, "missing"), None);
    }

    #[test]
    fn flag_is_only_true_for_true() {
        let node = json!({ "t": true, "f": false, "s": "true", "n": 1 });
        assert!(flag(&node, "t"));
        assert!(!flag(&node, "f"));
        assert!(!flag(&node, "s"));
        assert!(!flag(&node, "n"));
        assert!(!flag(&node, "missing"));
    }

    #[test]
    fn items_is_empty_unless_an_array() {
        let node = json!({ "a": [1, 2], "o": {} });
        assert_eq!(items(&node, "a").len(), 2);
        assert!(items(&node, "o").is_empty());
        assert!(items(&node, "missing").is_empty());
    }

    #[test]
    fn times_with_and_without_offsets() {
        let node = json!({
            "utc": "2026-11-23T10:00:00Z",
            "nanos": "2026-11-23T10:00:00.123456789+03:30",
            "naive": "2026-11-23T10:00:00",
            "bad": "soon",
        });
        assert_eq!(time(&node, "utc"), Some("2026-11-23T10:00:00Z".parse().unwrap()));
        assert_eq!(time(&node, "nanos"), Some("2026-11-23T06:30:00.123456789Z".parse().unwrap()));
        assert_eq!(time(&node, "naive"), Some("2026-11-23T10:00:00Z".parse().unwrap()));
        assert_eq!(time(&node, "bad"), None);
        assert_eq!(time(&node, "missing"), None);
    }
}
