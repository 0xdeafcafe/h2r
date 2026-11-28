//! Teabags (docs/game/teabag.md): a living player crouching over the body
//! of a player who just died. Every game type, humans and bots alike.
//!
//! [`Watch`] keeps every player's crouch and every fresh body, stepped once
//! a game tick with that tick's reads ([`Seen`], and a read of any unit by
//! its datum); it is pure, so the tests are lists of ticks. [`read_players`]
//! and [`read_unit`] read the guest. The feature ([`TeabagWatch`]) steps it
//! from the tick observer and records each teabag for the film and Live
//! through [`record`], as `Telemetry::teabag`, on the console whose
//! simulation is the game's. The film and Live work out the `incel` medal
//! from the teabags at the game's end (`live_proto::film::events::medals`).
//!
//! Ticks only: the count is the same at any frame rate. It is not in a
//! snapshot: after a state load or a Theater seek the crouches and bodies
//! in flight are forgotten.

use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::GuestMemory;

/// A world position.
pub type Vec3 = [f32; 3];

/// Player slots the game has.
pub const SLOTS: usize = 16;
/// Crouches (a press then its release) over one body that make a teabag.
pub const CROUCHES: usize = 2;
/// Ticks from the first of those crouches' press to the last one's
/// release: 3 s at 30 Hz.
pub const WINDOW_TICKS: u32 = 90;
/// A body is fresh for this many ticks after its player died: 10 s.
pub const BODY_TICKS: u32 = 300;
/// A crouch counts over a body when the crouching player's origin is
/// within this distance of the body's, across the floor (world units,
/// about 4.5 m), at its press and at its release. The bots' emote gate
/// (`bots::emote::NEAR`) is no wider, so a bot's teabag counts.
pub const RADIUS: f32 = 1.5;
/// And within this distance of it up or down (world units): a body a
/// floor below is not under the player.
pub const HEIGHT: f32 = 1.5;
/// Bodies watched at most; the oldest goes first.
pub const MAX_BODIES: usize = 32;

/// What one tick reads of a unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitRead {
    /// The object's origin.
    pub position: Vec3,
    /// A biped whose body is not depleted (object damage flags bit 2).
    pub alive: bool,
    /// The crouch request, unit flags bit 23: set while crouch is held.
    pub crouch: bool,
}

/// One player as read this tick: the slot and the player's unit, its datum
/// and read (`None` without one).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub slot: u8,
    pub unit: Option<(u32, UnitRead)>,
}

/// A teabag: the player in slot `player` crouched [`CROUCHES`] times over
/// the body of the player in slot `victim`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Teabag {
    pub tick: u32,
    pub player: u8,
    pub victim: u8,
}

/// A body: whose, its unit, the tick its player died and where it is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Body {
    id: u64,
    victim: u8,
    unit: u32,
    died: u32,
    position: Vec3,
}

/// One crouch over one body: its press's tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Crouch {
    press: u32,
    body: u64,
}

/// One player's crouch.
#[derive(Clone, Debug, Default, PartialEq)]
struct Player {
    /// The living unit last tick.
    life: Option<u32>,
    crouch: bool,
    /// The held crouch's press: its tick and where.
    press: Option<(u32, Vec3)>,
    /// Crouches over bodies, the last [`WINDOW_TICKS`]' worth.
    crouches: VecDeque<Crouch>,
}

impl Player {
    fn forget(&mut self) {
        *self = Player::default();
    }
}

/// Every player's crouch and the fresh bodies.
#[derive(Clone, Debug, Default)]
pub struct Watch {
    players: Vec<Player>,
    bodies: VecDeque<Body>,
    next: u64,
    /// The (player, body) pairs that made a teabag already: one each.
    done: BTreeSet<(u8, u64)>,
}

fn near(a: Vec3, b: Vec3) -> bool {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    (dx * dx + dy * dy).sqrt() <= RADIUS && (a[2] - b[2]).abs() <= HEIGHT
}

impl Watch {
    pub const fn new() -> Watch {
        Watch {
            players: Vec::new(),
            bodies: VecDeque::new(),
            next: 0,
            done: BTreeSet::new(),
        }
    }

