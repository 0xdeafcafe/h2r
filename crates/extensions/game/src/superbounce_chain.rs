//! Superbounce medals (docs/game/superbounce.md, "Medals"): the chain of
//! bounces each player makes with no ground between them, the medals each
//! bounce earns, and Bounced, a fall out of the map or through a strip's
//! floor during a bounce.
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
//!
//! Each step also says what the chain saw of the player's physics
//! ([`Physics`]): every touchdown, each landing judged a bounce with its
//! launch, and the end of a run of bounces. A rules script hears them as
//! `on touchdown`, `on launch` and `on grounded` (docs/game/rules-world-api.md,
//! "Physics") and keeps its own chain on them; the Superbounce script's
//! chain is this one's, step for step (`superbounce_chain_tests.rs`).
//!
//! Each award of a tick is recorded for the film and Live through
//! [`record`]: a bounce as `Telemetry::superbounce`, a Bounced as
//! `Telemetry::bounced`, on the console whose simulation is the game's,
//! as the kills are (docs/game/superbounce.md, "Recording").

use std::collections::{BTreeMap, VecDeque};

use guest_layout::GuestMemory;
use hud_kit_core::views::superbounce::{MEDALS, Medal};
use superbounce::{Spot, Vec3};

use crate::superbounce_charge::{
    LAUNCH_TICKS, LandingResult, Pending, TOUCHDOWN_VZ, Through, judge, strips_near, through,
    touchdown,
};
use crate::superbounce_mode::primed;

/// Player slots the game has.
pub const SLOTS: usize = 16;
/// From this tick after a bounce's touchdown (the touchdown's own is 0),
/// a tick with the airborne bit clear ends the chain: a soft landing at
/// the top of the flight is no touchdown. The two ticks before it are the
/// launch's, when the bit may still be clear. It is the tick the launch
/// window closes on ([`LAUNCH_TICKS`]), so the bounce is filed first.
pub const CLEAR_FROM: u32 = 3;
/// An origin that moves more than this (world units) in one tick was
/// moved (a teleporter, `sim.state.set`), and the chain ends.
pub const TELEPORT: f32 = 3.0;
/// Awards kept for `superbounce.state`'s `recent`.
pub const RECENT: usize = 8;
/// A bounce whose launch (the best vertical speed of its window,
/// [`Pending`]) comes out over this (units/s) is a Monster Bounce: above
/// the top of every measured launch (7.97 to 13.22 on Ascension's strip,
/// docs/game/superbounce.md, "Traces"), by about a fifth. Reasoned from
/// those, not measured: no Monster has been traced.
pub const MONSTER_VZ: f32 = 16.0;
/// Game ticks a fall through a strip's floor keeps its Bounced armed
/// (`superbounce_charge::through`): 10 s. The Guardians take a second or
/// more to kill a player under the map, and a kill barrier or a fatal
/// landing below can take longer; a player still alive after it dropped
/// through to a floor below and lived.
pub const BOUNCED_TICKS: u32 = 300;

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

/// The medals of a bounce at chain count `chain`: [`bounce_medals`], and
/// Monster Bounce last for a Monster.
pub fn bounce_medals_with(chain: u32, monster: bool) -> Vec<Medal> {
    let mut v = bounce_medals(chain).to_vec();
    if monster && !v.is_empty() {
        v.push(Medal::Monster);
    }
    v
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
    /// The unit crouch request on a living biped.
    pub crouching: bool,
    /// The level pair's word; `None` without a pair.
    pub word: Option<u32>,
}

/// A bounce spot's id as a rules script's `on bounce(p, spot)` hears it
/// (docs/game/rules-engine.md, "Firefight, AI and bounces"): FNV-1a over
/// the bits of its edge, in 31 bits so it is never `none`. The scan is a
/// function of the map's geometry, so the id is the same in every game on
/// the map, on every machine and in a film, whatever the verified file
/// says.
pub fn spot_id(spot: &Spot) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for v in spot.edge.iter().flatten() {
        for b in v.to_bits().to_le_bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
    }
    h & 0x7fff_ffff
}

