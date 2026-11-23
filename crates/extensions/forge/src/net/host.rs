//! The authority: admits proposals, holds and members, and writes the
//! stream.
use std::collections::{BTreeMap, VecDeque};

use forge_doc::{Edit, Endpoint, ForgeDoc, Limits, Proposal, Refusal};

use super::{
    Admit, Beat, Carry, Held, Item, Member, NetRefusal, Reply, Role, SEATS, Seat, Stamped, State,
    Touch, Up, Welcome, by_index, lane,
};

/// The host of a synchronous Forge session. Every message it accepts
/// becomes one [`Stamped`] item for every machine, the host's own peer
/// included; every refusal is a [`Reply`] to its author alone and changes
/// nothing.
pub struct Host<A: Admit> {
    admit: A,
    limits: Limits,
    delay: u32,
    /// After every item written.
    latest: State,
    /// After every item due by `now`: what a joiner is given.
    applied: State,
    /// Items written and not yet due, in order.
    pending: VecDeque<Stamped>,
    now: u32,
    /// The tick of the last item other than `Through`.
    last_tick: u32,
    /// The watermark sent.
    through: u32,
    /// The apply tick of each seat's last carry.
    carried: BTreeMap<Seat, u32>,
}

/// What one call produced: the items for every machine, and the answer
/// to the caller's seat.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Out {
    pub items: Vec<Stamped>,
    pub reply: Option<Reply>,
}

impl<A: Admit> Host<A> {
    /// Open a session at its checkpoint, `now` being the host's tick.
    /// `delay` is at least one tick.
    pub fn new(
        start: ForgeDoc,
        limits: Limits,
        admit: A,
        now: u32,
        delay: u32,
    ) -> Result<Self, Refusal> {
        let latest = State::new(start, &limits)?;
        Ok(Self {
            admit,
            delay: delay.max(1),
            applied: latest.clone(),
            latest,
            limits,
            pending: VecDeque::new(),
            now,
            last_tick: now,
            through: now,
            carried: BTreeMap::new(),
        })
    }

    /// The session after every item written.
    pub fn state(&self) -> &State {
        &self.latest
    }

    /// The session as every machine has it at the host's tick.
    pub fn applied(&self) -> &State {
        &self.applied
    }

    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Whether a Rebuild may start: no one holds anything.
    pub fn may_rebuild(&self) -> bool {
        self.latest.holds().is_empty()
    }

    /// Advance the host's clock. The items due by `now` join the applied
    /// state, and the watermark moves to the tick before the earliest one
    /// a new item can have.
    pub fn tick(&mut self, now: u32) -> Result<Option<Stamped>, NetRefusal> {
        if now < self.now {
            return Err(NetRefusal::Clock { now, was: self.now });
        }
        self.now = now;
        self.drain();
        let through = now.saturating_add(self.delay - 1);
        if through <= self.through {
            return Ok(None);
        }
        self.through = through;
        Ok(Some(self.write(through, Item::Through)))
    }

    /// Seat a machine. Its welcome holds the session as of the host's
    /// tick and the items still to apply, the new member's item among
    /// them; the item also goes to every machine already in.
    pub fn join(
        &mut self,
        seat: Seat,
        name: &str,
        role: Role,
    ) -> Result<(Welcome, Stamped), NetRefusal> {
        if seat.0 >= SEATS || self.latest.members().contains_key(&seat) {
            return Err(NetRefusal::Seat);
        }
        if !name_ok(name)
            || self
                .latest
                .members()
                .values()
                .any(|m| m.name.eq_ignore_ascii_case(name))
        {
            return Err(NetRefusal::Name);
        }
        let item = self.stamp(Item::Member {
            seat,
            member: Member {
                name: name.into(),
                role,
            },
        });
        let welcome = Welcome {
            seat,
            tick: self.now,
            start: self.applied.start().clone(),
            start_head: self.applied.start_head(),
            entries: self.applied.log().to_vec(),
            head: self.applied.head(),
            members: self.applied.members().clone(),
            holds: self.applied.holds().clone(),
            n: self.applied.n(),
            through: self.through,
            pending: self.pending.iter().cloned().collect(),
        };
        Ok((welcome, item))
    }

