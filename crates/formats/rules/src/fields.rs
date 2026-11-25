//! Tag reads: `tag_int(tag, "field")` and `tag_fixed(tag, "field")`.
//! See `docs/game/rules-world-api.md`, "Advanced data".
//!
//! A script names the tag and the field as constants written in the call.
//! The field is a path of our own names (`maximum_shield_vitality`,
//! `damage.threshold`), never an offset. A host resolves each path at load
//! against the typed tag definitions (`tags::defs`) to an offset and a
//! type and refuses the script, naming the field, when the definitions
//! lack it. The resolution is a [`Schema`] the host supplies, so this
//! crate holds no tag layout and a test runs over an invented one.
//!
//! Every machine in a game loads the same map, so a read gives the same
//! value on each. A float field is rounded once to a `fixed`.

use crate::caps::id;
use crate::ir::{Ir, Op};
use crate::ty::fixed;

/// The longest field path, in bytes.
pub const MAX_PATH_BYTES: usize = 64;
/// The most names one path joins.
pub const MAX_SEGMENTS: usize = 6;

/// Whether `path` is a field path: one to [`MAX_SEGMENTS`] names of
/// lower-case letters, digits and underscores, each starting with a letter,
/// joined by dots, in at most [`MAX_PATH_BYTES`] bytes. The reason when it
/// is not.
pub fn valid_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return Err(format!("a field path is 1 to {MAX_PATH_BYTES} bytes"));
    }
    let names: Vec<&str> = path.split('.').collect();
    if names.len() > MAX_SEGMENTS {
        return Err(format!("a field path joins at most {MAX_SEGMENTS} names"));
    }
    for n in names {
        let mut chars = n.chars();
        let first = chars.next();
        if !first.is_some_and(|c| c.is_ascii_lowercase())
            || !chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(format!(
                "\"{n}\" is not a field name: lower-case letters, digits and underscores, starting with a letter"
            ));
        }
    }
    Ok(())
}

/// What a resolved field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An integer of 1, 2 or 4 bytes.
    Int { bytes: u8, signed: bool },
    /// An IEEE single.
    Float,
}

/// A field resolved to where a tag's bytes hold it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// The byte offset in the tag's root element.
    pub offset: u32,
    pub kind: Kind,
}

impl Field {
    /// The bytes the field spans.
    pub fn width(&self) -> usize {
        match self.kind {
            Kind::Int { bytes, .. } => bytes as usize,
            Kind::Float => 4,
        }
    }

    /// Whether `entry` reads this kind of field: `tag_int` an integer,
    /// `tag_fixed` an integer or a float.
    pub fn takes(&self, entry: u16) -> bool {
        match self.kind {
            Kind::Int { bytes, .. } => matches!(bytes, 1 | 2 | 4) && is_read(entry),
            Kind::Float => entry == id::TAG_FIXED,
        }
    }

    /// The field's value from the tag's root element, as `tag_int` or
    /// `tag_fixed` (`entry`) answers it: an integer field is an `int` or,
    /// for `tag_fixed`, that whole number as a `fixed`; a float field is
    /// only `tag_fixed`'s, rounded once to the nearest 1/65536 and
    /// saturating, 0 for a NaN. `None` when the bytes end before the field
    /// or the entry does not take this kind of field.
    pub fn read(&self, entry: u16, bytes: &[u8]) -> Option<i32> {
        if !self.takes(entry) {
            return None;
        }
        let at = self.offset as usize;
        let raw = bytes.get(at..at.checked_add(self.width())?)?;
        match (self.kind, entry) {
            (Kind::Int { bytes: n, signed }, id::TAG_INT | id::TAG_FIXED) => {
                let v = match (n, signed) {
                    (1, false) => i64::from(raw[0]),
                    (1, true) => i64::from(raw[0] as i8),
                    (2, false) => i64::from(u16::from_le_bytes([raw[0], raw[1]])),
                    (2, true) => i64::from(i16::from_le_bytes([raw[0], raw[1]])),
                    (4, false) => i64::from(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
                    (4, true) => i64::from(i32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
                    _ => return None,
                };
                let v = v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
                Some(if entry == id::TAG_INT { v } else { fixed::from_int(v) })
            }
            (Kind::Float, id::TAG_FIXED) => {
                let f = f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
                Some(if f.is_nan() {
                    0
                } else {
                    (f64::from(f) * f64::from(fixed::ONE)).round().clamp(f64::from(i32::MIN), f64::from(i32::MAX))
                        as i32
                })
            }
            _ => None,
        }
    }
}

/// The host's typed tag definitions, as far as a script reads them.
pub trait Schema {
    /// The field `path` of the tag at `tag` (the script's own path text),
    /// or why the definitions lack it: the tag is not on the map, or the
    /// field is not there, or it is not a single integer or float.
    fn field(&self, tag: &str, path: &str) -> Result<Field, String>;
}

/// One `tag_int` or `tag_fixed` call of a script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Use {
    pub handler: usize,
    pub pc: usize,
    /// [`id::TAG_INT`] or [`id::TAG_FIXED`].
    pub entry: u16,
    /// The tag table's index, and the tag path.
    pub tag: u16,
    pub path: String,
    /// The field path.
    pub field: String,
}

/// The tag table index and the string table index a read's two constants
/// hold: the two ops before the call at `pc`, in either order. The verifier
/// holds every read to this ([`crate::verify`]).
pub(crate) fn constants(code: &[Op], pc: usize) -> Option<(i32, i32)> {
    let Op::Call { args, .. } = code.get(pc)? else {
        return None;
    };
    let &[tag, field] = args.as_slice() else {
        return None;
    };
    let before = |n: usize| code.get(pc.checked_sub(n)?);
    let (a, b) = (before(2)?, before(1)?);
    let value = |reg| match (a, b) {
        (&Op::Const { dst, value }, _) if dst == reg => Some(value),
        (_, &Op::Const { dst, value }) if dst == reg => Some(value),
        _ => None,
    };
    if tag == field {
        return None;
    }
    Some((value(tag)?, value(field)?))
}

/// Whether `entry` is a tag read.
pub fn is_read(entry: u16) -> bool {
    matches!(entry, id::TAG_INT | id::TAG_FIXED)
}

/// Every tag read of a script, in handler and op order. A read the
/// verifier would refuse is left out.
pub fn uses(ir: &Ir) -> Vec<Use> {
    let mut out = Vec::new();
    for (handler, hd) in ir.handlers.iter().enumerate() {
        for (pc, op) in hd.code.iter().enumerate() {
            let Op::Call { entry, .. } = op else { continue };
            if !is_read(*entry) {
                continue;
            }
            let Some((tag, field)) = constants(&hd.code, pc) else {
                continue;
            };
            let (Some(t), Some(f)) = (
                usize::try_from(tag).ok().and_then(|i| ir.tags.get(i)),
                usize::try_from(field).ok().and_then(|i| ir.strings.get(i)),
            ) else {
                continue;
            };
            out.push(Use {
                handler,
                pc,
                entry: *entry,
                tag: tag as u16,
                path: t.path.clone(),
                field: f.clone(),
            });
        }
    }
    out
}

/// A read the definitions do not satisfy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub tag: String,
    pub field: String,
    /// What a person reads: the field and the tag it is named for, and why.
    pub text: String,
}

