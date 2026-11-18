//! A field's bytes as JSON, and JSON back to bytes, by its definition's
//! kind. Reading never fails on odd bytes (an enum past its names is a
//! number); writing refuses what the field can't hold.

use map::FourCC;
use serde_json::{Value, json};
use tags::defs::{Kind, fourcc_text, parse_fourcc};

/// The kind as text: `f32[3]`, `enum16`, `tag_ref`, `bytes[8]`.
pub fn kind_text(kind: &Kind) -> String {
    let n = |base: &str, count: u32| {
        if count == 1 {
            base.to_string()
        } else {
            format!("{base}[{count}]")
        }
    };
    match kind {
        Kind::Int {
            bytes,
            signed,
            count,
        } => n(
            &format!("{}{}", if *signed { "i" } else { "u" }, bytes * 8),
            *count,
        ),
        Kind::Float { count } => n("f32", *count),
        Kind::Enum { bytes, .. } => format!("enum{}", bytes * 8),
        Kind::Flags { bytes, .. } => format!("flags{}", bytes * 8),
        Kind::StringId => "string_id".into(),
        Kind::TagRef { .. } => "tag_ref".into(),
        Kind::Datum { .. } => "datum".into(),
        Kind::Raw => "raw".into(),
        Kind::RawSize { .. } => "raw_size".into(),
        Kind::Word => "word".into(),
        Kind::Bsp => "bsp".into(),
        Kind::Block(s) => format!("block of {} (0x{:x} bytes each)", s.name, s.size),
        Kind::Data { .. } => "data".into(),
        Kind::Struct(s) => format!("struct {}", s.name),
        Kind::Unknown { len, .. } => format!("bytes[{len}]"),
        Kind::Pad { len } => format!("pad[{len}]"),
    }
}

/// Bytes a value of `kind` occupies where it is.
pub fn width(kind: &Kind) -> u32 {
    kind.width()
}

/// Whether the field can be written as a value (blocks, data and structs
/// are structure: their elements are what is edited).
pub fn writable(kind: &Kind) -> bool {
    !matches!(kind, Kind::Block(_) | Kind::Data { .. } | Kind::Struct(_))
}

fn uint(b: &[u8]) -> u32 {
    let mut w = [0u8; 4];
    w[..b.len()].copy_from_slice(b);
    u32::from_le_bytes(w)
}

