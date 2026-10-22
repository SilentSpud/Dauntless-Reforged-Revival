//! JSON text exactly as JavaScript's `JSON.stringify` writes it (MIGRATION_PLAN.md SB1).
//!
//! The Node services re-serialize what they store and send (`JSON.stringify(JSON.parse(x))`), which
//! rewrites the client's text: `1.0` becomes `1`, `1e2` becomes `100`, `é` becomes `é`, and an
//! integer above 2^53 loses its last digits. Clients and the parity replays compare this text, so
//! every number here is a JavaScript double written with `Number.prototype.toString`, and strings
//! are escaped as `JSON.stringify` escapes them. Key order is the order of the map (insertion order
//! with `serde_json`'s `preserve_order`), as in JavaScript objects (apart from integer-like keys,
//! which JavaScript moves to the front; [`stringify`] does that too).

use serde_json::{Map, Value};

/// `JSON.stringify(value)`.
pub fn stringify(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

/// `JSON.parse(text)` as far as the result can differ: numbers become doubles (an integer above
/// 2^53 is rounded as JavaScript rounds it) and a repeated key keeps its first place with the last
/// value. Errors where JSON.parse throws.
pub fn parse(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text).map(|value| to_js(&value))
}

/// The value as JavaScript holds it: numbers as doubles, object keys in JavaScript's property order.
pub fn to_js(value: &Value) -> Value {
    match value {
        Value::Number(n) => n
            .as_f64()
            .and_then(|f| {
                if f.fract() == 0.0 && f.abs() < 9_007_199_254_740_992.0 {
                    Some(Value::from(f as i64))
                } else {
                    serde_json::Number::from_f64(f).map(Value::Number)
                }
            })
            .unwrap_or(Value::Null),
        Value::Array(items) => Value::Array(items.iter().map(to_js).collect()),
        Value::Object(map) => Value::Object(js_order(map.iter().map(|(k, v)| (k.clone(), to_js(v))))),
        other => other.clone(),
    }
}

/// An array index key in JavaScript: a canonical decimal integer below 2^32 - 1.
fn array_index(key: &str) -> Option<u32> {
    let n: u32 = key.parse().ok()?;
    (n != u32::MAX && n.to_string() == key).then_some(n)
}

/// JavaScript's own-property order: integer keys ascending first, then the rest in insertion order.
fn js_order(entries: impl Iterator<Item = (String, Value)>) -> Map<String, Value> {
    let entries: Vec<(String, Value)> = entries.collect();
    let mut indexed: Vec<(u32, String, Value)> = entries
        .iter()
        .filter_map(|(k, v)| array_index(k).map(|n| (n, k.clone(), v.clone())))
        .collect();
    indexed.sort_by_key(|(n, _, _)| *n);
    let mut map = Map::new();
    for (_, key, value) in indexed {
        map.insert(key, value);
    }
    for (key, value) in entries {
        if array_index(&key).is_none() {
            map.insert(key, value);
        }
    }
    map
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&number(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, item);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            let ordered = js_order(map.iter().map(|(k, v)| (k.clone(), v.clone())));
            for (index, (key, item)) in ordered.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, item);
            }
            out.push('}');
        }
    }
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// `Number.prototype.toString()` as JSON writes it: `null` for NaN and the infinities, `0` for -0.
pub fn number(value: f64) -> String {
    if !value.is_finite() {
        return "null".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    // Rust's `{:e}` gives the shortest digits that read back the same, as JavaScript uses.
    let scientific = format!("{:e}", value.abs());
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let k = digits.len() as i32;
    let n = exponent + 1;
    let sign = if value < 0.0 { "-" } else { "" };
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let e = n - 1;
        let e = if e >= 0 { format!("+{e}") } else { e.to_string() };
        if k == 1 {
            format!("{digits}e{e}")
        } else {
            format!("{}.{}e{e}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// `util.isDeepStrictEqual` on two parsed JSON values: numbers by value (with `0` and `-0` apart),
/// objects regardless of key order.
pub fn deep_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap_or(f64::NAN), y.as_f64().unwrap_or(f64::NAN));
            (x == y && x.is_sign_negative() == y.is_sign_negative()) || (x.is_nan() && y.is_nan())
        }
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(x, y)| deep_equal(x, y)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(key, value)| y.get(key).is_some_and(|other| deep_equal(value, other)))
        }
        _ => a == b,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // Expected values from Node: JSON.stringify(JSON.parse(input)).
    #[test]
    fn matches_json_stringify() {
        let cases = [
            (
                r#"{"a":1.0,"b":1e2,"c":-0.0,"d":0.1,"e":1e21,"f":1.5e-7,"g":123456789012345678901234}"#,
                r#"{"a":1,"b":100,"c":0,"d":0.1,"e":1e+21,"f":1.5e-7,"g":1.2345678901234569e+23}"#,
            ),
            (
                r#"[12345678901234567890, 9007199254740993, 0.000001, 1e-7, 123e18, 4.35, 2.5e-5]"#,
                r#"[12345678901234567000,9007199254740992,0.000001,1e-7,123000000000000000000,4.35,0.000025]"#,
            ),
            (
                r#"{"s":"é \n\t\u0001\"\\/"}"#,
                "{\"s\":\"é\u{2028}\\n\\t\\u0001\\\"\\\\/\"}",
            ),
            (
                r#"{"b":1,"2":2,"a":3,"1":4,"01":5,"x":{"10":0,"9":1}}"#,
                r#"{"1":4,"2":2,"b":1,"a":3,"01":5,"x":{"9":1,"10":0}}"#,
            ),
            (r#"{"a":1,"b":2,"a":3}"#, r#"{"a":3,"b":2}"#),
        ];
        for (input, expected) in cases {
            assert_eq!(stringify(&parse(input).unwrap()), expected, "{input}");
        }
        assert_eq!(number(-1.5), "-1.5");
        assert_eq!(number(f64::NAN), "null");
    }

    #[test]
    fn deep_equality_is_node_s() {
        assert!(deep_equal(
            &json!({"a": 1, "b": [1.0, {"c": null}]}),
            &json!({"b": [1, {"c": null}], "a": 1.0})
        ));
        assert!(!deep_equal(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!deep_equal(&json!(0.0), &json!(-0.0)));
        assert!(!deep_equal(&json!("1"), &json!(1)));
    }
}