    /// One tick at game tick `tick` over the players `seen` (a slot not in
    /// it has no unit); `read` reads a unit by its datum (`None` when the
    /// object is gone). Returns the teabags made.
    pub fn step(
        &mut self,
        tick: u32,
        seen: &[Seen],
        read: &dyn Fn(u32) -> Option<UnitRead>,
    ) -> Vec<Teabag> {
        if self.players.len() < SLOTS {
            self.players = vec![Player::default(); SLOTS];
        }
        // The bodies: moved as their objects move, gone with them or when
        // stale.
        self.bodies.retain_mut(|b| {
            let Some(r) = read(b.unit).filter(|_| tick.saturating_sub(b.died) <= BODY_TICKS) else {
                return false;
            };
            b.position = r.position;
            true
        });
        // A death: last tick's living unit is depleted now. A unit that
        // went (a player who left) leaves no body; one still alive (a seat
        // in a vehicle, a new unit) is no death.
        for slot in 0..SLOTS {
            let Some(unit) = self.players[slot].life else {
                continue;
            };
            if let Some(r) = read(unit).filter(|r| !r.alive) {
                self.next += 1;
                self.bodies.push_back(Body {
                    id: self.next,
                    victim: slot as u8,
                    unit,
                    died: tick,
                    position: r.position,
                });
                if self.bodies.len() > MAX_BODIES {
                    self.bodies.pop_front();
                }
            }
        }
        let mut out = Vec::new();
        for slot in 0..SLOTS {
            let now = seen
                .iter()
                .find(|s| usize::from(s.slot) == slot)
                .and_then(|s| s.unit)
                .filter(|(_, r)| r.alive);
            let p = &mut self.players[slot];
            let Some((unit, r)) = now else {
                // Dead or gone: a crouch while dead is none.
                p.forget();
                continue;
            };
            if p.life != Some(unit) {
                // A new life: a crouch held into it is no press.
                p.forget();
                p.life = Some(unit);
                p.crouch = r.crouch;
                continue;
            }
            let was = std::mem::replace(&mut p.crouch, r.crouch);
            match (was, r.crouch) {
                (false, true) => p.press = Some((tick, r.position)),
                (true, false) => {
                    let Some((press, at)) = p.press.take() else {
                        continue;
                    };
                    // Over a body (fresh: the stale ones went above) of
                    // someone else, pressed after the death.
                    for b in &self.bodies {
                        if usize::from(b.victim) == slot
                            || press < b.died
                            || !near(at, b.position)
                            || !near(r.position, b.position)
                            || self.done.contains(&(slot as u8, b.id))
                        {
                            continue;
                        }
                        p.crouches.push_back(Crouch { press, body: b.id });
                        let over = p
                            .crouches
                            .iter()
                            .filter(|c| {
                                c.body == b.id && tick.saturating_sub(c.press) <= WINDOW_TICKS
                            })
                            .count();
                        if over >= CROUCHES {
                            self.done.insert((slot as u8, b.id));
                            out.push(Teabag {
                                tick,
                                player: slot as u8,
                                victim: b.victim,
                            });
                        }
                    }
                }
                _ => {}
            }
            p.crouches
                .retain(|c| tick.saturating_sub(c.press) <= WINDOW_TICKS);
        }
        // Pairs whose body has gone can go too.
        let live: BTreeSet<u64> = self.bodies.iter().map(|b| b.id).collect();
        self.done.retain(|(_, id)| live.contains(id));
        out
    }

    /// The bodies being watched: whose, and where.
    pub fn bodies(&self) -> impl Iterator<Item = (u8, Vec3)> + '_ {
        self.bodies.iter().map(|b| (b.victim, b.position))
    }
}

// ---- the record ----

/// Where the teabags go for the film and Live: `live_telemetry::Telemetry`
/// in the game.
pub trait Recorder {
    /// A teabag by the player in slot `player` over the body of the one in
    /// slot `victim`.
    fn teabag(&self, guest: &runtime::Guest, player: u32, victim: u32);
}

impl Recorder for live_telemetry::Telemetry {
    fn teabag(&self, guest: &runtime::Guest, player: u32, victim: u32) {
        live_telemetry::Telemetry::teabag(self, guest, player, victim);
    }
}