    /// Change a seat's role. A role that cannot edit lets go of its hold.
    pub fn set_role(&mut self, seat: Seat, role: Role) -> Result<Stamped, NetRefusal> {
        let member = self.member(seat)?.clone();
        Ok(self.stamp(Item::Member {
            seat,
            member: Member { role, ..member },
        }))
    }

    /// A seat left: its hold ends.
    pub fn leave(&mut self, seat: Seat) -> Result<Stamped, NetRefusal> {
        self.member(seat)?;
        self.carried.remove(&seat);
        Ok(self.stamp(Item::Left { seat }))
    }

    /// Take one message from `seat`.
    pub fn receive(&mut self, seat: Seat, up: Up) -> Out {
        match up {
            Up::Propose { nonce, proposal } => answer(nonce, self.propose(seat, proposal)),
            Up::Hold { nonce, target } => answer(nonce, self.hold(seat, target)),
            Up::Carry { target, frame } => Out {
                items: self
                    .carry(seat, target, frame)
                    .ok()
                    .flatten()
                    .into_iter()
                    .collect(),
                reply: None,
            },
            Up::Release => Out {
                items: self.release(seat).ok().into_iter().collect(),
                reply: None,
            },
        }
    }

    /// Check a machine's keep-alive against the log. A head that differs
    /// ends that machine's session.
    pub fn check(&self, seat: Seat, beat: Beat) -> Result<(), NetRefusal> {
        match self.latest.head_at(beat.head.seq) {
            Ok(Some(head)) if head == beat.head => Ok(()),
            _ => Err(NetRefusal::HeadDiffers {
                seat,
                seq: beat.head.seq,
            }),
        }
    }

    fn member(&self, seat: Seat) -> Result<&Member, NetRefusal> {
        self.latest
            .members()
            .get(&seat)
            .ok_or(NetRefusal::NotJoined)
    }

    fn editor(&self, seat: Seat) -> Result<&Member, NetRefusal> {
        let member = self.member(seat)?;
        if member.role.edits() {
            Ok(member)
        } else {
            Err(NetRefusal::Role { role: member.role })
        }
    }

    fn held_by(&self, touch: Touch, seat: Seat) -> Result<(), NetRefusal> {
        match self.latest.holder(touch, seat) {
            Some(other) => Err(NetRefusal::Held {
                by: self
                    .latest
                    .members()
                    .get(&other)
                    .map_or_else(|| other.to_string(), |m| m.name.clone()),
            }),
            None => Ok(()),
        }
    }

    /// Admit one edit. A proposal made at an earlier head is rebased onto
    /// the current one when it names its entry by identity; one that names
    /// an entry by index is refused as stale, since an add or remove since
    /// may have moved the index. `was` is checked against what the host
    /// holds now.
    pub fn propose(&mut self, seat: Seat, proposal: Proposal) -> Result<Stamped, NetRefusal> {
        let author = self.editor(seat)?.name.clone();
        let head = self.latest.head();
        if proposal.base_seq > head.seq {
            return Err(NetRefusal::Ahead {
                proposed: proposal.base_seq,
                head: head.seq,
            });
        }
        if proposal.base_seq < head.seq && by_index(&proposal.op) {
            return Err(NetRefusal::Stale {
                proposed: proposal.base_seq,
                head: head.seq,
            });
        }
        if proposal.op.edit != Edit::Undo && !lane(seat, proposal.op.id, self.latest.start_max()) {
            return Err(NetRefusal::Lane { id: proposal.op.id });
        }
        self.held_by(self.latest.touches(&proposal.op), seat)?;
        let has_was = proposal.was.is_some()
            || matches!(&proposal.op.edit, Edit::TagField(write) if write.was.is_some());
        let holds = if has_was {
            self.admit
                .holds(self.latest.doc(), &proposal.op)
                .map_err(NetRefusal::Map)?
        } else {
            None
        };
        let proposal = Proposal {
            author,
            base_seq: head.seq,
            ..proposal
        };
        let tick = self.next_tick();
        let entry = head.stamp(tick, proposal, holds, &self.limits)?;
        let (doc, ..) = self.latest.fold_entry(&entry, &self.limits)?;
        self.admit.admit(&doc).map_err(NetRefusal::Map)?;
        Ok(self.stamp(Item::Entry(entry)))
    }

