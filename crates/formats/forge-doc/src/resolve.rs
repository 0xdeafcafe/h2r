//! Entry ops lowered to the placement ops an adapter applies
//! (docs/game/forge-map-variants.md, "The ops").
//!
//! An entry op names its target by identity. [`resolve`] walks the ops
//! keeping each identity's running index in its block through appends and
//! swap-removes, as `ForgeDoc::links` does for final indices, and gives each
//! entry op the `Placement::Set` or `Placement::Remove` of that running
//! index. A `Reset` lowers to nothing, and so does every entry op a later
//! reset of the same entry covers: the fold leaves them out, as it leaves
//! out a retracted op, so the entry has the base map's bytes again.
//! Placement ops by running index are never covered; their indices are read
//! in the document with the covered ops left out.
use std::collections::BTreeMap;

use crate::{
    BlockKind, Edit, Endpoint, EntryOp, Forge, Link, Op, OpId, Placement, Print, Refusal, Result,
    entry,
};

/// A base entry the document names with a print. The adapter checks it
/// against the entry as the base map has it (`check_print`) before it
/// writes anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Printed {
    pub block: BlockKind,
    /// The entry's index in the base map's block.
    pub index: u32,
    pub print: Print,
}

/// The document's entry ops as placement ops by running index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// Each entry op that has an effect, by op ID: its placement op at the
    /// target's running index when the op is applied. A reset, and an op a
    /// later reset covers, are absent.
    pub lowered: BTreeMap<OpId, Placement>,
    /// Every base entry an entry op, a link or a label names with a print,
    /// once, ordered by block then index.
    pub prints: Vec<Printed>,
    /// The identities of each block an entry op names, in index order
    /// after every op.
    rows: BTreeMap<BlockKind, Vec<Endpoint>>,
}

impl Resolved {
    /// The entry's index after every op, in a block an entry op names.
    /// `None` when the entry is removed or the block was not followed.
    pub fn index(&self, end: Endpoint) -> Option<u32> {
        let end = end.identity();
        let block = match end {
            Endpoint::Base { block, .. } => Some(block),
            Endpoint::Added(_) => None,
        };
        self.rows
            .iter()
            .filter(|(b, _)| block.is_none_or(|block| **b == block))
            .find_map(|(_, rows)| rows.iter().position(|e| *e == end))
            .map(|i| i as u32)
    }
}

/// Lower the entry ops of `ops` to placement ops by running index. The
/// map's base count is needed for each block an entry op names. `ops` are
/// a validated document's (`validate`); an op that names no entry here is
/// refused all the same.
pub fn resolve(ops: &[Op], base_counts: &BTreeMap<BlockKind, u32>) -> Result<Resolved> {
    let covered = entry::covered(ops);
    let added: BTreeMap<OpId, BlockKind> = ops
        .iter()
        .filter_map(|op| match &op.edit {
            Edit::Placement(p @ Placement::Add { .. }) => Some((op.id, p.block())),
            _ => None,
        })
        .collect();
    let block_of = |end: Endpoint, what: &str| match end {
        Endpoint::Base { block, .. } => Ok(block),
        Endpoint::Added(id) => added
            .get(&id)
            .copied()
            .ok_or_else(|| Refusal::invalid(what, "names no added entry")),
    };

    let mut resolved = Resolved::default();
    for op in ops {
        if let Edit::Entry(edit) = &op.edit {
            let block = block_of(edit.target(), edit.what())?;
            if let std::collections::btree_map::Entry::Vacant(slot) = resolved.rows.entry(block) {
                let count = *base_counts.get(&block).ok_or_else(|| {
                    Refusal::invalid(
                        edit.what(),
                        format!("the map's {} count is needed", block.name()),
                    )
                })?;
                slot.insert((0..count).map(|i| Endpoint::base(block, i)).collect());
            }
        }
    }

    let mut prints = BTreeMap::new();
    for op in ops {
        match &op.edit {
            Edit::Placement(placement) => {
                let Some(rows) = resolved.rows.get_mut(&placement.block()) else {
                    continue;
                };
                match *placement {
                    Placement::Add { .. } => rows.push(Endpoint::Added(op.id)),
                    Placement::Remove { index, block } => {
                        if index as usize >= rows.len() {
                            return Err(Refusal::invalid(
                                block.name(),
                                "placement index does not exist",
                            ));
                        }
                        rows.swap_remove(index as usize);
                    }
                    Placement::Set { .. } => {}
                }
            }
            Edit::Entry(edit) => {
                record(&mut prints, edit.target())?;
                if matches!(edit, EntryOp::Reset { .. }) || covered.contains(&op.id) {
                    continue;
                }
                let what = edit.what();
                let target = edit.target().identity();
                let block = block_of(target, what)?;
                let Some(rows) = resolved.rows.get_mut(&block) else {
                    // The first pass adds every entry op's block.
                    continue;
                };
                let index = rows.iter().position(|e| *e == target).ok_or_else(|| {
                    Refusal::invalid(what, format!("names no {} entry", block.name()))
                })?;
                let lowered = match edit {
                    EntryOp::Set { write, .. } => Placement::Set {
                        block,
                        index: index as u32,
                        write: write.clone(),
                    },
                    EntryOp::Remove { .. } => {
                        rows.swap_remove(index);
                        Placement::Remove {
                            block,
                            index: index as u32,
                        }
                    }
                    // Properties and areas have no scenario bytes.
                    EntryOp::Reset { .. } | EntryOp::Property { .. } | EntryOp::Area { .. } => {
                        continue;
                    }
                };
                resolved.lowered.insert(op.id, lowered);
            }
            Edit::Forge(Forge::Label { target, .. }) => record(&mut prints, *target)?,
            Edit::Forge(Forge::Piece { target, .. }) => record(&mut prints, *target)?,
            Edit::Forge(Forge::Group(group)) => {
                for end in &group.members {
                    record(&mut prints, *end)?;
                }
            }
            Edit::Forge(Forge::Link(link)) => {
                for end in Link::endpoints(link) {
                    record(&mut prints, end)?;
                }
            }
            _ => {}
        }
    }
    resolved.prints = prints
        .into_iter()
        .map(|((block, index), print)| Printed {
            block,
            index,
            print,
        })
        .collect();
    Ok(resolved)
}

/// Keep a base endpoint's print. One entry has one print in a document.
fn record(prints: &mut BTreeMap<(BlockKind, u32), Print>, end: Endpoint) -> Result<()> {
    if let Endpoint::Base {
        block,
        index,
        print: Some(print),
    } = end
        && *prints.entry((block, index)).or_insert(print) != print
    {
        return Err(Refusal::invalid(
            "print",
            "the entry is named with two different prints",
        ));
    }
    Ok(())
}
