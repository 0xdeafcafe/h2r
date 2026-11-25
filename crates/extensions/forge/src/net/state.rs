//! The session as one machine holds it after a prefix of the stream.
use std::collections::BTreeMap;

use forge_doc::{
    BlockKind, Edit, Endpoint, Entry, ForgeDoc, Limits, LogHead, MAX_LOG_BYTES, MAX_LOG_ENTRIES,
    Op, OpId, Placement, Refusal,
};

use super::{Held, Item, Member, Seat, Stamped, Touch};

/// The document, its log and the session's tables after stream item `n`.
/// The host keeps two (after every item it wrote, and after the items due
/// by its clock); each peer keeps one.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    start: ForgeDoc,
    start_head: LogHead,
    start_max: u32,
    doc: ForgeDoc,
    head: LogHead,
    log: Vec<Entry>,
    log_bytes: usize,
    /// Every op the log has named, as `forge_doc::fold` keeps them over a
    /// whole log: folding one entry at a time forgets an undone op and the
    /// author of an op already folded, so the session keeps both here and
    /// admits exactly what one fold of the whole log admits.
    seen: BTreeMap<OpId, Seen>,
    members: BTreeMap<Seat, Member>,
    holds: BTreeMap<Seat, Held>,
    n: u64,
}

#[derive(Clone, Debug, PartialEq)]
struct Seen {
    op: Op,
    /// `None` for the starting document's ops.
    author: Option<String>,
    active: bool,
}

impl State {
    /// The session at its checkpoint: the document the map was
    /// materialised from, with no entries yet.
    pub fn new(start: ForgeDoc, limits: &Limits) -> Result<Self, Refusal> {
        forge_doc::validate(&start, limits)?;
        let start_head = LogHead::genesis(&start)?;
        let seen = start
            .ops
            .iter()
            .map(|op| {
                (
                    op.id,
                    Seen {
                        op: op.clone(),
                        author: None,
                        active: true,
                    },
                )
            })
            .collect();
        Ok(Self {
            start_max: start.ops.iter().map(|op| op.id.0).max().unwrap_or(0),
            doc: start.clone(),
            start,
            start_head,
            head: start_head,
            log: vec![],
            log_bytes: 0,
            seen,
            members: BTreeMap::new(),
            holds: BTreeMap::new(),
            n: 0,
        })
    }

    pub fn start(&self) -> &ForgeDoc {
        &self.start
    }

    pub fn start_head(&self) -> LogHead {
        self.start_head
    }

    /// The largest op ID of the starting document.
    pub fn start_max(&self) -> u32 {
        self.start_max
    }

    /// The document as of the log head.
    pub fn doc(&self) -> &ForgeDoc {
        &self.doc
    }

    pub fn head(&self) -> LogHead {
        self.head
    }

    /// The entries after the checkpoint.
    pub fn log(&self) -> &[Entry] {
        &self.log
    }

    pub fn members(&self) -> &BTreeMap<Seat, Member> {
        &self.members
    }

    pub fn holds(&self) -> &BTreeMap<Seat, Held> {
        &self.holds
    }

    /// The last stream item applied.
    pub fn n(&self) -> u64 {
        self.n
    }

    /// The largest op ID the log or the starting document has named.
    pub fn max_id(&self) -> u32 {
        self.seen.keys().next_back().map_or(0, |id| id.0)
    }

    /// The head after entry `seq`, if this state has reached it.
    pub fn head_at(&self, seq: u32) -> Result<Option<LogHead>, Refusal> {
        if seq == self.start_head.seq {
            return Ok(Some(self.start_head));
        }
        let Some(entry) = seq
            .checked_sub(self.start_head.seq + 1)
            .and_then(|i| self.log.get(i as usize))
        else {
            return Ok(None);
        };
        Ok(Some(LogHead {
            seq: entry.seq,
            tick: entry.tick,
            hash: entry.hash()?,
        }))
    }

