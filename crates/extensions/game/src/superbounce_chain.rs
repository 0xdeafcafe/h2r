//! Superbounce medals (docs/game/superbounce.md, "Medals"): the chain of
//! bounces each player makes with no ground between them, the medals each
//! bounce earns, and Bounced, a fall out of the map during a bounce.
//!
//! [`Chain`] is one player's, stepped once a tick with that player's
//! reads ([`PlayerTick`]); [`Medals`] keeps one per player slot, the
//! counts and the last few awards. Everything here but [`read_players`] is
//! pure, so the tests are lists of ticks. `superbounce_mode` steps it for
//! every player while the mode is on and publishes it on its state; the
//! HUD shows a local player's awards (`superbounce_hud`).
//!
//! It is not in a snapshot: after a state load or a Theater seek a chain
//! in flight starts again at 1.

use std::collections::{BTreeMap, VecDeque};

use engine::GuestMemory;
use hud_kit_core::huds::superbounce::{MEDALS, Medal};
use superbounce::{Spot, Vec3};

use crate::superbounce_charge::{LandingResult, TOUCHDOWN_VZ, strip_landing, touchdown};
use crate::superbounce_mode::primed;

/// Player slots the game has.
pub const SLOTS: usize = 16;
/// From this tick after a bounce (the bounce's own is 0), a tick with the
/// airborne bit clear ends the chain: a soft landing at the top of the
/// flight is no touchdown. The two ticks before it are the launch's, when
/// the bit may still be clear.
pub const CLEAR_FROM: u32 = 3;
/// An origin that moves more than this (world units) in one tick was
/// moved (a teleporter, `sim.state.set`), and the chain ends.
pub const TELEPORT: f32 = 3.0;
/// Awards kept for `superbounce.state`'s `recent`.
pub const RECENT: usize = 8;

/// The medals a bounce earns at chain count `chain`, lowest first: every
/// bounce is a Superbounce, and the second to the sixth of a chain earn
/// its tier as well, once each (the count passes each once). Seven and
/// over earn Superbounce alone. The canonical table, which the film and
/// Live use, is `live_proto::film::events::medals`'s; this copy is the
/// game's, which does not depend on Live, and the two are kept the same
/// by id (`medals_by_id`).
pub fn bounce_medals(chain: u32) -> &'static [Medal] {
    use Medal::*;
    match chain {
        0 => &[],
        2 => &[Superbounce, DoubleBounce],
        3 => &[Superbounce, TripleBounce],
        4 => &[Superbounce, QuadBounce],
        5 => &[Superbounce, ChainBounce],
        6 => &[Superbounce, Bouncetacular],
        _ => &[Superbounce],
    }
}

/// The highest medal of `medals`: the one a bounce's line shows.
pub fn highest(medals: &[Medal]) -> Option<Medal> {
    medals.iter().copied().max()
}

/// What one tick reads of a player's unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerTick {
    /// The unit's datum: a new one is a new life or a vehicle.
    pub unit: u32,
    pub position: Vec3,
    /// World units a second, up positive.
    pub velocity: Vec3,
    /// Biped flags bit 0.
    pub airborne: bool,
    /// The level pair's word; `None` without a pair.
    pub word: Option<u32>,
}

/// What a step gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A bounce, the chain's count with it.
    Bounce { chain: u32 },
    /// The unit went (a death) on the tick after one that was airborne,
    /// falling at [`TOUCHDOWN_VZ`] or faster, and either in a chain or
    /// primed: a fall out of the map during a bounce. `chain` is the count
    /// of the chain it was in, 0 if none.
    Bounced { chain: u32 },
}

impl Event {
    /// The medals the event earns.
    pub fn medals(self) -> Vec<Medal> {
        match self {
            Event::Bounce { chain } => bounce_medals(chain).to_vec(),
            Event::Bounced { .. } => vec![Medal::Bounced],
        }
    }
    /// The chain's count with the event.
    pub fn chain(self) -> u32 {
        match self {
            Event::Bounce { chain } | Event::Bounced { chain } => chain,
        }
    }
}

/// The previous tick, for the touchdown and the unit's change.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Prev {
    unit: u32,
    position: Vec3,
    vz: f32,
    airborne: bool,
    word: Option<u32>,
}

/// One player's chain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chain {
    count: u32,
    live: bool,
    /// Ticks since the last bounce (0 on its tick).
    since: u32,
    prev: Option<Prev>,
}

impl Chain {
    /// The count of the chain, the last one if it has ended.
    pub fn count(&self) -> u32 {
        self.count
    }

    /// Whether the next bounce adds to the count.
    pub fn live(&self) -> bool {
        self.live
    }

    /// Ends the chain (the count stays, for the record).
    pub fn end(&mut self) {
        self.live = false;
    }

