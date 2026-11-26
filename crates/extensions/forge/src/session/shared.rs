//! A session in a synchronous network game (docs/game/forge-design.md, F5).
//!
//! The same tools drive it, but an edit is not applied where it is made: it
//! is queued as a proposal for the host ([`Outgoing`]), and every entry the
//! host's stream carries, this machine's own included, is applied at its
//! tick ([`Session::apply_item`]). New op IDs come from the seat's lane
//! ([`crate::net::next_in_lane`]), so two machines never pick the same one.
use forge_doc::{Edit, Entry, ForgeDoc, Limits, LogHead, Op, OpId, Proposal, Sha256};

use super::{Session, reason, scene, things};
use crate::World;
use crate::net::{Admit, Item, Seat, Stamped, State, next_in_lane};

/// What a shared session knows of its place in the game.
#[derive(Clone, Debug)]
pub(super) struct Shared {
    pub seat: Seat,
    /// Proposals made and not yet sent to the host.
    pub outbox: Vec<Outgoing>,
}

/// One edit for the host, as the session made it. The node adds the
/// author's name and a nonce when it sends the message.
#[derive(Clone, Debug, PartialEq)]
pub struct Outgoing {
    /// The log position the edit was made at.
    pub base_seq: u32,
    pub op: Op,
    pub was: Option<Sha256>,
}

impl Outgoing {
    pub fn proposal(&self, author: &str) -> Proposal {
        Proposal {
            author: author.into(),
            base_seq: self.base_seq,
            op: self.op.clone(),
            was: self.was,
        }
    }
}

/// The text a shared session gives for the actions it leaves to the host.
pub const SHARED_PROPOSE: &str =
    "In a shared Forge session an edit goes to the host: use the node's propose.";
pub const SHARED_REDO: &str =
    "A shared Forge session has no redo: propose the undone edit again.";

impl Session {
    /// Make this a seat of a shared session. Edits made through it queue
    /// as proposals under `name`, the author the host gives the seat.
    pub fn share(mut self, seat: Seat, name: &str) -> Self {
        self.author = name.into();
        self.shared = Some(Shared {
            seat,
            outbox: vec![],
        });
        self
    }

    /// The limits the map's document is held to.
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// The seat this session speaks for, in a shared session.
    pub fn seat(&self) -> Option<Seat> {
        self.shared.as_ref().map(|s| s.seat)
    }

    /// The author the host knows this seat by.
    pub fn author(&self) -> &str {
        &self.author
    }

    /// The proposals made since the last call, oldest first.
    pub fn take_outgoing(&mut self) -> Vec<Outgoing> {
        self.shared
            .as_mut()
            .map(|s| std::mem::take(&mut s.outbox))
            .unwrap_or_default()
    }

    /// The largest op ID this machine has seen or made.
    fn id_floor(&self) -> u32 {
        self.doc
            .ops
            .iter()
            .map(|op| op.id.0)
            .chain(self.log.iter().map(|entry| entry.op.id.0))
            .chain(self.redo.iter().flatten().map(|op| op.id.0))
            .max()
            .unwrap_or(0)
            .max(self.last_id)
    }

    /// The next `count` op IDs in this seat's lane, above every one seen.
    pub fn lane_ids(&self, count: usize) -> Option<Vec<OpId>> {
        let seat = self.shared.as_ref()?.seat;
        let mut above = self.id_floor();
        let mut ids = Vec::with_capacity(count);
        for _ in 0..count {
            let id = next_in_lane(seat, above)?;
            above = id.0;
            ids.push(id);
        }
        Some(ids)
    }

    /// New ops for `edits` in a shared session's lane.
    pub(super) fn lane_ops(&self, edits: Vec<Edit>) -> Result<Vec<Op>, String> {
        let ids = self
            .lane_ids(edits.len())
            .ok_or("Forge operation IDs exhausted.")?;
        Ok(ids
            .into_iter()
            .zip(edits)
            .map(|(id, edit)| Op { id, edit })
            .collect())
    }

    /// Check the ops against the document as this machine has it, as the
    /// host will, then queue them. Nothing changes in the document or the
    /// world until the stream brings the entries back.
    pub(super) fn queue(&mut self, ops: &[Op], was: Option<Sha256>) -> Result<(), String> {
        let max_entries = self.limits.max_log_entries.min(forge_doc::MAX_LOG_ENTRIES);
        if self.log.len() + ops.len() > max_entries {
            return Err("Forge edit log is full.".into());
        }
        let mut head = self.head;
        let mut entries = Vec::with_capacity(ops.len());
        for op in ops {
            let holds = if was.is_some() { self.holds(op)? } else { None };
            let proposal = Proposal {
                author: self.author.clone(),
                base_seq: head.seq,
                op: op.clone(),
                was,
            };
            let entry = head
                .stamp(self.head.tick, proposal, holds, &self.limits)
                .map_err(reason)?;
            head = LogHead {
                seq: entry.seq,
                tick: entry.tick,
                hash: entry.hash().map_err(reason)?,
            };
            entries.push(entry);
        }
        let (doc, _) =
            forge_doc::fold_from(&self.doc, self.head, &entries, &self.limits).map_err(reason)?;
        self.check_doc(&doc)?;
        self.check_drops(&doc)?;
        let at = self.head.seq;
        let shared = self.shared.as_mut().expect("a shared session");
        shared.outbox.extend(ops.iter().map(|op| Outgoing {
            base_seq: at,
            op: op.clone(),
            was,
        }));
        Ok(())
    }