    /// Who holds a thing an op reaches, other than `except`.
    pub(crate) fn holder(&self, touch: Touch, except: Seat) -> Option<Seat> {
        self.holds
            .iter()
            .filter(|(seat, _)| **seat != except)
            .find(|(_, held)| match touch {
                Touch::Nothing => false,
                Touch::Thing(end) => held.target.identity() == end.identity(),
                Touch::Block(block) => self.block_of(held.target) == Some(block),
            })
            .map(|(seat, _)| *seat)
    }

    /// The thing or block an op reaches. An undo reaches what the op it
    /// retracts reached; an add's own entry is the thing it made.
    pub(crate) fn touches(&self, op: &Op) -> Touch {
        match &op.edit {
            Edit::Undo => match self.seen.get(&op.id) {
                Some(seen) if matches!(seen.op.edit, Edit::Placement(Placement::Add { .. })) => {
                    Touch::Thing(Endpoint::Added(op.id))
                }
                Some(seen) if seen.op.edit != Edit::Undo => self.touches(&seen.op),
                _ => Touch::Nothing,
            },
            Edit::Placement(Placement::Add { .. }) => Touch::Nothing,
            Edit::Placement(placement) => Touch::Block(placement.block()),
            Edit::Entry(entry) => Touch::Thing(entry.target().identity()),
            _ => Touch::Nothing,
        }
    }

    fn block_of(&self, end: Endpoint) -> Option<BlockKind> {
        match end {
            Endpoint::Base { block, .. } => Some(block),
            Endpoint::Added(id) => match &self.seen.get(&id)?.op.edit {
                Edit::Placement(Placement::Add { block, .. }) => Some(*block),
                _ => None,
            },
        }
    }

    /// A thing that can be held: the base map's entry, or an active add.
    pub(crate) fn exists(&self, target: Endpoint, limits: &Limits) -> bool {
        match target.identity() {
            Endpoint::Base { block, index, .. } => limits
                .base_counts
                .get(&block)
                .is_some_and(|count| index < *count),
            Endpoint::Added(id) => self.seen.get(&id).is_some_and(|seen| {
                seen.active && matches!(seen.op.edit, Edit::Placement(Placement::Add { .. }))
            }),
        }
    }

    /// Fold one entry at the head without changing anything: the
    /// document's checks, plus the ones a whole-log fold makes that an
    /// incremental fold forgets.
    pub(crate) fn fold_entry(
        &self,
        entry: &Entry,
        limits: &Limits,
    ) -> Result<(ForgeDoc, LogHead, usize), Refusal> {
        let max = limits.max_log_entries.min(MAX_LOG_ENTRIES);
        if self.log.len() >= max {
            return Err(Refusal::Limit {
                what: "log entries".into(),
                max,
            });
        }
        let bytes = self.log_bytes + entry.canonical_json()?.len();
        if bytes > MAX_LOG_BYTES {
            return Err(Refusal::Limit {
                what: "log bytes".into(),
                max: MAX_LOG_BYTES,
            });
        }
        let op = &entry.op;
        if let Some(seen) = self.seen.get(&op.id) {
            let other = seen
                .author
                .as_deref()
                .is_some_and(|author| author != entry.author);
            if op.edit == Edit::Undo {
                if other {
                    return Err(Refusal::UndoAuthor(op.id));
                }
            } else if seen.active || seen.op != *op || other {
                return Err(Refusal::DuplicateOp(op.id));
            }
        }
        let (doc, head) =
            forge_doc::fold_from(&self.doc, self.head, std::slice::from_ref(entry), limits)?;
        // Entry ops must name an entry that is there.
        forge_doc::resolve(&doc.ops, &limits.base_counts)?;
        Ok((doc, head, bytes))
    }

