use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    Edit, ForgeDoc, Limits, MAX_DOC_BYTES, Op, OpId, Refusal, Result, Sha256,
    validate::{CountBefore, Counts, author, op},
    wire,
};

pub const MAX_LOG_ENTRIES: usize = 65_536;
pub const MAX_LOG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_ENTRY_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub author: String,
    pub base_seq: u32,
    pub op: Op,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub was: Option<Sha256>,
}

impl Proposal {
    /// Decode a bounded proposal before consulting any live state.
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self> {
        wire::check_size(bytes.len(), "proposal bytes", MAX_ENTRY_BYTES)?;
        let proposal: Self = serde_json::from_slice(bytes).map_err(wire::json_error)?;
        proposal.validate(limits)?;
        Ok(proposal)
    }

    fn validate(&self, limits: &Limits) -> Result<()> {
        author(&self.author)?;
        op(&self.op, limits, true)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub seq: u32,
    pub tick: u32,
    pub author: String,
    pub op: Op,
    pub prev_hash: Sha256,
}

impl Entry {
    pub fn from_bytes(bytes: &[u8], limits: &Limits) -> Result<Self> {
        wire::check_size(bytes.len(), "entry bytes", MAX_ENTRY_BYTES)?;
        let entry: Self = serde_json::from_slice(bytes).map_err(wire::json_error)?;
        entry.validate(limits)?;
        Ok(entry)
    }

    pub fn canonical_json(&self) -> Result<String> {
        self.validate(&Limits::default())?;
        wire::canonical(self, "entry bytes", MAX_ENTRY_BYTES)
    }

    pub fn hash(&self) -> Result<Sha256> {
        Ok(Sha256::of(self.canonical_json()?.as_bytes()))
    }

    fn validate(&self, limits: &Limits) -> Result<()> {
        if self.seq == 0 {
            return Err(chain_error(0, "entries start at sequence 1"));
        }
        author(&self.author)?;
        op(&self.op, limits, true)
    }
}

/// The last accepted entry. At sequence zero, the hash is the starting
/// document's identity and the tick is zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogHead {
    pub seq: u32,
    pub tick: u32,
    pub hash: Sha256,
}

impl LogHead {
    pub fn genesis(doc: &ForgeDoc) -> Result<Self> {
        Ok(Self {
            seq: 0,
            tick: 0,
            hash: doc.id()?,
        })
    }

    /// Stamp a structurally valid proposal at the current head. The host
    /// supplies the authenticated author and a digest of the target it
    /// actually holds, and checks the edit against its live state before
    /// accepting or broadcasting the entry.
    pub fn stamp(
        self,
        tick: u32,
        proposal: Proposal,
        holds: Option<Sha256>,
        limits: &Limits,
    ) -> Result<Entry> {
        if proposal.base_seq != self.seq {
            return Err(Refusal::Stale {
                proposed: proposal.base_seq,
                head: self.seq,
            });
        }
        proposal.validate(limits)?;
        let write_was = match &proposal.op.edit {
            Edit::TagField(write) => write.was,
            _ => None,
        };
        if [proposal.was, write_was]
            .into_iter()
            .flatten()
            .any(|was| Some(was) != holds)
        {
            return Err(Refusal::Mismatch);
        }
        let seq = self.next_seq()?;
        if tick < self.tick {
            return Err(chain_error(seq, "tick goes backwards"));
        }
        let entry = Entry {
            seq,
            tick,
            author: proposal.author,
            op: proposal.op,
            prev_hash: self.hash,
        };
        wire::measure(&entry, "entry bytes", MAX_ENTRY_BYTES)?;
        Ok(entry)
    }

    fn next_seq(self) -> Result<u32> {
        self.seq
            .checked_add(1)
            .ok_or_else(|| chain_error(self.seq, "sequence exhausted"))
    }
}

/// Check a batch of entries against its predecessor and the host's expected
/// head. Without an expected head, the batch determines the returned head.
pub fn validate_chain(
    start: LogHead,
    entries: &[Entry],
    expected: Option<LogHead>,
    limits: &Limits,
) -> Result<LogHead> {
    let mut chain = Chain::new(start, entries.len(), limits)?;
    for entry in entries {
        chain.push(entry, limits)?;
    }
    if expected.is_some_and(|head| head != chain.head) {
        return Err(chain_error(chain.head.seq, "head differs from the host's"));
    }
    Ok(chain.head)
}

struct Chain {
    head: LogHead,
    bytes: usize,
}

impl Chain {
    fn new(head: LogHead, len: usize, limits: &Limits) -> Result<Self> {
        wire::check_size(len, "log entries", limits.entries())?;
        if head.seq == 0 && head.tick != 0 {
            return Err(chain_error(0, "the initial tick must be zero"));
        }
        Ok(Self { head, bytes: 0 })
    }

    fn push(&mut self, entry: &Entry, limits: &Limits) -> Result<()> {
        if entry.seq != self.head.next_seq()? {
            return Err(chain_error(entry.seq, "sequence is not consecutive"));
        }
        if entry.prev_hash != self.head.hash {
            return Err(chain_error(entry.seq, "predecessor hash differs"));
        }
        if entry.tick < self.head.tick {
            return Err(chain_error(entry.seq, "tick goes backwards"));
        }
        entry.validate(limits)?;
        let json = wire::canonical(entry, "entry bytes", MAX_ENTRY_BYTES)?;
        wire::check_size(self.bytes + json.len(), "log bytes", MAX_LOG_BYTES)?;
        self.bytes += json.len();
        self.head = LogHead {
            seq: entry.seq,
            tick: entry.tick,
            hash: Sha256::of(json.as_bytes()),
        };
        Ok(())
    }
}

