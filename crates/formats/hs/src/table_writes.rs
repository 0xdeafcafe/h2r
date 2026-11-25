//! The writes that turn a map's script tables into another set of tables
//! (docs/editor/rich-editors.md, "Scripts"; slice H3).
//!
//! The scenario tag's header holds five `(count, address)` pairs: the
//! nodes, the string data (one-byte elements), the scripts, the globals and
//! the tag references. [`writes`] compares the tables as the map holds them
//! with the tables as they are now and says, for each one that differs,
//! how a save writes it:
//!
//! - one that has no more elements than the map's is written in place, one
//!   patch per run of changed bytes, and its count patched when it
//!   changed. An unedited node is never written;
//! - one with more elements is written whole to a new place
//!   ([`Grown`]): the save appends it to the metadata and points the pair
//!   at the copy. The old bytes stay as dead data.
//!
//! Nothing is returned unless the new tables write out and read back as
//! the same bytes and graph ([`Scenario::verify_written`]).

use crate::scenario::HS_HEADER_SIZE;
use crate::wire::word;
use crate::{Result, Scenario, invalid};

/// A table that outgrew the map's: every element, to be appended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grown {
    /// Guest address of the table's `(count, address)` pair.
    pub root: u32,
    pub count: u32,
    /// Every element, in order.
    pub bytes: Vec<u8>,
}

/// What a save writes for a changed set of tables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Writes {
    /// `(guest address, bytes)`, in address order.
    pub patches: Vec<(u32, Vec<u8>)>,
    pub grown: Vec<Grown>,
}

impl Writes {
    pub fn is_empty(&self) -> bool {
        self.patches.is_empty() && self.grown.is_empty()
    }
}

/// The writes that make the map's `old` tables the `new` ones. `header` is
/// the scenario payload's first `0x240` bytes and `base` its guest address
/// ([`Scenario::read_header`]).
pub fn writes(header: &[u8], base: u32, old: &Scenario, new: &Scenario) -> Result<Writes> {
    if header.len() < HS_HEADER_SIZE as usize {
        return Err(invalid("scenario header is truncated"));
    }
    new.verify_written()?;
    let mut out = Writes::default();
    for ((field, stride, before), (_, _, after)) in
        old.table_bytes().into_iter().zip(new.table_bytes())
    {
        if before == after {
            continue;
        }
        let count = word(header, field) as usize;
        let address = word(header, field + 4);
        if before.len() != count * stride {
            return Err(invalid(format!(
                "table +{field:#x} is not the one the header describes"
            )));
        }
        let root = base + field as u32;
        let new_count = after.len() / stride;
        if new_count <= count {
            let mut run: Option<usize> = None;
            for i in 0..=after.len() {
                let differs = i < after.len() && before[i] != after[i];
                match (differs, run) {
                    (true, None) => run = Some(i),
                    (false, Some(start)) => {
                        out.patches.push((address + start as u32, after[start..i].to_vec()));
                        run = None;
                    }
                    _ => {}
                }
            }
            if new_count != count {
                out.patches
                    .push((root, (new_count as u32).to_le_bytes().to_vec()));
            }
        } else {
            out.grown.push(Grown {
                root,
                count: new_count as u32,
                bytes: after,
            });
        }
    }
    out.patches.sort();
    Ok(out)
}

#[cfg(test)]
#[path = "table_writes_tests.rs"]
mod tests;