    fn commit_entry(&mut self, entry: Entry, doc: ForgeDoc, head: LogHead, bytes: usize) {
        if entry.op.edit == Edit::Undo {
            if let Some(seen) = self.seen.get_mut(&entry.op.id) {
                seen.active = false;
            }
            // An undo that retracts what a holder held ends the hold.
            let gone = Endpoint::Added(entry.op.id);
            self.holds.retain(|_, held| held.target.identity() != gone);
        } else {
            self.seen.insert(
                entry.op.id,
                Seen {
                    op: entry.op.clone(),
                    author: Some(entry.author.clone()),
                    active: true,
                },
            );
            if let Edit::Entry(forge_doc::EntryOp::Remove { target }) = &entry.op.edit {
                let gone = target.identity();
                self.holds.retain(|_, held| held.target.identity() != gone);
            }
        }
        self.doc = doc;
        self.head = head;
        self.log_bytes = bytes;
        self.log.push(entry);
    }

    /// Apply the next stream item. A refusal is a stream the host should
    /// not have written; it leaves the state as it was.
    pub fn apply(&mut self, stamped: &Stamped, limits: &Limits) -> Result<(), String> {
        if Some(stamped.n) != self.n.checked_add(1) {
            return Err(format!(
                "stream item {} follows {}; want {}",
                stamped.n,
                self.n,
                self.n + 1
            ));
        }
        match &stamped.item {
            Item::Entry(entry) => {
                if entry.tick != stamped.tick {
                    return Err(format!(
                        "entry {} is stamped for tick {} but names {}",
                        entry.seq, stamped.tick, entry.tick
                    ));
                }
                let (doc, head, bytes) = self
                    .fold_entry(entry, limits)
                    .map_err(|e| format!("entry {}: {e}", entry.seq))?;
                self.commit_entry(entry.clone(), doc, head, bytes);
            }
            Item::Hold { seat, target } => {
                if !self.members.get(seat).is_some_and(|m| m.role.edits()) {
                    return Err(format!("{seat} cannot hold"));
                }
                if let Some(other) = self.holder(Touch::Thing(*target), *seat) {
                    return Err(format!("{seat} holds what {other} holds"));
                }
                self.holds.insert(
                    *seat,
                    Held {
                        target: *target,
                        frame: None,
                    },
                );
            }
            Item::Carry {
                seat,
                target,
                frame,
            } => {
                let held = self
                    .holds
                    .get_mut(seat)
                    .filter(|held| held.target.identity() == target.identity())
                    .ok_or_else(|| format!("{seat} carries what it does not hold"))?;
                held.frame = Some(*frame);
            }
            Item::Release { seat } => {
                self.holds
                    .remove(seat)
                    .ok_or_else(|| format!("{seat} releases nothing"))?;
            }
            Item::Member { seat, member } => {
                if !member.role.edits() {
                    self.holds.remove(seat);
                }
                self.members.insert(*seat, member.clone());
            }
            Item::Left { seat } => {
                self.members.remove(seat);
                self.holds.remove(seat);
            }
            Item::Through => {}
        }
        self.n = stamped.n;
        Ok(())
    }

    /// A late joiner's state: the checkpoint folded with the entries after
    /// it, checked against the host's head, and the host's tables.
    pub(crate) fn from_welcome(welcome: &super::Welcome, limits: &Limits) -> Result<Self, String> {
        let mut state = Self::new(welcome.start.clone(), limits).map_err(|e| e.to_string())?;
        if state.start_head != welcome.start_head {
            return Err("the starting document is not the host's checkpoint".into());
        }
        for entry in &welcome.entries {
            let (doc, head, bytes) = state
                .fold_entry(entry, limits)
                .map_err(|e| format!("entry {}: {e}", entry.seq))?;
            state.commit_entry(entry.clone(), doc, head, bytes);
        }
        if state.head != welcome.head {
            return Err(format!(
                "the entries end at {} with another head than the host's",
                state.head.seq
            ));
        }
        // One fold of the whole log gives the same document.
        let (whole, head) =
            forge_doc::fold_from(&welcome.start, welcome.start_head, &welcome.entries, limits)
                .map_err(|e| e.to_string())?;
        if whole != state.doc || head != state.head {
            return Err("the log folds to another document".into());
        }
        state.members = welcome.members.clone();
        state.holds = welcome.holds.clone();
        state.n = welcome.n;
        Ok(state)
    }
}
