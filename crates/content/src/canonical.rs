//! RFC 8785 canonical JSON for the values a manifest holds: objects, arrays,
//! strings, booleans, null and integers.
//!
//! This is the same output as `live-proto::jcs` on those values. It is a
//! separate copy because live-proto depends on this crate (for `MapId`), so
//! this crate cannot depend on live-proto. Non-integer numbers, which need
//! ECMAScript number formatting, never occur in a manifest and are refused
//! rather than formatted. The tests replay jcs's vectors that apply.
use std::fmt::Write;

use serde_json::Value;

/// A number that is not an integer exactly representable in a double.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("canonical JSON here holds only integers up to 2^53, not {0}")]
pub struct NotInteger(pub String);

/// The canonical form of `value`.
pub fn canonicalize(value: &Value) -> Result<String, NotInteger> {
    let mut out = String::new();
    write_value(&mut out, value)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value) -> Result<(), NotInteger> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            const EXACT: u64 = 1 << 53;
            if let Some(u) = n.as_u64().filter(|u| *u <= EXACT) {
                let _ = write!(out, "{u}");
            } else if let Some(i) = n.as_i64().filter(|i| i.unsigned_abs() <= EXACT) {
                let _ = write!(out, "{i}");
            } else {
                return Err(NotInteger(n.to_string()));
            }
        }
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.encode_utf16().cmp(b.0.encode_utf16()));
            out.push('{');
            for (i, (k, v)) in entries.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(out, k);
                out.push(':');
                write_value(out, v)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    // The vectors below are live-proto::jcs's tests, less the float ones.

    #[test]
    fn rfc_8785_example_without_numbers() {
        let input = r#"{
            "string": "€$\u000F\u000aA'B\"\\\\\"\/",
            "literals": [null, true, false]
        }"#;
        let v: Value = serde_json::from_str(input).unwrap();
        assert_eq!(
            canonicalize(&v).unwrap(),
            r#"{"literals":[null,true,false],"string":"€$\u000f\nA'B\"\\\\\"/"}"#
        );
    }

    #[test]
    fn keys_sort_by_utf16_units() {
        let v = serde_json::json!({"\u{e000}": 1, "\u{10000}": 2, "a": 3, "": 4});
        assert_eq!(
            canonicalize(&v).unwrap(),
            "{\"\":4,\"a\":3,\"\u{10000}\":2,\"\u{e000}\":1}"
        );
    }

    #[test]
    fn integers_stay_integers_and_others_are_refused() {
        let v = serde_json::json!([0, -7, 9007199254740992u64]);
        assert_eq!(canonicalize(&v).unwrap(), "[0,-7,9007199254740992]");
        for bad in [
            serde_json::json!(18446744073709551615u64),
            serde_json::json!(4.5),
            serde_json::json!({"a": [1.0]}),
        ] {
            assert!(canonicalize(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn control_characters_use_short_escapes_then_u_escapes() {
        let v = Value::String("\u{8}\u{c}\n\r\t\u{1}\u{1f} ".into());
        assert_eq!(canonicalize(&v).unwrap(), r#""\b\f\n\r\t\u0001\u001f ""#);
    }
}