fn sint(b: &[u8]) -> i64 {
    let u = uint(b) as i64;
    let bits = 8 * b.len() as u32;
    if bits < 64 && u & (1 << (bits - 1)) != 0 {
        u - (1 << bits)
    } else {
        u
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn datum_json(d: u32) -> Value {
    if d == u32::MAX {
        Value::Null
    } else {
        json!(format!("{d:#010x}"))
    }
}

/// `bytes` (exactly [`width`] of them) as JSON.
pub fn read(kind: &Kind, bytes: &[u8]) -> Value {
    match kind {
        Kind::Int {
            bytes: w,
            signed,
            count,
        } => {
            let w = *w as usize;
            let vals: Vec<Value> = bytes
                .chunks(w)
                .take(*count as usize)
                .map(|c| {
                    if *signed {
                        json!(sint(c))
                    } else {
                        json!(uint(c))
                    }
                })
                .collect();
            if *count == 1 {
                vals[0].clone()
            } else {
                Value::Array(vals)
            }
        }
        Kind::Float { count } => {
            let vals: Vec<Value> = bytes
                .chunks(4)
                .take(*count as usize)
                .map(|c| json!(f32::from_le_bytes(c.try_into().unwrap())))
                .collect();
            if *count == 1 {
                vals[0].clone()
            } else {
                Value::Array(vals)
            }
        }
        Kind::Enum { values, .. } => {
            let v = uint(bytes);
            match values.get(v as usize) {
                Some(name) => json!(name),
                None => json!(v),
            }
        }
        Kind::Flags { bits, .. } => {
            let v = uint(bytes);
            let set: Vec<String> = (0..8 * bytes.len() as u32)
                .filter(|b| v >> b & 1 != 0)
                .map(|b| match bits.get(b as usize) {
                    Some(n) if n != "_" => n.clone(),
                    _ => format!("bit{b}"),
                })
                .collect();
            json!(set)
        }
        Kind::StringId | Kind::Word | Kind::Raw | Kind::RawSize { .. } => json!(uint(bytes)),
        Kind::TagRef { .. } => {
            let group = uint(&bytes[0..4]);
            json!({
                "group": if group == u32::MAX { Value::Null } else { json!(fourcc_text(FourCC::from_u32(group))) },
                "datum": datum_json(uint(&bytes[4..8])),
            })
        }
        Kind::Datum { .. } => datum_json(uint(bytes)),
        Kind::Block(_) | Kind::Data { .. } => json!({
            "count": uint(&bytes[0..4]),
            "address": format!("{:#010x}", uint(&bytes[4..8])),
        }),
        Kind::Struct(_) => Value::Null,
        Kind::Bsp | Kind::Unknown { .. } | Kind::Pad { .. } => json!(hex(bytes)),
    }
}

fn number(v: &Value, what: &str) -> Result<f64, String> {
    match v {
        Value::Number(n) => n.as_f64().ok_or_else(|| format!("{what}: not a number")),
        Value::String(s) => {
            let t = s.trim();
            let parsed = match t.strip_prefix("0x") {
                Some(h) => u64::from_str_radix(h, 16).ok().map(|x| x as f64),
                None => t.parse::<f64>().ok(),
            };
            parsed.ok_or_else(|| format!("{what}: {s:?} is not a number"))
        }
        other => Err(format!("{what}: {other} is not a number")),
    }
}

fn int_bytes(v: f64, bytes: u8, signed: bool, what: &str) -> Result<Vec<u8>, String> {
    if v.fract() != 0.0 {
        return Err(format!("{what}: {v} is not a whole number"));
    }
    let bits = 8 * u32::from(bytes);
    let (lo, hi) = if signed {
        (
            -(2f64.powi(bits as i32 - 1)),
            2f64.powi(bits as i32 - 1) - 1.0,
        )
    } else {
        (0.0, 2f64.powi(bits as i32) - 1.0)
    };
    if v < lo || v > hi {
        return Err(format!("{what}: {v} is outside {lo}..={hi}"));
    }
    let w = (v as i64) as u32;
    Ok(w.to_le_bytes()[..bytes as usize].to_vec())
}

fn several<'a>(v: &'a Value, count: u32, what: &str) -> Result<Vec<&'a Value>, String> {
    if count == 1 {
        return Ok(vec![v]);
    }
    match v {
        Value::Array(a) if a.len() == count as usize => Ok(a.iter().collect()),
        _ => Err(format!("{what}: needs an array of {count} values")),
    }
}

/// A datum from JSON: `"0xe1230045"`, a number, or null (none).
pub fn datum(v: &Value, what: &str) -> Result<u32, String> {
    if v.is_null() {
        return Ok(u32::MAX);
    }
    let d = number(v, what)?;
    if d.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&d) {
        return Err(format!("{what}: {v} is not a datum"));
    }
    Ok(d as u32)
}