/// What a step gave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A bounce, the chain's count with it, off the strip [`spot_id`]
    /// names (`None` off every strip: a seam the marks don't know).
    /// `monster` when it came out over [`MONSTER_VZ`]; `strips` is how
    /// many bounce strips were within `LANDING_RADIUS` of the touchdown,
    /// kept for study of Monsters (a V of seams), not a test.
    Bounce {
        chain: u32,
        spot: Option<u32>,
        monster: bool,
        strips: u8,
    },
    /// The unit went (a death) on the tick after one that was airborne,
    /// falling at [`TOUCHDOWN_VZ`] or faster, and either in a chain or
    /// primed: a fall out of the map during a bounce. Or it went within
    /// [`BOUNCED_TICKS`] of a fall through a strip's floor
    /// (`superbounce_charge::through`) with no bounce since. `chain` is the
    /// count of the chain it was in, 0 if none.
    Bounced { chain: u32 },
}

impl Event {
    /// The medals the event earns.
    pub fn medals(self) -> Vec<Medal> {
        match self {
            Event::Bounce { chain, monster, .. } => bounce_medals_with(chain, monster),
            Event::Bounced { .. } => vec![Medal::Bounced],
        }
    }
    /// The chain's count with the event.
    pub fn chain(self) -> u32 {
        match self {
            Event::Bounce { chain, .. } | Event::Bounced { chain } => chain,
        }
    }
    /// The bounce's strip ([`spot_id`]); `None` for a Bounced and a
    /// bounce off every strip.
    pub fn spot(self) -> Option<u32> {
        match self {
            Event::Bounce { spot, .. } => spot,
            Event::Bounced { .. } => None,
        }
    }
    /// The bounce strips near its touchdown; `None` for a Bounced.
    pub fn strips(self) -> Option<u8> {
        match self {
            Event::Bounce { strips, .. } => Some(strips),
            Event::Bounced { .. } => None,
        }
    }
}

/// What a step saw of the player's physics, in the order it saw it: the
/// host half of the world API's physics events
/// (docs/game/rules-world-api.md, "Physics"). A rules script that keeps
/// a chain on [`Physics::Launch`] and [`Physics::Grounded`] counts as
/// [`Chain`] does: the count grows on a launch while the run is live and
/// starts at 1 otherwise, and a Grounded comes after every launch whose
/// run then ends, before the next launch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Physics {
    /// A landing ([`touchdown`]): the fall before (units/s, positive) and
    /// the vertical speed on the touchdown tick (units/s, up positive).
    /// None is seen while a touchdown's launch window is open, or under a
    /// strip's floor after a fall through it.
    Touchdown { down: f32, up: f32 },
    /// A landing judged a bounce, when its launch window closed: the strip
    /// ([`spot_id`], `None` off every marked strip) and the launch (the
    /// best vertical speed of its window, units/s).
    Launch { spot: Option<u32>, speed: f32 },
    /// The run of bounces ended: [`Chain::live`] went from true to false.
    Grounded,
}

/// What a step read of a player's biped, for the rules script's reads
/// `p.airborne`, `p.crouching` and `p.primed` (docs/game/rules-world-api.md, "Physics").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Body {
    pub slot: u8,
    /// Biped flags bit 0 ([`PlayerTick::airborne`]).
    pub airborne: bool,
    /// The unit crouch request on a living biped.
    pub crouching: bool,
    /// The level pair's word is the primed one ([`primed`]).
    pub primed: bool,
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

/// A Bounced waiting on the unit going: a fall through a strip's floor.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Armed {
    /// The chain's count when it fell through, 0 if none was live.
    chain: u32,
    /// Ticks since the fall through (0 on its tick).
    ticks: u32,
    /// The fall through: a touchdown under its floor by its edge is the
    /// same fall ([`Through::under`]).
    through: Through,
}

