//! Starting points for new definitions: a group's schema layout with every
//! other word an `unknown` span, and statistics of retail values to
//! summarise those spans (`seen=`) and merge zero runs. A person (or the
//! XBE's readers) names the fields from there.

use super::{FieldDef, Kind, Record, StructDef};
use crate::schema::{Field, Struct};
use std::collections::{BTreeMap, HashSet};

/// A definition with the schema's fields (named after their kind and
/// offset) and every other 4-byte word (or the bytes up to the next field)
/// an unknown span.
pub fn from_schema(name: &str, s: &Struct) -> StructDef {
    let mut fields = Vec::new();
    let mut at = 0;
    let gap = |from: u32, to: u32, fields: &mut Vec<FieldDef>| {
        let mut o = from;
        while o < to {
            let next = ((o / 4) + 1) * 4;
            let end = next.min(to);
            fields.push(FieldDef {
                offset: o,
                name: format!("unknown_{o:x}"),
                kind: Kind::Unknown {
                    len: end - o,
                    seen: String::new(),
                },
                meaning: String::new(),
                default: None,
            });
            o = end;
        }
    };
    for (&o, f) in &s.fields {
        gap(at, o, &mut fields);
        let (name, kind) = match f {
            Field::Block(c) if c.size == 1 && c.fields.is_empty() => {
                (format!("data_{o:x}"), Kind::Data { align: c.align })
            }
            Field::Block(c) => (
                format!("block_{o:x}"),
                Kind::Block(Box::new(from_schema(&format!("block_{o:x}"), c))),
            ),
            Field::TagRef => (format!("tagref_{o:x}"), Kind::TagRef { groups: Vec::new() }),
            Field::TagId => (format!("datum_{o:x}"), Kind::Datum { groups: Vec::new() }),
            Field::StringId => (format!("string_{o:x}"), Kind::StringId),
            Field::Raw => (format!("raw_{o:x}"), Kind::Raw),
            Field::RawSize(of) => (format!("raw_size_{o:x}"), Kind::RawSize { of: *of }),
            Field::Word => (format!("word_{o:x}"), Kind::Word),
            Field::Bsp => (format!("bsp_{o:x}"), Kind::Bsp),
        };
        let w = kind.width();
        fields.push(FieldDef {
            offset: o,
            name,
            kind,
            meaning: String::new(),
            default: None,
        });
        at = o + w;
    }
    gap(at, s.size, &mut fields);
    StructDef {
        name: name.to_owned(),
        size: s.size,
        align: s.align,
        fields,
    }
}

/// Retail values of one unknown span.
#[derive(Clone, Debug, Default)]
pub struct SpanStats {
    /// Instances seen.
    pub n: u64,
    /// Instances all zero.
    pub zero: u64,
    /// Distinct values (at most [`SpanStats::CAP`] kept).
    pub distinct: HashSet<Vec<u8>>,
    /// Some instance did not fit the cap.
    pub overflow: bool,
    /// Every 4-byte instance is a plausible float (zero, or exponent
    /// between 2^-20 and 2^20, not NaN).
    pub floaty: bool,
    /// Every 4-byte instance is `0xffffffff` or zero or a small integer.
    pub small: bool,
}

impl SpanStats {
    /// Distinct values kept.
    pub const CAP: usize = 64;

    /// Adds an instance.
    pub fn add(&mut self, b: &[u8]) {
        if self.n == 0 {
            self.floaty = b.len() == 4;
            self.small = b.len() == 4;
        }
        self.n += 1;
        if b.iter().all(|x| *x == 0) {
            self.zero += 1;
        }
        if b.len() == 4 {
            let w = u32::from_le_bytes(b.try_into().expect("4"));
            let e = (w >> 23) & 0xff;
            if !(w & 0x7fff_ffff == 0 || (107..=147).contains(&e)) {
                self.floaty = false;
            }
            if !(w < 0x1_0000 || w == u32::MAX || w >= 0xffff_0000) {
                self.small = false;
            }
        }
        if self.distinct.len() < Self::CAP || self.distinct.contains(b) {
            self.distinct.insert(b.to_vec());
        } else {
            self.overflow = true;
        }
    }

    /// `seen=` text: `zero`, `const`, `<n>` distinct values, or `many`.
    /// Never the values.
    pub fn seen(&self) -> String {
        if self.n == 0 {
            "none".into()
        } else if self.zero == self.n {
            "zero".into()
        } else if self.distinct.len() == 1 {
            "const".into()
        } else if self.overflow {
            "many".into()
        } else {
            self.distinct.len().to_string()
        }
    }

    /// A hint at the type, for whoever names it (not a name).
    pub fn hint(&self) -> &'static str {
        if self.zero == self.n {
            ""
        } else if self.small {
            "small integers"
        } else if self.floaty {
            "floats"
        } else {
            ""
        }
    }
}

/// Statistics per unknown span, by path (as [`super::unknowns`] names them).
pub type Stats = BTreeMap<String, SpanStats>;

/// Adds a decoded tag's unknown spans to `stats`.
pub fn collect(def: &StructDef, rec: &Record, group: &str, stats: &mut Stats) {
    let mut spans = Vec::new();
    super::unknowns(def, rec, group, &mut spans);
    for (p, b) in spans {
        stats.entry(p).or_default().add(b);
    }
}

/// `def` with each unknown span's `seen=` filled in from `stats` and runs
/// of all-zero spans merged into one; type hints go in the meaning.
pub fn annotate(def: &StructDef, path: &str, stats: &Stats) -> StructDef {
    let mut out: Vec<FieldDef> = Vec::new();
    for f in &def.fields {
        let p = format!("{path}/{}", f.name);
        let mut f = f.clone();
        match &mut f.kind {
            Kind::Unknown { len, seen } => {
                let s = stats.get(&p);
                *seen = s.map_or("none".into(), SpanStats::seen);
                f.meaning = s.map_or("", SpanStats::hint).to_owned();
                if seen == "zero"
                    && let Some(prev) = out.last_mut()
                    && let Kind::Unknown { len: pl, seen: ps } = &mut prev.kind
                    && ps == "zero"
                {
                    *pl += *len;
                    continue;
                }
            }
            Kind::Block(s) => **s = annotate(s, &format!("{p}[]"), stats),
            Kind::Struct(s) => **s = annotate(s, &p, stats),
            _ => {}
        }
        out.push(f);
    }
    StructDef {
        fields: out,
        ..def.clone()
    }
}

/// Unknown span values of a record, by path, for a quick look in tests.
pub fn values_at<'r>(def: &StructDef, rec: &'r Record, want: &str) -> Vec<&'r [u8]> {
    let mut spans = Vec::new();
    super::unknowns(def, rec, "", &mut spans);
    spans
        .into_iter()
        .filter(|(p, _)| p == want)
        .map(|(_, b)| b)
        .collect()
}
