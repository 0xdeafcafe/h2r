//! An independent replay segment. Stored bytes are Brotli (served with
//! Content-Encoding: br). The inflated H2AR stream transposes the complete
//! Frame tree by field and array ordinal across the segment's timeline.
mod motion;
mod numbers;

use crate::game::Frame;
use serde_json::{Map, Number, Value};
use std::{collections::BTreeSet, io::Read};
use thiserror::Error;

pub const MAX_FRAMES: usize = 900;
pub const SPAN_TICKS: u64 = 900;
/// Cap working memory in both storage backends. Count the canonical source
/// frame JSON bytes, which also accounts for long strings and dense rosters.
pub const MAX_SOURCE_BYTES: usize = 8 << 20;

/// Whether the next frame must be the first frame of a fresh segment.
/// A single legal source frame always makes progress.
pub fn starts_segment(
    open: &[Frame],
    source_bytes: usize,
    next: &Frame,
    next_bytes: usize,
) -> bool {
    let (Some(first), Some(last)) = (open.first(), open.last()) else {
        return false;
    };
    next.game_id != first.game_id
        || next.cut.is_some()
        || next.tick < last.tick
        || next.tick.saturating_sub(first.tick) >= SPAN_TICKS
        || open.len() >= MAX_FRAMES
        || source_bytes.saturating_add(next_bytes) > MAX_SOURCE_BYTES
}
const MAGIC: &[u8] = b"H2AR\x01";
const MAX_BODY: usize = 16 << 20;
const MAX_NODES: usize = 2_000_000;
const MAX_STRINGS: usize = 16 << 20;
const MAX_WIDTH: usize = 4096;

#[derive(Debug, Error)]
pub enum Error {
    #[error("malformed or unsupported replay archive")]
    Invalid,
    #[error("replay segment exceeds its format limits")]
    Limit,
    #[error("invalid replay frame: {0}")]
    Frame(#[from] serde_json::Error),
    #[error("replay compression: {0}")]
    Compression(#[from] std::io::Error),
    #[error("archived frame differs from source at offset {0}")]
    Mismatch(usize),
}
type Result<T> = std::result::Result<T, Error>;
pub type ArchiveError = Error;

fn put(out: &mut Vec<u8>, mut n: u64) {
    while n >= 128 {
        out.push(n as u8 | 128);
        n >>= 7;
    }
    out.push(n as u8);
}
fn string(out: &mut Vec<u8>, s: &str) {
    put(out, s.len() as u64);
    out.extend(s.as_bytes());
}
struct Reader<'a> {
    bytes: &'a [u8],
    nodes: usize,
    strings: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let (a, b) = self.bytes.split_at_checked(n).ok_or(Error::Invalid)?;
        self.bytes = b;
        Ok(a)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn var(&mut self) -> Result<u64> {
        let mut n = 0;
        for s in (0..64).step_by(7) {
            let b = self.byte()?;
            if s == 63 && b > 1 {
                return Err(Error::Invalid);
            }
            n |= u64::from(b & 127) << s;
            if b < 128 {
                return Ok(n);
            }
        }
        Err(Error::Invalid)
    }
    fn count(&mut self, max: usize) -> Result<usize> {
        usize::try_from(self.var()?)
            .ok()
            .filter(|&x| x <= max)
            .ok_or(Error::Limit)
    }
    fn string(&mut self) -> Result<&'a str> {
        let len = self.count(MAX_BODY)?;
        std::str::from_utf8(self.take(len)?).map_err(|_| Error::Invalid)
    }
    fn charge_strings(&mut self, n: usize) -> Result<()> {
        self.strings = self.strings.checked_sub(n).ok_or(Error::Limit)?;
        Ok(())
    }
}

fn kind(v: &Value) -> Result<u8> {
    Ok(match v {
        Value::Object(_) => 0,
        Value::Array(_) => 1,
        Value::String(_) => 2,
        Value::Bool(_) => 3,
        Value::Number(n) if n.is_f64() => 5,
        Value::Number(_) => 4,
        Value::Null => return Err(Error::Invalid),
    })
}