/// Records `teabags` (one tick's, from [`Watch::step`]) to `to`, when this
/// console records the game (`superbounce_chain::records`: not a
/// synchronous or distributed client of the simulation, whose players'
/// teabags the host records).
pub fn record(to: &dyn Recorder, guest: &runtime::Guest, teabags: &[Teabag]) {
    if teabags.is_empty() || !crate::superbounce_chain::records(crate::coop_look::view::mode(guest))
    {
        return;
    }
    for t in teabags {
        to.teabag(guest, u32::from(t.player), u32::from(t.victim));
    }
}

// ---- the guest ----

/// A unit read by its datum: origin, alive and the crouch request.
pub fn read_unit(m: &dyn GuestMemory, unit: u32) -> Option<UnitRead> {
    use guest_layout::view::objects;
    use live_telemetry::layout::{BODY_DEPLETED, UNIT_FLAG_CROUCH, UNIT_FLAGS};
    let handle = guest_layout::handle::DatumHandle::from_raw(unit)?;
    let (kind, body) = objects::get(m, handle).ok()??;
    let position = body.position(m).ok()?;
    if !position.iter().all(|v| v.is_finite()) {
        return None;
    }
    let depleted = !body.damage_flags(m).is_ok_and(|f| f & BODY_DEPLETED == 0);
    let biped = kind == objects::BIPED;
    let crouch = biped
        && m.u32(body.addr.wrapping_add(UNIT_FLAGS))
            .is_ok_and(|flags| flags & UNIT_FLAG_CROUCH != 0);
    Some(UnitRead {
        position,
        alive: biped && !depleted,
        crouch,
    })
}

/// Every listed player's slot and unit, for [`Watch::step`].
pub fn read_players(m: &dyn GuestMemory) -> Vec<Seen> {
    use guest_layout::view::players;
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
        let unit = p
            .unit(m)
            .ok()
            .flatten()
            .and_then(|u| Some((u.raw(), read_unit(m, u.raw())?)));
        out.push(Seen {
            slot: handle.index() as u8,
            unit,
        });
    }
    out
}

/// The game tick, from the game-time record.
fn game_tick(m: &dyn GuestMemory) -> Option<u32> {
    use live_telemetry::layout::{GAME_TIME_PTR, TIME_TICK};
    let time = m.u32(GAME_TIME_PTR).ok().filter(|&t| t != 0)?;
    m.u32(time.wrapping_add(TIME_TICK)).ok()
}

// ---- the feature ----

/// The feature's id.
pub const FEATURE: &str = "teabag";

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static WATCH: Mutex<Watch> = Mutex::new(Watch::new());
/// Live telemetry, which records the teabags; `None` without Live.
static TELEMETRY: Mutex<Option<Arc<live_telemetry::Telemetry>>> = Mutex::new(None);

fn on_tick(o: &hooks::Obs) {
    let teabags = {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        let Some(tick) = game_tick(m) else {
            return;
        };
        let seen = read_players(m);
        lock(&WATCH).step(tick, &seen, &|unit| read_unit(m, unit))
    };
    if !teabags.is_empty()
        && let Some(t) = lock(&TELEMETRY).clone()
    {
        let guest = o.guest();
        record(&*t, &guest, &teabags);
    }
}

/// The tick observer that watches for teabags.
pub struct TeabagWatch;

impl hooks::Feature for TeabagWatch {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Teabags, for the incel medal")
            .doc("docs/game/teabag.md")
            // A tick observer and start and end observers only: a recorded
            // film's hook digest does not include it.
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // The watch is rebuilt from guest RAM tick by tick; after a load it
        // starts again from the next tick.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.game()
            .on_start("teabag: a new game", |_| *lock(&WATCH) = Watch::new());
        b.game()
            .on_end("teabag: the game ended", |_| *lock(&WATCH) = Watch::new());
        b.frame().on_tick("teabag: watch", on_tick);
        Ok(())
    }
}

/// Adds the feature to `registry`, recording the teabags to `telemetry`.
pub fn install(
    registry: &mut hooks::Registry,
    telemetry: Option<Arc<live_telemetry::Telemetry>>,
) -> anyhow::Result<()> {
    *lock(&TELEMETRY) = telemetry;
    registry
        .add(&TeabagWatch)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("teabag: {e}"))
}

#[cfg(test)]
#[path = "teabag_tests.rs"]
mod tests;