fn chain_error(seq: u32, reason: &str) -> Refusal {
    Refusal::Chain {
        seq,
        reason: reason.into(),
    }
}

/// Fold a log starting at the saved document.
pub fn fold(doc: &ForgeDoc, entries: &[Entry]) -> Result<ForgeDoc> {
    fold_document(doc, None, entries, &Limits::default()).map(|(doc, _)| doc)
}

/// Fold entries after a trusted checkpoint. A sequence-zero checkpoint is
/// checked against the document's identity. A later checkpoint is supplied
/// by the host along with the folded document at that position.
pub fn fold_from(
    doc: &ForgeDoc,
    start: LogHead,
    entries: &[Entry],
    limits: &Limits,
) -> Result<(ForgeDoc, LogHead)> {
    fold_document(doc, Some(start), entries, limits)
}

fn fold_document(
    doc: &ForgeDoc,
    checkpoint: Option<LogHead>,
    entries: &[Entry],
    limits: &Limits,
) -> Result<(ForgeDoc, LogHead)> {
    crate::validate::document(doc, limits)?;
    let start = match checkpoint {
        Some(head) if head.seq > 0 => {
            wire::measure(doc, "document bytes", MAX_DOC_BYTES)?;
            head
        }
        checkpoint => {
            let json = wire::canonical(doc, "document bytes", MAX_DOC_BYTES)?;
            let head = LogHead {
                seq: 0,
                tick: 0,
                hash: Sha256::of(json.as_bytes()),
            };
            if checkpoint.is_some_and(|checkpoint| checkpoint != head) {
                return Err(chain_error(0, "starting document or tick differs"));
            }
            head
        }
    };
    let mut chain = Chain::new(start, entries.len(), limits)?;

    let mut counts = Counts::new();
    let mut active = Vec::with_capacity(doc.ops.len());
    let mut ids = BTreeMap::new();
    for op in &doc.ops {
        ids.insert(
            op.id,
            Seen {
                op,
                author: None,
                active: true,
            },
        );
        active.push((op, counts.push(op, limits)?));
    }

    for entry in entries {
        chain.push(entry, limits)?;
        let op = &entry.op;
        if op.edit == Edit::Undo {
            let Some((last, before)) = active.pop().filter(|(last, _)| last.id == op.id) else {
                return Err(Refusal::UndoOrder(op.id));
            };
            if let Some(seen) = ids.get_mut(&last.id) {
                if seen.author.is_some_and(|author| author != entry.author) {
                    return Err(Refusal::UndoAuthor(op.id));
                }
                seen.active = false;
            }
            counts.pop(before);
        } else {
            if let Some(seen) = ids.get(&op.id)
                && (seen.active || seen.op != op || seen.author.is_some_and(|a| a != entry.author))
            {
                return Err(Refusal::DuplicateOp(op.id));
            }
            wire::check_size(active.len() + 1, "operations", limits.ops())?;
            active.push((op, counts.push(op, limits)?));
            ids.insert(
                op.id,
                Seen {
                    op,
                    author: Some(&entry.author),
                    active: true,
                },
            );
        }
    }

    let mut result = doc.clone();
    if !active.iter().map(|(op, _)| *op).eq(doc.ops.iter()) {
        result.ops = active.into_iter().map(|(op, _)| op.clone()).collect();
        result.expect = None;
    }
    wire::measure(&result, "document bytes", MAX_DOC_BYTES)?;
    Ok((result, chain.head))
}

struct Seen<'a> {
    op: &'a Op,
    author: Option<&'a str>,
    active: bool,
}

/// The previous tail, needed when inverting an undo for redo. It carries
/// only the portable op. A placement's unknown fields remain on the map.
pub struct Before<'a> {
    last: Option<&'a Op>,
}

impl<'a> Before<'a> {
    pub fn capture(doc: &'a ForgeDoc) -> Self {
        Self {
            last: doc.ops.last(),
        }
    }
}

/// Invert a portable op. A retraction only takes effect while the op is the
/// latest active edit, so later index-based commands cannot be reordered.
pub fn inverse(op: &Op, before: &Before<'_>) -> Result<Op> {
    match &op.edit {
        Edit::Placement(_) | Edit::TagField(_) => Ok(Op {
            id: op.id,
            edit: Edit::Undo,
        }),
        Edit::Undo => before
            .last
            .filter(|last| last.id == op.id)
            .cloned()
            .ok_or(Refusal::UndoOrder(op.id)),
        edit => Err(edit.unsupported()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Placement;

    #[test]
    fn checks_the_log_byte_budget_before_advancing_the_head() {
        let start = LogHead {
            seq: 0,
            tick: 0,
            hash: Sha256([1; 32]),
        };
        let entry = Entry {
            seq: 1,
            tick: 1,
            author: "Editor".into(),
            op: Op {
                id: OpId(1),
                edit: Edit::Placement(Placement::Remove {
                    block: crate::BlockKind::Weapons,
                    index: 0,
                }),
            },
            prev_hash: start.hash,
        };
        let mut chain = Chain {
            head: start,
            bytes: MAX_LOG_BYTES - 1,
        };
        assert_eq!(
            chain.push(&entry, &Limits::default()),
            Err(Refusal::limit("log bytes", MAX_LOG_BYTES))
        );
        assert_eq!(chain.head, start);
    }
}