    /// Undo in a shared session: the inverse of the last active edit,
    /// when it is this seat's own, proposed like any other.
    pub(super) fn undo_shared(&mut self) -> Result<(), String> {
        let last = self.doc.ops.last().ok_or("There is nothing to undo.")?;
        let mine = self
            .log
            .iter()
            .rev()
            .find(|entry| entry.op.id == last.id && entry.op.edit != Edit::Undo)
            .is_some_and(|entry| entry.author == self.author);
        if !mine {
            return Err("The last edit is not yours to undo.".into());
        }
        let op = Op {
            id: last.id,
            edit: Edit::Undo,
        };
        self.queue(&[op], None)
    }

    /// What a document may hold on this machine: the palette, the budget,
    /// the bounds, the entries that edit ops name and the teleporters'
    /// roles. The host asks it of every edit ([`SessionAdmit`]), and a
    /// shared session asks it before it proposes one.
    pub(crate) fn check_doc(&self, doc: &ForgeDoc) -> Result<(), String> {
        scene::project(doc, &self.limits, &self.palette, self.budget)?;
        things::desired(doc, &self.limits.base_counts, &self.base)?;
        self.channel_changes(doc)?;
        Ok(())
    }

    /// [`Self::holds`] against `doc` rather than the session's own.
    pub(crate) fn holds_in(&self, doc: &ForgeDoc, op: &Op) -> Result<Option<Sha256>, String> {
        let mut at = self.clone();
        at.own = things::desired(doc, &self.limits.base_counts, &self.base)?;
        at.doc = doc.clone();
        at.holds(op)
    }

    /// The host's checks over this session's map: its palette, budget,
    /// bounds and rows. It holds a copy, so the host can keep it while
    /// the session goes on.
    pub fn admit(&self) -> SessionAdmit {
        SessionAdmit(Box::new(self.clone()))
    }

    /// Take the state a joined peer holds as this session's own: the
    /// document and the log, and the world made to match. Used when this
    /// machine joins or hosts, before any item is applied.
    pub fn sync_to(&mut self, world: &mut impl World, state: &State) -> Result<(), String> {
        let doc = state.doc();
        if doc.base != self.doc.base
            || doc.builder != self.doc.builder
            || doc.recipes != self.doc.recipes
            || doc.imports != self.doc.imports
            || doc.sources != self.doc.sources
        {
            return Err("The session's document requires another base map, builder or kit.".into());
        }
        self.restore(world)?;
        self.update_world(world, doc, false)?;
        self.held = None;
        self.doc = doc.clone();
        self.head = state.head();
        self.log = state.log().to_vec();
        self.log_bytes = self
            .log
            .iter()
            .map(|entry| entry.canonical_json().map(|json| json.len()))
            .sum::<Result<usize, _>>()
            .map_err(reason)?;
        self.last_id = self.id_floor().max(state.max_id());
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }

    /// Apply one item of the host's stream at its tick: an entry changes
    /// the document and the world, a carry moves the held thing in the
    /// world, a release puts it back; the tables (members, holds) are the
    /// peer's, not the session's.
    pub fn apply_item(&mut self, world: &mut impl World, item: &Stamped) -> Result<(), String> {
        match &item.item {
            Item::Entry(entry) => self.apply_entry(world, entry),
            Item::Carry { target, frame, .. } => self.hold(world, item.tick, *target, frame.frame()),
            Item::Release { .. } => self.release(world),
            Item::Hold { .. } | Item::Member { .. } | Item::Left { .. } | Item::Through => Ok(()),
        }
    }

    fn apply_entry(&mut self, world: &mut impl World, entry: &Entry) -> Result<(), String> {
        let (doc, head) = forge_doc::fold_from(
            &self.doc,
            self.head,
            std::slice::from_ref(entry),
            &self.limits,
        )
        .map_err(reason)?;
        forge_doc::resolve(&doc.ops, &self.limits.base_counts).map_err(reason)?;
        let bytes = self.log_bytes
            + entry
                .canonical_json()
                .map_err(reason)?
                .len();
        self.restore(world)?;
        let zones = self.zones_before();
        // An undo may bring a thing back: the world makes it again.
        self.update_world(world, &doc, entry.op.edit == Edit::Undo)?;
        self.held = None;
        self.doc = doc;
        self.follow_drops(zones);
        self.head = head;
        self.log_bytes = bytes;
        self.last_id = self.last_id.max(entry.op.id.0);
        self.log.push(entry.clone());
        self.undo.clear();
        self.redo.clear();
        Ok(())
    }
}

/// The host's checks over a session's map ([`Session::admit`]).
#[derive(Clone, Debug)]
pub struct SessionAdmit(Box<Session>);

impl Admit for SessionAdmit {
    fn holds(&self, doc: &ForgeDoc, op: &Op) -> Result<Option<Sha256>, String> {
        self.0.holds_in(doc, op)
    }

    fn admit(&self, doc: &ForgeDoc) -> Result<(), String> {
        self.0.check_doc(doc)
    }
}