// Each path contains one schema type. Numeric columns preserve the schema's
// unsigned integers, and float columns select an exactly reversible grid.
fn write_node(rows: Vec<&Value>, out: &mut Vec<u8>, remaining: &mut usize) -> Result<()> {
    *remaining = remaining.checked_sub(rows.len()).ok_or(Error::Limit)?;
    let tag = kind(rows.first().ok_or(Error::Invalid)?)?;
    if !rows.iter().all(|r| kind(r).ok() == Some(tag)) {
        return Err(Error::Invalid);
    }
    out.push(tag);
    match tag {
        0 => {
            let keys: BTreeSet<_> = rows
                .iter()
                .flat_map(|r| r.as_object().unwrap().keys())
                .collect();
            put(out, keys.len() as u64);
            for k in keys {
                string(out, k);
                let vals: Vec<_> = rows.iter().filter_map(|r| r.get(k)).collect();
                if vals.len() == rows.len() {
                    out.push(0)
                } else {
                    out.push(1);
                    for part in rows.chunks(8) {
                        out.push(
                            part.iter()
                                .enumerate()
                                .fold(0, |b, (i, r)| b | ((r.get(k).is_some() as u8) << i)),
                        );
                    }
                }
                write_node(vals, out, remaining)?;
            }
        }
        1 => {
            let lens: Vec<_> = rows
                .iter()
                .map(|r| r.as_array().unwrap().len() as i64)
                .collect();
            let width = *lens.iter().max().unwrap() as usize;
            if width > MAX_WIDTH {
                return Err(Error::Limit);
            }
            numbers::encode(&lens, out);
            for i in 0..width {
                write_node(
                    rows.iter().filter_map(|r| r.get(i)).collect(),
                    out,
                    remaining,
                )?;
            }
        }
        2 => {
            let dict: Vec<_> = rows
                .iter()
                .map(|r| r.as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            put(out, dict.len() as u64);
            for s in &dict {
                string(out, s);
            }
            if dict.len() > 1 {
                numbers::encode(
                    &rows
                        .iter()
                        .map(|r| dict.binary_search(&r.as_str().unwrap()).unwrap() as i64)
                        .collect::<Vec<_>>(),
                    out,
                );
            }
        }
        3 => numbers::encode(
            &rows
                .iter()
                .map(|r| r.as_bool().unwrap() as i64)
                .collect::<Vec<_>>(),
            out,
        ),
        4 => {
            // Synthetic signed columns ($r) and native unsigned ticks share
            // the same integer coder. Only signed columns can contain negatives.
            let signed = rows.iter().any(|r| r.as_u64().is_none());
            out.push(signed as u8);
            let vals = rows
                .iter()
                .map(|r| {
                    if signed {
                        r.as_i64()
                    } else {
                        r.as_u64().map(|n| n as i64)
                    }
                    .ok_or(Error::Invalid)
                })
                .collect::<Result<Vec<_>>>()?;
            numbers::encode(&vals, out);
        }
        5 => {
            let v: Vec<_> = rows.iter().map(|r| r.as_f64().unwrap()).collect();
            let mut scale = 0;
            for s in [1, 16, 32, 63, 128, 255, 256, 1024, 65536] {
                if v.iter().all(|&x| {
                    let y = x * s as f64;
                    y.abs() <= ((1u64 << 52) as f64)
                        && ((y.round() as i64) as f64 / s as f64).to_bits() == x.to_bits()
                }) {
                    scale = s;
                    break;
                }
            }
            put(out, scale);
            numbers::encode(
                &v.iter()
                    .map(|&x| {
                        if scale == 0 {
                            x.to_bits() as i64
                        } else {
                            (x * scale as f64).round() as i64
                        }
                    })
                    .collect::<Vec<_>>(),
                out,
            );
        }
        _ => unreachable!(),
    }
    if out.len() > MAX_BODY {
        return Err(Error::Limit);
    }
    Ok(())
}

fn read_node(r: &mut Reader<'_>, n: usize, depth: usize) -> Result<Vec<Value>> {
    if depth > 10 || n == 0 {
        return Err(Error::Invalid);
    }
    r.nodes = r.nodes.checked_sub(n).ok_or(Error::Limit)?;
    Ok(match r.byte()? {
        0 => {
            let mut objects = vec![Map::new(); n];
            let keys = r.count(128)?;
            let mut seen = BTreeSet::new();
            for _ in 0..keys {
                let k = r.string()?;
                if !seen.insert(k) {
                    return Err(Error::Invalid);
                }
                let mask = match r.byte()? {
                    0 => None,
                    1 => Some(r.take(n.div_ceil(8))?),
                    _ => return Err(Error::Invalid),
                };
                let has = |i: usize| mask.is_none_or(|m| m[i / 8] & (1 << (i % 8)) != 0);
                let ct = (0..n).filter(|&i| has(i)).count();
                r.charge_strings(k.len().checked_mul(ct).ok_or(Error::Limit)?)?;
                let col = read_node(r, ct, depth + 1)?;
                for (obj, value) in objects
                    .iter_mut()
                    .enumerate()
                    .filter(|(i, _)| has(*i))
                    .map(|(_, o)| o)
                    .zip(col)
                {
                    obj.insert(k.to_owned(), value);
                }
            }
            objects.into_iter().map(Value::Object).collect()
        }
        1 => {
            let lens = numbers::decode(r, n)?;
            if lens.iter().any(|&v| !(0..=MAX_WIDTH as i64).contains(&v)) {
                return Err(Error::Limit);
            }
            let width = *lens.iter().max().unwrap();
            if lens.iter().map(|&v| v as usize).sum::<usize>() > r.nodes {
                return Err(Error::Limit);
            }
            let mut arrays = vec![Vec::new(); n];
            for i in 0..width {
                let ct = lens.iter().filter(|&&l| l > i).count();
                for ((a, _), v) in arrays
                    .iter_mut()
                    .zip(&lens)
                    .filter(|(_, l)| **l > i)
                    .zip(read_node(r, ct, depth + 1)?)
                {
                    a.push(v);
                }
            }
            arrays.into_iter().map(Value::Array).collect()
        }
        2 => {
            let d = r.count(n)?;
            if d == 0 {
                return Err(Error::Invalid);
            }
            let dict = (0..d).map(|_| r.string()).collect::<Result<Vec<_>>>()?;
            let indexes = if d == 1 {
                vec![0; n]
            } else {
                numbers::decode(r, n)?
            };
            indexes
                .into_iter()
                .map(|i| {
                    let s = usize::try_from(i)
                        .ok()
                        .and_then(|i| dict.get(i))
                        .ok_or(Error::Invalid)?;
                    r.charge_strings(s.len())?;
                    Ok(Value::from(*s))
                })
                .collect::<Result<_>>()?
        }
        3 => numbers::decode(r, n)?
            .into_iter()
            .map(|b| match b {
                0 => Ok(Value::Bool(false)),
                1 => Ok(Value::Bool(true)),
                _ => Err(Error::Invalid),
            })
            .collect::<Result<_>>()?,
        4 => {
            let signed = r.byte()?;
            if signed > 1 {
                return Err(Error::Invalid);
            }
            numbers::decode(r, n)?
                .into_iter()
                .map(|i| {
                    if signed == 1 {
                        Value::from(i)
                    } else {
                        Value::from(i as u64)
                    }
                })
                .collect()
        }
        5 => {
            let scale = r.count(65536)?;
            numbers::decode(r, n)?
                .into_iter()
                .map(|q| {
                    let x = if scale == 0 {
                        f64::from_bits(q as u64)
                    } else {
                        q as f64 / scale as f64
                    };
                    Number::from_f64(x).map(Value::Number).ok_or(Error::Invalid)
                })
                .collect::<Result<_>>()?
        }
        _ => return Err(Error::Invalid),
    })
}

/// Encode one complete, independently decodable segment in chronological
/// source order. Storage partitions by tick span, frame count and cuts.
pub fn encode(frames: &[Frame]) -> Result<Vec<u8>> {
    if frames.is_empty() || frames.len() > MAX_FRAMES {
        return Err(Error::Limit);
    }
    let rows = frames
        .iter()
        .map(motion::prepare)
        .collect::<Result<Vec<_>>>()?;
    let mut body = MAGIC.to_vec();
    put(&mut body, rows.len() as u64);
    let mut remaining = MAX_NODES;
    write_node(rows.iter().collect(), &mut body, &mut remaining)?;
    drop(rows);
    let mut stored = Vec::new();
    brotli::CompressorReader::new(body.as_slice(), 4096, 10, 18).read_to_end(&mut stored)?;
    Ok(stored)
}

pub fn decode(stored: &[u8]) -> Result<Vec<Frame>> {
    if stored.len() > MAX_BODY + 1024 {
        return Err(Error::Limit);
    }
    let mut body = Vec::new();
    brotli::Decompressor::new(stored, 4096)
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut body)?;
    if body.len() > MAX_BODY {
        return Err(Error::Limit);
    }
    let mut r = Reader {
        bytes: body.strip_prefix(MAGIC).ok_or(Error::Invalid)?,
        nodes: MAX_NODES,
        strings: MAX_STRINGS,
    };
    let n = r.count(MAX_FRAMES)?;
    let rows = read_node(&mut r, n, 0)?;
    if !r.bytes.is_empty() {
        return Err(Error::Invalid);
    }
    rows.into_iter().map(motion::restore).collect()
}

/// Round trip against the *source* decoder. Typed equality includes explicit empty
/// events. Round-trip decimal serialization distinguishes -0 from +0.
pub fn verify(source: &[Frame], stored: &[u8]) -> Result<()> {
    let frames = decode(stored)?;
    if source.len() != frames.len() {
        return Err(Error::Mismatch(frames.len().min(source.len())));
    }
    for (i, (a, b)) in source.iter().zip(&frames).enumerate() {
        if a != b || serde_json::to_vec(a)? != serde_json::to_vec(b)? {
            return Err(Error::Mismatch(i));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "archive/tests.rs"]
mod tests;