    /// Pick a thing up. A second hold of a thing is refused; a seat's new
    /// hold replaces its old one.
    pub fn hold(&mut self, seat: Seat, target: Endpoint) -> Result<Stamped, NetRefusal> {
        self.editor(seat)?;
        if !self.latest.exists(target, &self.limits) {
            return Err(NetRefusal::Doc(Refusal::Invalid {
                what: "hold".into(),
                reason: "names no entry".into(),
            }));
        }
        self.held_by(Touch::Thing(target), seat)?;
        self.carried.remove(&seat);
        Ok(self.stamp(Item::Hold {
            seat,
            target: target.identity(),
        }))
    }

    /// Move the held thing. A second carry for the same apply tick is
    /// dropped (`Ok(None)`).
    pub fn carry(
        &mut self,
        seat: Seat,
        target: Endpoint,
        frame: Carry,
    ) -> Result<Option<Stamped>, NetRefusal> {
        self.holding(seat, target)?;
        if let Some(world) = self.limits.world {
            let p = frame.position.map(forge_doc::F32::get);
            if (0..3).any(|i| p[i] < world.min[i] || p[i] > world.max[i]) {
                return Err(NetRefusal::Doc(Refusal::Invalid {
                    what: "position".into(),
                    reason: "outside the map's world bounds".into(),
                }));
            }
        }
        let tick = self.next_tick();
        if self.carried.get(&seat) == Some(&tick) {
            return Ok(None);
        }
        self.carried.insert(seat, tick);
        Ok(Some(self.stamp(Item::Carry {
            seat,
            target: target.identity(),
            frame,
        })))
    }

    pub fn release(&mut self, seat: Seat) -> Result<Stamped, NetRefusal> {
        self.member(seat)?;
        if !self.latest.holds().contains_key(&seat) {
            return Err(NetRefusal::NotHolding);
        }
        self.carried.remove(&seat);
        Ok(self.stamp(Item::Release { seat }))
    }

    fn holding(&self, seat: Seat, target: Endpoint) -> Result<&Held, NetRefusal> {
        self.member(seat)?;
        self.latest
            .holds()
            .get(&seat)
            .filter(|held| held.target.identity() == target.identity())
            .ok_or(NetRefusal::NotHolding)
    }

    fn next_tick(&self) -> u32 {
        self.now
            .saturating_add(self.delay)
            .max(self.last_tick)
            .max(self.through.saturating_add(1))
    }

    /// Write an item at the next apply tick.
    fn stamp(&mut self, item: Item) -> Stamped {
        let tick = match &item {
            Item::Entry(entry) => entry.tick,
            _ => self.next_tick(),
        };
        self.last_tick = tick;
        self.write(tick, item)
    }

    fn write(&mut self, tick: u32, item: Item) -> Stamped {
        let stamped = Stamped {
            n: self.latest.n() + 1,
            tick,
            item,
        };
        self.latest
            .apply(&stamped, &self.limits)
            .expect("the host writes only what it admitted");
        self.pending.push_back(stamped.clone());
        self.drain();
        stamped
    }

    /// Apply the items due by `now`, in stream order. A watermark behind a
    /// later item waits for it.
    fn drain(&mut self) {
        while self
            .pending
            .front()
            .is_some_and(|item| item.tick <= self.now)
        {
            let item = self.pending.pop_front().expect("front exists");
            self.applied
                .apply(&item, &self.limits)
                .expect("the host applies what it wrote");
        }
    }
}

fn answer(nonce: u32, result: Result<Stamped, NetRefusal>) -> Out {
    match result {
        Ok(item) => Out {
            reply: Some(Reply::Accepted { nonce, n: item.n }),
            items: vec![item],
        },
        Err(refusal) => Out {
            items: vec![],
            reply: Some(Reply::Refused { nonce, refusal }),
        },
    }
}

fn name_ok(name: &str) -> bool {
    name.len() <= super::MAX_NAME && !name.trim().is_empty() && !name.chars().any(char::is_control)
}
