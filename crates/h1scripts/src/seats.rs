//! The scenario's unit seat-mapping table, built from the vehicles' seat
//! labels.
//!
//! A `unit_seat_mapping` value is `count << 16 | first`: `count` elements of
//! the scenario block [`MAPPINGS`] from `first`. Each element is a unit
//! definition datum and a mask of its seats (bit `i` is seat `i` of the
//! unit's seats block, [`UNIT_SEATS`]). V on 05a: `pelican_lc` is one
//! element, the pelican with bit 12, the one seat whose label is
//! `pelican_lc`; `pelican_p` is the pelican with bits 1 to 10, the seats
//! whose labels start `pelican_p` (`pelican_p_l01` ...). So retail Halo 2
//! matches a seat when its label starts with the script's text.
//!
//! Halo 1 names seats by role (`driver`, `gunner`, `passenger`,
//! `p-riderlf`, ...) on Halo 1's vehicles; the converted level drives Halo
//! 2's vehicles, whose labels read `<vehicle>_d`, `_p`, `_g`, `_p_l01`.
//! [`h2_label_matches`] maps one to the other by role.

use std::collections::HashMap;

use map::TagId;
use map::build::{Block, FixupKind, TagPayload, TagSet};

/// The scenario's seat-mapping block: (offset, element size); an element is
/// the unit definition datum `+0` and the seat mask `+4`.
pub const MAPPINGS: (u32, usize) = (0x228, 8);
/// A unit definition's seats block: (offset, element size), and the label
/// string ID's offset in an element. V: every `vehi` on 05a (the labels the
/// seat-mapping table's masks select).
pub const UNIT_SEATS: (usize, usize, usize) = (0x1c8, 0xb0, 4);

/// A unit definition and its seat labels, in seat order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// The `vehi` tag.
    pub id: TagId,
    /// Seat labels.
    pub labels: Vec<String>,
}

/// Every vehicle definition in `set` with its seat labels, read from pinned
/// or built bytes.
pub fn units(set: &TagSet) -> Vec<Unit> {
    let mut out = Vec::new();
    for slot in &set.slots {
        let Some(tag) = &slot.tag else { continue };
        if tag.group != map::FourCC(*b"vehi") {
            continue;
        }
        let labels = match &tag.payload {
            TagPayload::Pinned { address, bytes } => pinned_labels(set, *address, bytes),
            TagPayload::Built(b) => built_labels(set, b),
            _ => None,
        };
        if let Some(labels) = labels {
            out.push(Unit { id: tag.id, labels });
        }
    }
    out
}

fn string(set: &TagSet, sid: u32) -> String {
    set.strings
        .get((sid & 0xff_ffff) as usize)
        .cloned()
        .flatten()
        .unwrap_or_default()
}

fn word(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn pinned_labels(set: &TagSet, address: u32, bytes: &[u8]) -> Option<Vec<String>> {
    let (field, stride, label) = UNIT_SEATS;
    let count = word(bytes, field)? as usize;
    if count == 0 {
        return Some(Vec::new());
    }
    let start = word(bytes, field + 4)?.checked_sub(address)? as usize;
    let seats = bytes.get(start..start.checked_add(count.checked_mul(stride)?)?)?;
    Some(
        seats
            .chunks_exact(stride)
            .map(|e| string(set, word(e, label).unwrap_or(0)))
            .collect(),
    )
}

fn built_labels(set: &TagSet, b: &Block) -> Option<Vec<String>> {
    let (field, stride, label) = UNIT_SEATS;
    let Some(f) = b.fixups.iter().find(|f| f.offset as usize == field) else {
        return Some(Vec::new());
    };
    let FixupKind::Block { count, child } = &f.kind else {
        return None;
    };
    Some(
        (0..*count as usize)
            .map(|i| {
                let at = (i * stride + label) as u32;
                child
                    .fixups
                    .iter()
                    .find_map(|f| match &f.kind {
                        FixupKind::StringId(s) if f.offset == at => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| string(set, word(&child.bytes, at as usize).unwrap_or(0)))
            })
            .collect(),
    )
}

/// Whether Halo 2's seat label `label` answers Halo 1's seat text `token`.
/// Halo 1's roles map to Halo 2's label suffixes; boarding seats (`_b_`)
/// never match a role. Text no role covers matches as retail Halo 2 does:
/// labels that start with it.
pub fn h2_label_matches(token: &str, label: &str) -> bool {
    let t = token.to_ascii_lowercase();
    let l = label.to_ascii_lowercase();
    if l.is_empty() {
        return false;
    }
    let boarding = l.contains("_b_");
    let side = |prefix: &str| {
        t.rsplit(prefix)
            .next()
            .and_then(|rest| rest.chars().next())
            .filter(|c| matches!(c, 'l' | 'r'))
    };
    if t.contains("driver") {
        !boarding && l.ends_with("_d")
    } else if t.contains("gunner") {
        !boarding && (l.ends_with("_g") || l.contains("_g_"))
    } else if t.contains("rider") || t.contains("passenger") {
        if boarding || !(l.ends_with("_p") || l.contains("_p_")) {
            return false;
        }
        match side("rider").or_else(|| side("passenger")) {
            Some(s) => l.contains(&format!("_p_{s}")),
            None => true,
        }
    } else if t.contains("cargo") {
        l.contains("_lc")
    } else {
        l.starts_with(&t)
    }
}

/// The seat-mapping table and the value of each script text it covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    /// Elements: unit definition and seat mask.
    pub entries: Vec<(TagId, u32)>,
    /// `count << 16 | first` by script text.
    pub values: HashMap<String, u32>,
}

impl Table {
    /// One run of elements per distinct text in `tokens`: every unit with a
    /// matching seat (seats past the 32nd are not representable and are
    /// left out). Texts no seat answers get no value.
    pub fn build<'a>(tokens: impl IntoIterator<Item = &'a str>, units: &[Unit]) -> Self {
        let mut t = Self::default();
        for token in tokens {
            if t.values.contains_key(token) {
                continue;
            }
            let first = t.entries.len();
            for u in units {
                let mask = u
                    .labels
                    .iter()
                    .enumerate()
                    .take(32)
                    .filter(|(_, l)| h2_label_matches(token, l))
                    .fold(0u32, |m, (i, _)| m | 1 << i);
                if mask != 0 {
                    t.entries.push((u.id, mask));
                }
            }
            let count = t.entries.len() - first;
            if count > 0 && first <= 0xffff {
                t.values.insert(
                    token.to_owned(),
                    (count.min(0xffff) as u32) << 16 | first as u32,
                );
            }
        }
        t
    }

    /// The block's elements.
    pub fn elements(&self) -> Vec<Block> {
        self.entries
            .iter()
            .map(|&(id, mask)| {
                let mut b = Block::zeroed(MAPPINGS.1);
                b.tag_id(0, id);
                b.put_u32(4, mask);
                b
            })
            .collect()
    }
}