    /// Files a bounce at count `chain` as a real one would: the chain is
    /// live at that count (`superbounce.medal_test`).
    pub fn force(&mut self, chain: u32) {
        self.count = chain;
        self.live = chain > 0;
        self.since = 0;
    }

    /// Whether the tick before was a fall during a bounce: airborne,
    /// falling, and in a chain or primed.
    fn falling(&self, p: &Prev) -> bool {
        p.airborne && p.vz <= TOUCHDOWN_VZ && (self.live || p.word.is_some_and(primed))
    }

    /// One tick: `t` is the player's unit, `None` when it has none (dead,
    /// or not in the game).
    pub fn step(&mut self, t: Option<&PlayerTick>, spots: &[Spot]) -> Option<Event> {
        let Some(t) = t else {
            let fell = self.prev.is_some_and(|p| self.falling(&p));
            let chain = if self.live { self.count } else { 0 };
            self.live = false;
            self.prev = None;
            return fell.then_some(Event::Bounced { chain });
        };
        let now = Prev {
            unit: t.unit,
            position: t.position,
            vz: t.velocity[2],
            airborne: t.airborne,
            word: t.word,
        };
        let Some(p) = self.prev.replace(now) else {
            return None;
        };
        if p.unit != t.unit {
            // A new unit on the same tick the old one went: a death and a
            // respawn, or a vehicle.
            let fell = self.falling(&p);
            let chain = if self.live { self.count } else { 0 };
            self.live = false;
            return fell.then_some(Event::Bounced { chain });
        }
        if distance(p.position, t.position) > TELEPORT || !t.velocity[2].is_finite() {
            self.live = false;
            return None;
        }
        if touchdown(p.airborne, p.vz, t.airborne, now.vz) {
            let bounced = p.word.is_some_and(primed)
                && strip_landing(spots, p.word, p.position, p.vz, now.vz)
                    == Some(LandingResult::Bounce);
            if bounced {
                self.count = if self.live { self.count + 1 } else { 1 };
                self.live = true;
                self.since = 0;
                return Some(Event::Bounce { chain: self.count });
            }
            self.live = false;
            return None;
        }
        if self.live {
            self.since = self.since.saturating_add(1);
            if self.since >= CLEAR_FROM && !t.airborne {
                self.live = false;
            }
        }
        None
    }
}

fn distance(a: Vec3, b: Vec3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// One player as read this tick: the slot, the local user (`None` for a
/// remote player or a bot) and the unit (`None` without one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub slot: u8,
    pub user: Option<u8>,
    pub tick: Option<PlayerTick>,
}

/// A medal award: one event of one player.
#[derive(Clone, Debug, PartialEq)]
pub struct Award {
    /// Counts up from 1 over the game: the HUD shows each award once.
    pub seq: u64,
    pub slot: u8,
    /// The player's local user when the award was made.
    pub user: Option<u8>,
    pub tick: u32,
    pub chain: u32,
    pub medals: Vec<Medal>,
}

/// A player's medals this game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Record {
    /// The count of its last chain.
    pub chain: u32,
    pub live: bool,
    pub counts: BTreeMap<Medal, u32>,
}

/// The medals as of a tick: each player's record by slot, and the last
/// [`RECENT`] awards, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub players: Vec<(u8, Record)>,
    pub recent: Vec<Award>,
}

impl View {
    pub const EMPTY: View = View {
        players: Vec::new(),
        recent: Vec::new(),
    };
}

/// Every player's chain, records and the last [`RECENT`] awards.
#[derive(Clone, Debug, Default)]
pub struct Medals {
    chains: Vec<Chain>,
    users: Vec<Option<u8>>,
    records: BTreeMap<u8, Record>,
    recent: VecDeque<Award>,
    seq: u64,
}

impl Medals {
    /// None yet: the slots are made at the first step.
    pub const fn empty() -> Medals {
        Medals {
            chains: Vec::new(),
            users: Vec::new(),
            records: BTreeMap::new(),
            recent: VecDeque::new(),
            seq: 0,
        }
    }

    pub fn new() -> Medals {
        Medals {
            chains: vec![Chain::default(); SLOTS],
            users: vec![None; SLOTS],
            ..Medals::default()
        }
    }

    /// One tick at game tick `tick` over the players `seen` (a slot not in
    /// it has no unit). Returns the awards made.
    pub fn step(&mut self, tick: u32, seen: &[Seen], spots: &[Spot]) -> Vec<Award> {
        if self.chains.len() < SLOTS {
            *self = Medals::new();
        }
        let mut out = Vec::new();
        for slot in 0..SLOTS {
            let s = seen.iter().find(|s| usize::from(s.slot) == slot);
            if let Some(s) = s {
                self.users[slot] = s.user;
            }
            let t = s.and_then(|s| s.tick);
            let event = self.chains[slot].step(t.as_ref(), spots);
            let slot = slot as u8;
            if let Some(e) = event {
                out.push(self.award(slot, tick, e.chain(), e.medals()));
            }
            self.sync(slot);
        }
        out
    }