/// JSON to the field's bytes. `current` is what it holds now (a tag
/// reference set to null keeps its group word); `group_of` names the group
/// of a datum, for references and their allowed groups.
pub fn encode(
    kind: &Kind,
    v: &Value,
    current: &[u8],
    group_of: &dyn Fn(u32) -> Option<FourCC>,
) -> Result<Vec<u8>, String> {
    let what = kind_text(kind);
    let what = what.as_str();
    let check_group = |d: u32, groups: &[FourCC]| -> Result<Option<FourCC>, String> {
        if d == u32::MAX {
            return Ok(None);
        }
        let g = group_of(d).ok_or_else(|| format!("{what}: no tag {d:#010x} in the loaded map"))?;
        if !groups.is_empty() && !groups.contains(&g) {
            return Err(format!(
                "{what}: {d:#010x} is a {}, the field takes {}",
                fourcc_text(g),
                groups
                    .iter()
                    .map(|g| fourcc_text(*g))
                    .collect::<Vec<_>>()
                    .join(" or ")
            ));
        }
        Ok(Some(g))
    };
    match kind {
        Kind::Int {
            bytes,
            signed,
            count,
        } => {
            let mut out = Vec::new();
            for x in several(v, *count, what)? {
                out.extend(int_bytes(number(x, what)?, *bytes, *signed, what)?);
            }
            Ok(out)
        }
        Kind::Float { count } => {
            let mut out = Vec::new();
            for x in several(v, *count, what)? {
                let f = number(x, what)? as f32;
                if !f.is_finite() {
                    return Err(format!("{what}: {x} is not a finite f32"));
                }
                out.extend(f.to_le_bytes());
            }
            Ok(out)
        }
        Kind::Enum { bytes, values } => {
            let n = match v {
                Value::String(s) if !s.starts_with("0x") && s.parse::<f64>().is_err() => {
                    values.iter().position(|x| x == s).ok_or_else(|| {
                        format!("{what}: {s:?} is not one of {}", values.join(" "))
                    })? as f64
                }
                other => number(other, what)?,
            };
            int_bytes(n, *bytes, false, what)
        }
        Kind::Flags { bytes, bits } => {
            let n = match v {
                Value::Array(names) => {
                    let mut w = 0u64;
                    for name in names {
                        let s = name
                            .as_str()
                            .ok_or_else(|| format!("{what}: flag names are strings"))?;
                        let b = bits
                            .iter()
                            .position(|x| x == s && x != "_")
                            .or_else(|| s.strip_prefix("bit").and_then(|n| n.parse().ok()))
                            .filter(|b| *b < 8 * *bytes as usize)
                            .ok_or_else(|| format!("{what}: no flag {s:?}"))?;
                        w |= 1 << b;
                    }
                    w as f64
                }
                other => number(other, what)?,
            };
            int_bytes(n, *bytes, false, what)
        }
        Kind::StringId | Kind::Word | Kind::Raw | Kind::RawSize { .. } => {
            int_bytes(number(v, what)?, 4, false, what)
        }
        Kind::TagRef { groups } => {
            let d = match v {
                Value::Object(o) => datum(o.get("datum").unwrap_or(&Value::Null), what)?,
                other => datum(other, what)?,
            };
            let explicit = v
                .get("group")
                .and_then(|g| g.as_str())
                .map(|g| parse_fourcc(g).ok_or_else(|| format!("{what}: {g:?} is not a group")))
                .transpose()?;
            let found = check_group(d, groups)?;
            if let (Some(e), Some(f)) = (explicit, found)
                && e != f
            {
                return Err(format!(
                    "{what}: {d:#010x} is a {}, not a {}",
                    fourcc_text(f),
                    fourcc_text(e)
                ));
            }
            let group = match found {
                Some(g) => g.as_u32(),
                // Null keeps its group word.
                None => uint(&current[0..4]),
            };
            let mut out = group.to_le_bytes().to_vec();
            out.extend(d.to_le_bytes());
            Ok(out)
        }
        Kind::Datum { groups } => {
            let d = datum(v, what)?;
            check_group(d, groups)?;
            Ok(d.to_le_bytes().to_vec())
        }
        Kind::Bsp | Kind::Unknown { .. } | Kind::Pad { .. } => {
            let s = v
                .as_str()
                .ok_or_else(|| format!("{what}: needs a hex string"))?;
            let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
            if s.len() != 2 * width(kind) as usize {
                return Err(format!("{what}: needs {} hex digits", 2 * width(kind)));
            }
            (0..s.len())
                .step_by(2)
                .map(|i| {
                    u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| format!("{what}: bad hex"))
                })
                .collect()
        }
        Kind::Block(_) | Kind::Data { .. } | Kind::Struct(_) => Err(format!(
            "{what} is structure: edit its fields (or elements, name[i])"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: u32) -> Option<FourCC> {
        None
    }

    #[test]
    fn ints_and_floats_round_trip() {
        let k = Kind::Int {
            bytes: 2,
            signed: true,
            count: 2,
        };
        let b = encode(&k, &json!([-2, 300]), &[0; 4], &none).unwrap();
        assert_eq!(read(&k, &b), json!([-2, 300]));
        assert!(encode(&k, &json!([1, 40000]), &[0; 4], &none).is_err());
        assert!(encode(&k, &json!(1), &[0; 4], &none).is_err());
        let k = Kind::Float { count: 3 };
        let b = encode(&k, &json!([0.5, 1, "2.25"]), &[0; 12], &none).unwrap();
        assert_eq!(read(&k, &b), json!([0.5, 1.0, 2.25]));
        let k = Kind::Int {
            bytes: 4,
            signed: false,
            count: 1,
        };
        let b = encode(&k, &json!("0xff00ff00"), &[0; 4], &none).unwrap();
        assert_eq!(read(&k, &b), json!(0xff00ff00u32));
    }

    #[test]
    fn enums_and_flags_by_name() {
        let k = Kind::Enum {
            bytes: 2,
            values: vec!["a".into(), "b".into()],
        };
        let b = encode(&k, &json!("b"), &[0; 2], &none).unwrap();
        assert_eq!(read(&k, &b), json!("b"));
        assert_eq!(read(&k, &[9, 0]), json!(9));
        assert!(encode(&k, &json!("c"), &[0; 2], &none).is_err());
        let k = Kind::Flags {
            bytes: 1,
            bits: vec!["_".into(), "lit".into()],
        };
        let b = encode(&k, &json!(["lit", "bit3"]), &[0], &none).unwrap();
        assert_eq!(b, vec![0b1010]);
        assert_eq!(read(&k, &b), json!(["lit", "bit3"]));
        assert!(encode(&k, &json!(["bit8"]), &[0], &none).is_err());
    }

    #[test]
    fn references_check_the_group() {
        let bitm = FourCC(*b"bitm");
        let shad = FourCC(*b"shad");
        let lookup = |d: u32| match d {
            0xe001_0002 => Some(bitm),
            0xe003_0004 => Some(shad),
            _ => None,
        };
        let k = Kind::TagRef { groups: vec![bitm] };
        let b = encode(&k, &json!("0xe0010002"), &[0; 8], &lookup).unwrap();
        assert_eq!(
            read(&k, &b),
            json!({"group": "bitm", "datum": "0xe0010002"})
        );
        assert!(encode(&k, &json!("0xe0030004"), &[0; 8], &lookup).is_err());
        assert!(encode(&k, &json!("0xe0050006"), &[0; 8], &lookup).is_err());
        // Null keeps the group word.
        let b = encode(&k, &Value::Null, &b, &lookup).unwrap();
        assert_eq!(read(&k, &b), json!({"group": "bitm", "datum": null}));
        let k = Kind::Datum { groups: vec![] };
        let b = encode(&k, &json!(0xe003_0004u32), &[0; 4], &lookup).unwrap();
        assert_eq!(read(&k, &b), json!("0xe0030004"));
    }

    #[test]
    fn unknown_bytes_as_hex_and_structure_refused() {
        let k = Kind::Unknown {
            len: 2,
            seen: String::new(),
        };
        let b = encode(&k, &json!("0a ff"), &[0; 2], &none).unwrap();
        assert_eq!(read(&k, &b), json!("0aff"));
        assert!(encode(&k, &json!("0a"), &[0; 2], &none).is_err());
        let k = Kind::Data { align: 4 };
        assert!(encode(&k, &json!(1), &[0; 8], &none).is_err());
        assert!(!writable(&k));
    }
}