/// Every read of `ir` whose field `schema` lacks, or whose kind its entry
/// does not take (`tag_int` of a float field), each once, in the script's
/// order. A script with no tag read has none.
pub fn check(ir: &Ir, schema: &dyn Schema) -> Vec<Problem> {
    let name = ir.header.name.as_str();
    let mut out: Vec<Problem> = Vec::new();
    for u in uses(ir) {
        let why = match schema.field(&u.path, &u.field) {
            Err(why) => Some(why),
            Ok(f) if !f.takes(u.entry) => Some(match f.kind {
                Kind::Float => "it is a float: read it with tag_fixed".to_string(),
                Kind::Int { .. } => "it is not a whole number of 1, 2 or 4 bytes".to_string(),
            }),
            Ok(_) => None,
        };
        let Some(why) = why else { continue };
        if out.iter().any(|p| p.tag == u.path && p.field == u.field) {
            continue;
        }
        out.push(Problem {
            text: format!("\"{}\" has no field \"{}\" that {name} can read: {why}", u.path, u.field),
            tag: u.path,
            field: u.field,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_names_joined_by_dots() {
        for ok in ["shield", "damage.threshold", "a1_b", "a.b.c.d.e.f"] {
            assert_eq!(valid_path(ok), Ok(()), "{ok}");
        }
        for bad in ["", "Shield", "1a", "a..b", ".a", "a.", "a b", "a-b", "a.b.c.d.e.f.g"] {
            assert!(valid_path(bad).is_err(), "{bad:?}");
        }
        assert!(valid_path(&"a".repeat(MAX_PATH_BYTES + 1)).is_err());
        assert!(valid_path(&"a".repeat(MAX_PATH_BYTES)).is_ok());
    }

    #[test]
    fn integers_read_by_width_and_sign_and_floats_round_once() {
        let int = |bytes, signed| Field { offset: 1, kind: Kind::Int { bytes, signed } };
        let data = [0u8, 0xff, 0xff, 0xff, 0xff, 9];
        assert_eq!(int(1, false).read(id::TAG_INT, &data), Some(255));
        assert_eq!(int(1, true).read(id::TAG_INT, &data), Some(-1));
        assert_eq!(int(2, false).read(id::TAG_INT, &data), Some(65535));
        assert_eq!(int(2, true).read(id::TAG_INT, &data), Some(-1));
        assert_eq!(int(4, false).read(id::TAG_INT, &data), Some(i32::MAX));
        assert_eq!(int(4, true).read(id::TAG_INT, &data), Some(-1));
        assert_eq!(int(1, false).read(id::TAG_FIXED, &data), Some(255 << 16));
        // The field ends past the bytes.
        assert_eq!(int(4, true).read(id::TAG_INT, &data[..4]), None);
        let float = Field { offset: 0, kind: Kind::Float };
        let mut bytes = 0.5f32.to_le_bytes().to_vec();
        assert_eq!(float.read(id::TAG_FIXED, &bytes), Some(fixed::ONE / 2));
        assert_eq!(float.read(id::TAG_INT, &bytes), None);
        bytes = f32::NAN.to_le_bytes().to_vec();
        assert_eq!(float.read(id::TAG_FIXED, &bytes), Some(0));
        bytes = 1e12f32.to_le_bytes().to_vec();
        assert_eq!(float.read(id::TAG_FIXED, &bytes), Some(i32::MAX));
        bytes = (1.0f32 / 3.0).to_le_bytes().to_vec();
        assert_eq!(float.read(id::TAG_FIXED, &bytes), Some(21845));
    }
}