    /// Files a bounce at count `chain` for player `slot` (local user
    /// `user`), as the chain's rule gives it, or a Bounced with
    /// `bounced`: `superbounce.medal_test`.
    pub fn test(
        &mut self,
        slot: u8,
        user: Option<u8>,
        tick: u32,
        chain: u32,
        bounced: bool,
    ) -> Award {
        if self.chains.len() < SLOTS {
            *self = Medals::new();
        }
        let i = usize::from(slot) % SLOTS;
        self.users[i] = user;
        let medals = if bounced {
            self.chains[i].end();
            vec![Medal::Bounced]
        } else {
            self.chains[i].force(chain);
            bounce_medals(chain).to_vec()
        };
        let a = self.award(slot, tick, chain, medals);
        self.sync(slot);
        a
    }

    fn award(&mut self, slot: u8, tick: u32, chain: u32, medals: Vec<Medal>) -> Award {
        self.seq += 1;
        let rec = self.records.entry(slot).or_default();
        for m in &medals {
            *rec.counts.entry(*m).or_default() += 1;
        }
        let a = Award {
            seq: self.seq,
            slot,
            user: self.users.get(usize::from(slot)).copied().flatten(),
            tick,
            chain,
            medals,
        };
        self.recent.push_back(a.clone());
        while self.recent.len() > RECENT {
            self.recent.pop_front();
        }
        a
    }

    /// A player's record follows its chain once it has one.
    fn sync(&mut self, slot: u8) {
        let c = &self.chains[usize::from(slot)];
        if let Some(r) = self.records.get_mut(&slot) {
            r.chain = c.count();
            r.live = c.live();
        }
    }

    /// The players that have earned a medal this game, by slot.
    pub fn records(&self) -> &BTreeMap<u8, Record> {
        &self.records
    }

    /// The last [`RECENT`] awards, oldest first.
    pub fn recent(&self) -> impl Iterator<Item = &Award> {
        self.recent.iter()
    }

    /// What `superbounce.state` and the HUD read.
    pub fn view(&self) -> View {
        View {
            players: self
                .records
                .iter()
                .map(|(slot, r)| (*slot, r.clone()))
                .collect(),
            recent: self.recent.iter().cloned().collect(),
        }
    }

    /// The chain of player `slot`.
    pub fn chain(&self, slot: u8) -> Option<&Chain> {
        self.chains.get(usize::from(slot))
    }
}

/// Every medal by its id, for checking the game's table against Live's.
pub fn medals_by_id() -> Vec<(&'static str, Medal)> {
    MEDALS.into_iter().map(|m| (m.id(), m)).collect()
}

// ---- the guest ----

/// Every listed player's slot, local user and unit reads, for
/// [`Medals::step`]: position, velocity, airborne and the level pair's
/// word (one pair read each, sixteen at most).
pub fn read_players(m: &dyn GuestMemory) -> Vec<Seen> {
    use engine::view::{objects, players};
    use live_telemetry::layout::{BIPED_FLAGS, BODY_VELOCITY};
    let Ok(Some(array)) = players::array(m) else {
        return Vec::new();
    };
    let Ok(live) = array.live(m, SLOTS as i32) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (handle, p) in live {
        if !p.listed(m).unwrap_or(false) {
            continue;
        }
        let slot = handle.index() as u8;
        let user = p.local_user(m).ok().filter(|u| *u >= 0).map(|u| u as u8);
        let tick = (|| {
            let unit = p.unit(m).ok()??;
            let (kind, body) = objects::get(m, unit).ok()??;
            let f = |at: u32| {
                m.u32(body.addr.wrapping_add(at))
                    .ok()
                    .map(f32::from_bits)
                    .filter(|v| v.is_finite())
            };
            let position = [
                f(crate::superbounce_mode::OBJECT_POSITION)?,
                f(crate::superbounce_mode::OBJECT_POSITION + 4)?,
                f(crate::superbounce_mode::OBJECT_POSITION + 8)?,
            ];
            let velocity = [
                f(BODY_VELOCITY)?,
                f(BODY_VELOCITY + 4)?,
                f(BODY_VELOCITY + 8)?,
            ];
            let airborne = kind == objects::BIPED
                && m.u16(body.addr.wrapping_add(BIPED_FLAGS))
                    .is_ok_and(|flags| flags & 1 != 0);
            let word = live_telemetry::havok_agents::fixed_pair_word(m, unit.raw())
                .ok()
                .flatten();
            Some(PlayerTick {
                unit: unit.raw(),
                position,
                velocity,
                airborne,
                word,
            })
        })();
        out.push(Seen { slot, user, tick });
    }
    out
}

#[cfg(test)]
#[path = "superbounce_chain_tests.rs"]
mod tests;