/// One player's chain.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chain {
    count: u32,
    live: bool,
    /// Ticks since the last bounce's touchdown (0 on its tick).
    since: u32,
    prev: Option<Prev>,
    /// A touchdown whose launch is being read.
    pending: Option<Pending>,
    /// A fall through a strip's floor, awarding a Bounced when the unit
    /// goes.
    armed: Option<Armed>,
    /// What the last step saw ([`Chain::physics`]).
    seen: Vec<Physics>,
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

    /// Whether a fall through a strip's floor has armed a Bounced: the
    /// unit going within [`BOUNCED_TICKS`] earns it.
    pub fn armed(&self) -> bool {
        self.armed.is_some()
    }

    /// What the last [`Chain::step`] saw of the player's physics, in order.
    pub fn physics(&self) -> &[Physics] {
        &self.seen
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

    /// Judges the pending touchdown: a primed bounce adds to the chain
    /// (or starts one) and is returned; anything else ends it.
    fn settle(&mut self, spots: &[Spot]) -> Option<Event> {
        let p = self.pending.take()?;
        let judged = judge(spots, &p).filter(|j| j.result == LandingResult::Bounce && p.primed());
        let Some(j) = judged else {
            self.live = false;
            return None;
        };
        self.count = if self.live { self.count + 1 } else { 1 };
        self.live = true;
        self.since = LAUNCH_TICKS;
        let spot = j.spot.map(|i| spot_id(&spots[i]));
        self.seen.push(Physics::Launch {
            spot,
            speed: p.launch,
        });
        Some(Event::Bounce {
            chain: self.count,
            spot,
            monster: p.launch > MONSTER_VZ,
            strips: strips_near(spots, p.at).min(255) as u8,
        })
    }

    /// The unit went (or was replaced) after the tick `prev`: what was
    /// pending settles, and a Bounced is earned once, by an armed fall
    /// through (its chain) or by `prev` being a fall during a bounce. A
    /// bounce settled here is the event instead. The chain ends.
    fn gone(&mut self, prev: Option<Prev>, spots: &[Spot]) -> Option<Event> {
        let settled = self.settle(spots);
        let fell = prev.is_some_and(|p| self.falling(&p));
        let chain = if self.live { self.count } else { 0 };
        let armed = self.armed.take();
        self.live = false;
        settled.or(match armed {
            Some(a) => Some(Event::Bounced { chain: a.chain }),
            None => fell.then_some(Event::Bounced { chain }),
        })
    }

    /// One tick: `t` is the player's unit, `None` when it has none (dead,
    /// or not in the game). A touchdown is judged when its launch window
    /// closes ([`Pending`]), [`LAUNCH_TICKS`] after it; a window cut short
    /// (the unit gone, a new one, a teleport) is judged on what was seen.
    /// A fall through a strip's floor (`superbounce_charge::through`) drops
    /// the touchdown it follows, ends the chain and arms a Bounced for
    /// [`BOUNCED_TICKS`]; a bounce or a teleport disarms it.
    ///
    /// What the step saw of the player's physics is [`Chain::physics`]
    /// after it: a [`Physics::Grounded`] whenever the chain was live, or
    /// went live in the step, and is not after it.
    pub fn step(&mut self, t: Option<&PlayerTick>, spots: &[Spot]) -> Option<Event> {
        self.seen.clear();
        let was = self.live;
        let event = self.step_inner(t, spots);
        let launched = self
            .seen
            .iter()
            .any(|p| matches!(p, Physics::Launch { .. }));
        if (was || launched) && !self.live {
            self.seen.push(Physics::Grounded);
        }
        event
    }

    fn step_inner(&mut self, t: Option<&PlayerTick>, spots: &[Spot]) -> Option<Event> {
        let Some(t) = t else {
            let prev = self.prev.take();
            return self.gone(prev, spots);
        };
        let now = Prev {
            unit: t.unit,
            position: t.position,
            vz: t.velocity[2],
            airborne: t.airborne,
            word: t.word,
        };
        let p = self.prev.replace(now)?;
        if p.unit != t.unit {
            // A new unit on the same tick the old one went: a death and a
            // respawn, or a vehicle.
            return self.gone(Some(p), spots);
        }
        if distance(p.position, t.position) > TELEPORT || !t.velocity[2].is_finite() {
            let settled = self.settle(spots);
            self.live = false;
            self.armed = None;
            return settled;
        }
        if let Some(a) = self.armed.as_mut() {
            a.ticks += 1;
            if a.ticks >= BOUNCED_TICKS {
                // Alive 10 s after it: it dropped to a floor below.
                self.armed = None;
            }
        }
        let went = through(
            spots,
            p.position,
            p.vz,
            p.word,
            t.position,
            self.pending.as_ref(),
            self.live,
        );
        if let Some(th) = went {
            // The touchdown it follows was the seam hit: dropped, not
            // judged.
            self.pending = None;
            let chain = if self.live { self.count } else { 0 };
            self.live = false;
            if self.armed.is_none() {
                self.armed = Some(Armed {
                    chain,
                    ticks: 0,
                    through: th,
                });
            }
            return None;
        }
        let sunk = self.armed.is_some_and(|a| a.through.under(p.position));
        let mut event = None;
        if let Some(pending) = self.pending.as_mut() {
            if !pending.see(now.vz) {
                return None;
            }
            event = self.settle(spots);
            if matches!(event, Some(Event::Bounce { .. })) {
                self.armed = None;
            }
        } else if !sunk && touchdown(p.airborne, p.vz, t.airborne, now.vz) {
            self.seen.push(Physics::Touchdown {
                down: -p.vz,
                up: now.vz,
            });
            if self.live || p.word.is_some_and(primed) {
                self.pending = Some(Pending::new(p.word, p.position, p.vz, 0, now.vz));
            } else {
                self.live = false;
            }
            return None;
        } else if self.live {
            self.since = self.since.saturating_add(1);
        }
        if self.live && self.since >= CLEAR_FROM && !t.airborne {
            self.live = false;
        }
        event
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
    /// A bounce's strip ([`spot_id`]); `None` for a Bounced and for
    /// `superbounce.medal_test`'s awards, which no strip made.
    pub spot: Option<u32>,
    /// For a bounce, the bounce strips within `LANDING_RADIUS` of its
    /// touchdown ([`Event::strips`]); `None` otherwise.
    pub strips: Option<u8>,
}

impl Award {
    /// Whether it was a Monster Bounce.
    pub fn monster(&self) -> bool {
        self.medals.contains(&Medal::Monster)
    }
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
    /// What the last step saw of each player's physics, by slot.
    heard: Vec<(u8, Physics)>,
    /// The bodies of the players with a unit at the last step, by slot.
    bodies: Vec<Body>,
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
            heard: Vec::new(),
            bodies: Vec::new(),
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
        self.heard.clear();
        self.bodies.clear();
        for slot in 0..SLOTS {
            let s = seen.iter().find(|s| usize::from(s.slot) == slot);
            if let Some(s) = s {
                self.users[slot] = s.user;
            }
            let t = s.and_then(|s| s.tick);
            if let Some(t) = t {
                self.bodies.push(Body {
                    slot: slot as u8,
                    airborne: t.airborne,
                    crouching: t.crouching,
                    primed: t.word.is_some_and(primed),
                });
            }
            let event = self.chains[slot].step(t.as_ref(), spots);
            let saw = self.chains[slot].physics();
            self.heard.extend(saw.iter().map(|p| (slot as u8, *p)));
            let slot = slot as u8;
            if let Some(e) = event {
                out.push(self.award(slot, tick, e.chain(), e.medals(), e.spot(), e.strips()));
            }
            self.sync(slot);
        }
        out
    }

    /// Files a bounce at count `chain` for player `slot` (local user
    /// `user`), as the chain's rule gives it (a Monster with `monster`),
    /// or a Bounced with `bounced`: `superbounce.medal_test`.
    pub fn test(
        &mut self,
        slot: u8,
        user: Option<u8>,
        tick: u32,
        chain: u32,
        bounced: bool,
        monster: bool,
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
            bounce_medals_with(chain, monster)
        };
        let a = self.award(slot, tick, chain, medals, None, None);
        self.sync(slot);
        a
    }

    fn award(
        &mut self,
        slot: u8,
        tick: u32,
        chain: u32,
        medals: Vec<Medal>,
        spot: Option<u32>,
        strips: Option<u8>,
    ) -> Award {
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
            spot,
            strips,
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

    /// What the last [`Medals::step`] read of each player's biped that had
    /// a unit, by slot, for the rules script's biped reads.
    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    /// What the last [`Medals::step`] saw of the players' physics, slot
    /// by slot and in order within a slot ([`Chain::physics`]): what a
    /// rules script hears as `on touchdown`, `on launch` and `on grounded`.
    pub fn physics(&self) -> &[(u8, Physics)] {
        &self.heard
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

// ---- the record ----

/// Where the awards go for the film and Live: `live_telemetry::Telemetry`
/// in the game.
pub trait Recorder {
    /// A bounce by the player in slot `player`, `chain` into its chain;
    /// `monster` for a Monster Bounce.
    fn superbounce(&self, guest: &runtime::Guest, player: u32, chain: u32, monster: bool);
    /// A Bounced of the player in slot `player`, ending a chain of
    /// `chain` (0 if none).
    fn bounced(&self, guest: &runtime::Guest, player: u32, chain: u32);
}

impl Recorder for live_telemetry::Telemetry {
    fn superbounce(&self, guest: &runtime::Guest, player: u32, chain: u32, monster: bool) {
        live_telemetry::Telemetry::superbounce(self, guest, player, chain, monster);
    }
    fn bounced(&self, guest: &runtime::Guest, player: u32, chain: u32) {
        live_telemetry::Telemetry::bounced(self, guest, player, chain);
    }
}

/// Whether this console records the game, by the simulation world's mode
/// (`coop_look::view::mode`): not a synchronous or a distributed client.
/// A kill is recorded where the game's own damage code runs, which is that
/// console; a client's mode would see its players' bounces from its own
/// simulation and record them twice.
pub fn records(world_mode: Option<u32>) -> bool {
    use crate::firefight::runtime::{WORLD_DIST_CLIENT, WORLD_SYNC_CLIENT};
    !matches!(world_mode, Some(WORLD_SYNC_CLIENT | WORLD_DIST_CLIENT))
}

/// Records `awards` (one tick's, from [`Medals::step`]) to `to`, when
/// this console records the game ([`records`]): a Bounced as
/// [`Recorder::bounced`], any other award as [`Recorder::superbounce`]
/// at its chain's count and whether it was a Monster, each for its player
/// slot. Every bounce is one
/// call; the film and Live work its medals out from the count and the
/// flag.
/// `superbounce.medal_test`'s awards are not stepped, so not recorded.
pub fn record(to: &dyn Recorder, guest: &runtime::Guest, awards: &[Award]) {
    if awards.is_empty() || !records(crate::coop_look::view::mode(guest)) {
        return;
    }
    for a in awards {
        let player = u32::from(a.slot);
        if a.medals.contains(&Medal::Bounced) {
            to.bounced(guest, player, a.chain);
        } else if !a.medals.is_empty() {
            to.superbounce(guest, player, a.chain, a.monster());
        }
    }
}

/// Every medal by its id, for checking the game's table against Live's.
pub fn medals_by_id() -> Vec<(&'static str, Medal)> {
    MEDALS.into_iter().map(|m| (m.id(), m)).collect()
}

// ---- the guest ----

/// Every listed player's slot, local user and unit reads, for
/// [`Medals::step`]: position, velocity, airborne, crouching and the level pair's
/// word (one pair read each, sixteen at most).
pub fn read_players(m: &dyn GuestMemory) -> Vec<Seen> {
    use guest_layout::view::{objects, players};
    use live_telemetry::layout::{
        BIPED_FLAGS, BODY_DEPLETED, BODY_VELOCITY, UNIT_FLAG_CROUCH, UNIT_FLAGS,
    };
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
            let crouching = kind == objects::BIPED
                && body.damage_flags(m).is_ok_and(|flags| flags & BODY_DEPLETED == 0)
                && m.u32(body.addr.wrapping_add(UNIT_FLAGS))
                    .is_ok_and(|flags| flags & UNIT_FLAG_CROUCH != 0);
            let word = live_telemetry::havok_agents::fixed_pair_word(m, unit.raw())
                .ok()
                .flatten();
            Some(PlayerTick {
                unit: unit.raw(),
                position,
                velocity,
                airborne,
                crouching,
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
