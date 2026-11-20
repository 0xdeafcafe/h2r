//! RFC 8785 JSON Canonicalization Scheme: the exact bytes every signature
//! covers (docs/live/live-security.md §2).
//!
//! Objects are sorted by key as UTF-16 code units, strings use the minimal
//! ECMAScript escapes, and numbers are written the way ECMAScript's
//! `Number.prototype.toString` writes them.

use std::fmt::Write;

use serde_json::Value;

/// The canonical form of `value`.
pub fn canonicalize(value: &Value) -> String {
    let mut out = String::new();
    write_value(&mut out, value);
    out
}

/// Canonical JSON for anything serde can turn into a JSON value.
pub fn to_canonical<T: serde::Serialize>(value: &T) -> Result<String, serde_json::Error> {
    Ok(canonicalize(&serde_json::to_value(value)?))
}

fn write_value(out: &mut String, value: &Value) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => write_number(out, n),
        Value::String(s) => write_string(out, s),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(out, item);
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
                write_value(out, v);
            }
            out.push('}');
        }
    }
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

/// Integers that are exact in an IEEE double print as integers; everything
/// else goes through the double, as JavaScript would see it.
fn write_number(out: &mut String, n: &serde_json::Number) {
    const EXACT: u64 = 1 << 53;
    if let Some(u) = n.as_u64().filter(|u| *u <= EXACT) {
        let _ = write!(out, "{u}");
    } else if let Some(i) = n.as_i64().filter(|i| i.unsigned_abs() <= EXACT) {
        let _ = write!(out, "{i}");
    } else {
        // serde_json never holds NaN or infinities.
        write_f64(out, n.as_f64().unwrap_or(0.0));
    }
}

/// ECMAScript `Number::toString(x)` (ECMA-262 §6.1.6.1.20) for finite `x`.
/// Rust's `{:e}` gives the same shortest round-trip digits; only the layout
/// differs.
fn write_f64(out: &mut String, x: f64) {
    if x == 0.0 {
        out.push('0');
        return;
    }
    if x < 0.0 {
        out.push('-');
    }
    let sci = format!("{:e}", x.abs());
    let (mantissa, exp) = sci.split_once('e').expect("{:e} always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // x = 0.d1d2..dk * 10^n
    let n = exp.parse::<i32>().expect("{:e} exponent is an integer") + 1;
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let e = n - 1;
        let _ = write!(out, "e{}{}", if e < 0 { '-' } else { '+' }, e.abs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(x: f64) -> String {
        let mut s = String::new();
        write_f64(&mut s, x);
        s
    }

    #[test]
    fn numbers_match_ecmascript() {
        // RFC 8785 appendix B and ECMAScript edge cases.
        for (x, want) in [
            (0.0, "0"),
            (-0.0, "0"),
            (5e-324, "5e-324"),
            (1.7976931348623157e308, "1.7976931348623157e+308"),
            (9007199254740992.0, "9007199254740992"),
            (295147905179352830000.0, "295147905179352830000"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (333333333.3333333, "333333333.3333333"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (4.5, "4.5"),
            (0.002, "0.002"),
            (-1.5, "-1.5"),
            (1e30, "1e+30"),
            (123e-30, "1.23e-28"),
        ] {
            assert_eq!(num(x), want, "{x:e}");
        }
    }

    #[test]
    fn rfc_8785_example() {
        // RFC 8785 §3.2.2's input, parsed and canonicalized.
        let input = r#"{
            "numbers": [333333333.33333329, 1E30, 4.50, 2e-3, 0.000000000000000000000000001],
            "string": "€$\u000F\u000aA'B\"\\\\\"\/",
            "literals": [null, true, false]
        }"#;
        let v: Value = serde_json::from_str(input).unwrap();
        assert_eq!(
            canonicalize(&v),
            r#"{"literals":[null,true,false],"numbers":[333333333.3333333,1e+30,4.5,0.002,1e-27],"string":"€$\u000f\nA'B\"\\\\\"/"}"#
        );
    }

    #[test]
    fn keys_sort_by_utf16_units() {
        // U+10000 is D800 DC00 in UTF-16, which sorts before U+E000 even
        // though its UTF-8 bytes sort after.
        let v = serde_json::json!({"\u{e000}": 1, "\u{10000}": 2, "a": 3, "": 4});
        assert_eq!(
            canonicalize(&v),
            "{\"\":4,\"a\":3,\"\u{10000}\":2,\"\u{e000}\":1}"
        );
    }

    #[test]
    fn integers_stay_integers() {
        let v = serde_json::json!([0, -7, 9007199254740992u64, 18446744073709551615u64]);
        assert_eq!(
            canonicalize(&v),
            "[0,-7,9007199254740992,18446744073709552000]"
        );
    }
}
