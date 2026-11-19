//! Reversible field transforms. Use a grid only when its inverse gives back
//! every original bit. Historic axes, arbitrary JSON floats and -0 survive.
use crate::{game::Frame, pack};
use serde_json::{Value, json};

use super::{Error, Result};

/// Range reduction and a Taylor polynomial, using only IEEE binary64
/// operations shared with JS. Trig library approximations differ by platform.
pub(super) fn sin(x: f64) -> f64 {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    let mut a = x - TAU * (x / TAU + 0.5).floor();
    if a > FRAC_PI_2 {
        a = PI - a;
    }
    if a < -FRAC_PI_2 {
        a = -PI - a;
    }
    let z = -a * a;
    let mut p = 1.0;
    for k in (1..=9).rev() {
        p = 1.0 + z * p / ((2 * k) * (2 * k + 1)) as f64;
    }
    a * p
}

fn yaw(q: i64) -> f64 {
    use std::f64::consts::{PI, TAU};
    let a = q as f64 * TAU / 16384.0;
    if a > PI { a - TAU } else { a }
}

fn vec3(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
}

pub(crate) fn rotated(base: [f64; 3], delta: f64) -> Result<[u32; 2]> {
    let c = sin(delta + std::f64::consts::FRAC_PI_2);
    let s = sin(delta);
    pack::oct([
        base[0] * c - base[1] * s,
        base[0] * s + base[1] * c,
        base[2],
    ])
    .map_err(|_| Error::Invalid)
}

pub(super) fn prepare(frame: &Frame) -> Result<Value> {
    let mut row = serde_json::to_value(frame)?;
    for (name, rows) in row.as_object_mut().ok_or(Error::Invalid)? {
        let Some(rows) = rows.as_array_mut() else {
            continue;
        };
        let mut reference: Option<([f64; 3], f64)> = None;
        for (i, row) in rows.iter_mut().enumerate() {
            let obj = row.as_object_mut().ok_or(Error::Invalid)?;
            // The signed JSON writer omits zero/empty optional event fields.
            // A historic input can explicitly carry them.
            macro_rules! restore {
                ($p:expr) => {{
                    let p = $p;
                    if p.shots == Some(0) {
                        obj.insert("shots".into(), json!(0));
                    }
                    if p.fire == Some(0) {
                        obj.insert("fire".into(), json!(0));
                    }
                    if p.actions.as_ref().is_some_and(Vec::is_empty) {
                        obj.insert("actions".into(), json!([]));
                    }
                    if p.actions_ago.as_ref().is_some_and(Vec::is_empty) {
                        obj.insert("actions_ago".into(), json!([]));
                    }
                }};
            }
            if name == "players" {
                restore!(&frame.players[i]);
            }
            if name == "actors" {
                restore!(&frame.actors[i]);
            }
            if name == "hits" {
                let bits = frame.hits[i].pos.map(f64::to_bits);
                if let Some(slot) = frame
                    .players
                    .iter()
                    .position(|p| p.pos.map(f64::to_bits) == bits)
                {
                    obj.remove("pos");
                    obj.insert("$p".into(), json!(slot));
                }
            }
            let heading = obj.get("yaw").and_then(Value::as_f64);
            if let Some(h) = heading {
                let q = ((h / std::f64::consts::TAU * 16384.0).round() as i64).rem_euclid(16384);
                if yaw(q).to_bits() == h.to_bits() {
                    obj.remove("yaw");
                    obj.insert("$y".into(), json!(q));
                }
            }
            for k in ["aim", "forward", "up", "f", "u"] {
                let Some(v) = obj.get(k).and_then(vec3) else {
                    continue;
                };
                let Ok(q) = pack::oct(v) else { continue };
                if pack::unoct(q).map(f64::to_bits) != v.map(f64::to_bits) {
                    continue;
                }
                let mut xy = q.map(i64::from);
                let mut tag = format!("${k}");
                if matches!(name.as_str(), "players" | "actors") && k == "aim" {
                    if let (Some((base, h0)), Some(h)) = (reference, heading) {
                        // Only use the common orientation when it actually
                        // predicts both octahedral components closely.
                        if (h - h0).abs() <= std::f64::consts::TAU {
                            let pred = rotated(base, h - h0)?;
                            let delta = [xy[0] - i64::from(pred[0]), xy[1] - i64::from(pred[1])];
                            if delta.iter().all(|d| d.abs() <= 16) {
                                xy = delta;
                                tag = "$r".into();
                            }
                        }
                    } else if i == 0 {
                        reference = heading.map(|h| (v, h));
                    }
                }
                obj.remove(k);
                obj.insert(tag, json!(xy));
            }
        }
    }
    Ok(row)
}

pub(super) fn restore(mut row: Value) -> Result<Frame> {
    for (name, rows) in row.as_object_mut().ok_or(Error::Invalid)? {
        let Some(rows) = rows.as_array_mut() else {
            continue;
        };
        let mut reference: Option<([f64; 3], f64)> = None;
        for (i, row) in rows.iter_mut().enumerate() {
            let obj = row.as_object_mut().ok_or(Error::Invalid)?;
            if let Some(q) = obj.remove("$y") {
                let q = q
                    .as_i64()
                    .filter(|q| (0..16384).contains(q))
                    .ok_or(Error::Invalid)?;
                obj.insert("yaw".into(), json!(yaw(q)));
            }
            for (tag, k) in [
                ("$aim", "aim"),
                ("$forward", "forward"),
                ("$up", "up"),
                ("$f", "f"),
                ("$u", "u"),
                ("$r", "aim"),
            ] {
                let Some(q) = obj.remove(tag) else { continue };
                let [x, y] = serde_json::from_value::<[i64; 2]>(q)?;
                let heading = obj.get("yaw").and_then(Value::as_f64);
                let base = if tag == "$r" {
                    let (v, h0) = reference.ok_or(Error::Invalid)?;
                    rotated(v, heading.ok_or(Error::Invalid)? - h0)?
                } else {
                    [0, 0]
                };
                let get = |v: i64, b: u32| {
                    v.checked_add(i64::from(b))
                        .filter(|n| (0..=16382).contains(n))
                        .map(|n| n as u32)
                        .ok_or(Error::Invalid)
                };
                let v = pack::unoct([get(x, base[0])?, get(y, base[1])?]);
                if i == 0 && matches!(name.as_str(), "players" | "actors") && k == "aim" {
                    reference = heading.map(|h| (v, h));
                }
                obj.insert(k.into(), json!(v));
            }
        }
    }
    // Split borrows before restoring hits. Missing or invalid references fail.
    let positions: Vec<_> = row["players"]
        .as_array()
        .ok_or(Error::Invalid)?
        .iter()
        .map(|p| p["pos"].clone())
        .collect();
    if let Some(hits) = row.get_mut("hits").and_then(Value::as_array_mut) {
        for hit in hits {
            let obj = hit.as_object_mut().ok_or(Error::Invalid)?;
            if let Some(i) = obj.remove("$p") {
                let pos = i
                    .as_u64()
                    .and_then(|i| usize::try_from(i).ok())
                    .and_then(|i| positions.get(i))
                    .ok_or(Error::Invalid)?;
                obj.insert("pos".into(), pos.clone());
            }
        }
    }
    Ok(serde_json::from_value(row)?)
}
