//! JavaScript's value semantics on parsed JSON, for the code the Node services wrote against untyped
//! request bodies and files: `Number(x)`, `String(x)`, `===`, `typeof`, `trim()` and `Date.parse`.
//! `None` stands for `undefined` (a missing key).

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

/// `Number(x)`.
pub fn number(value: Option<&Value>) -> f64 {
    match value {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => string_to_number(text),
        Some(Value::Array(items)) => match items.as_slice() {
            [] => 0.0,
            [one] => string_to_number(&string(Some(one))),
            _ => f64::NAN,
        },
        Some(Value::Object(_)) => f64::NAN,
    }
}

fn string_to_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).map_or(f64::NAN, |n| n as f64);
    }
    match text {
        "Infinity" | "+Infinity" => f64::INFINITY,
        "-Infinity" => f64::NEG_INFINITY,
        _ if text.contains(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E') => f64::NAN,
        _ => text.parse().unwrap_or(f64::NAN),
    }
}

/// `String(x)`, as template literals write a value.
pub fn string(value: Option<&Value>) -> String {
    match value {
        None => "undefined".into(),
        Some(Value::Null) => "null".into(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => crate::jsjson::number(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    string(Some(item))
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".into(),
    }
}

/// `a === b`: primitives by value; two objects or arrays are never the same value here.
pub fn strict_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(Value::Null), Some(Value::Null)) => true,
        (Some(Value::Bool(x)), Some(Value::Bool(y))) => x == y,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(Value::String(x)), Some(Value::String(y))) => x == y,
        _ => false,
    }
}

/// `x == null`: null or undefined.
pub fn is_nullish(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// JavaScript truthiness, for `!!x`.
pub fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(_) => true,
    }
}

/// A regular expression's `\s`: JavaScript's white space (U+FEFF but not U+0085).
pub fn is_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `String.prototype.trim`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `typeof x === "number"`.
pub fn is_number(value: Option<&Value>) -> bool {
    matches!(value, Some(Value::Number(_)))
}

/// `Number.isSafeInteger(x)` (no coercion).
pub fn safe_integer(value: Option<&Value>) -> Option<i64> {
    let n = value?.as_f64()?;
    (n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0).then_some(n as i64)
}

/// `Date.parse(text)` for the forms the game and these services write: ISO 8601 with or without a
/// time zone (without one, local time, as JavaScript reads it), a bare date (UTC), and RFC 2822.
/// `None` where JavaScript gives NaN, or for a form outside these.
pub fn date_parse(text: &str) -> Option<i64> {
    if let Ok(when) = DateTime::parse_from_rfc3339(text) {
        return Some(when.timestamp_millis());
    }
    if let Ok(when) = DateTime::parse_from_rfc2822(text) {
        return Some(when.timestamp_millis());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(local) = NaiveDateTime::parse_from_str(text, format) {
            return chrono::Local
                .from_local_datetime(&local)
                .earliest()
                .map(|when| when.timestamp_millis());
        }
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|when| Utc.from_utc_datetime(&when).timestamp_millis())
}

/// `decodeURIComponent(text)`: every `%XX` decoded, the bytes read as UTF-8. `None` where JavaScript
/// throws a URIError: a `%` without two hex digits after it, or bytes that are not UTF-8.
pub fn decode_uri_component(text: &str) -> Option<String> {
    if !text.contains('%') {
        return Some(text.to_owned());
    }
    let raw = text.as_bytes();
    let mut out = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        if raw[index] != b'%' {
            out.push(raw[index]);
            index += 1;
            continue;
        }
        let hex = raw.get(index + 1..index + 3)?;
        let hex = std::str::from_utf8(hex).ok()?;
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        out.push(u8::from_str_radix(hex, 16).ok()?);
        index += 3;
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn coercions_match_javascript() {
        assert_eq!(number(Some(&json!(" 12 "))), 12.0);
        assert_eq!(number(Some(&json!(""))), 0.0);
        assert!(number(Some(&json!("12px"))).is_nan());
        assert_eq!(number(Some(&json!([7]))), 7.0);
        assert!(number(None).is_nan());
        assert_eq!(string(Some(&json!([1, null, "a"]))), "1,,a");
        assert_eq!(string(Some(&json!(1.0))), "1");
        assert!(strict_eq(None, None));
        assert!(!strict_eq(Some(&json!({})), Some(&json!({}))));
        assert_eq!(date_parse("2026-10-20T00:00:00.000Z"), Some(1_792_454_400_000));
        assert_eq!(date_parse("2026-10-20"), Some(1_792_454_400_000));
        assert_eq!(date_parse("soon"), None);
    }
}
