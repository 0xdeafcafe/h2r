//! Co-op with more than two players (docs/live/live-coop.md section 12):
//! where players 3 to 16 start, and keeping them with the others when the
//! level's scripts move only the first two.
//!
//! The engine already spawns, respawns and ends the game over all 16
//! player slots. What holds at two is the levels' data and scripts:
//!
//! * **Starting locations.** Every campaign scenario has two (00a has one).
//!   `PLAYER_SPAWN_LOCATION` (`0x14ef00`) skips a spot a biped already
//!   stands on and answers `0xffff`, so a third player's first spawn fails
//!   and is retried each tick until P1 or P2 steps off a spot. Here, for a
//!   player of index 2 or more, a `0xffff` becomes the spot `index % count`,
//!   and the spawn is followed by retail's co-op placement
//!   (`PLAYER_PLACE_NEAR`, `0x14e970`) next to player 0, as the co-op
//!   respawn does after its own spawn (`0x14cfeb`, `0x14cffd`). Both happen
//!   inside the one spawn call, so the new unit never stands a tick on the
//!   occupied spot.
//! * **Script teleports.** The scripts name `player0` and `player1` only:
//!   `object_teleport` (`SCRIPT_TELEPORT`, `0x29ffb0`) moves those two to a
//!   cutscene flag and leaves the rest behind. (`volume_teleport_players_not_inside`
//!   already walks every player.) After a teleport of player 0's or 1's
//!   unit, a BSP switch, or the end of a cinematic, the next player update
//!   moves every player of index 2 or more who is on foot and more than
//!   [`LEASH`] world units from the anchor next to it, with the same
//!   placement. Never during a cinematic.
//!
//! The hooks act only while co-op over Live is armed ([`Arm`]), and only on
//! players of index 2 or more, who exist only when the host's
//! `coop.max_players` let a third console in. They are part of the
//! simulation, so they must run alike on every console: they never read a
//! per-console setting.
//!
//! The decisions are pure functions over guest memory, tested on a fake
//! guest; the guest calls are async entries run by the engine's test
//! driver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use engine::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use engine::memory::GuestMemory;
use engine::points;
use feature::Flow;
use runtime::guest::Permissions;

use crate::coop_probe::Arm;

/// The feature id (`H2_FEATURES=-coop-players` turns it off).
pub const FEATURE: &str = "coop-players";

/// How far (world units) a player of index 2 or more may be from the
/// anchor before a teleport, BSP switch or cinematic's end brings them
/// over. [guess] A world unit is about three metres; 8 keeps a squad in
/// one room and does not chase someone a few steps away.
pub const LEASH: f32 = 8.0;

/// The first player index the scripts do not name.
pub const FIRST_EXTRA: u16 = 2;

/// Guest layouts (docs/live/live-coop.md section 12, all read in the
/// disassembly).
pub mod layout {
    /// Datum array header: element data, the index limit, the in-use bits.
    pub const ARRAY_DATA: u32 = 0x44;
    pub const ARRAY_LIMIT: u32 = 0x38;
    pub const ARRAY_IN_USE: u32 = 0x48;
    /// A player datum: salt (word) at `+0`, flags byte at `+2` (bit 3 a
    /// first spawn pending), the unit's handle at `+0x2c`.
    pub const PLAYER_SIZE: u32 = 0x21c;
    pub const PLAYER_FLAGS: u32 = 2;
    pub const FIRST_SPAWN: u8 = 0x08;
    pub const PLAYER_UNIT: u32 = 0x2c;
    /// An object header is 12 bytes, the object's address at `+8`; an
    /// object's parent handle is at `+0x14` (-1 for none) and the position
    /// the co-op respawn anchors on at `+0x30` (`0x14cf74`).
    pub const OBJECT_HEADER: u32 = 12;
    pub const OBJECT_ADDRESS: u32 = 8;
    pub const OBJECT_PARENT: u32 = 0x14;
    pub const OBJECT_POSITION: u32 = 0x30;
    /// Scenario: starting-location count.
    pub const SCENARIO_STARTS: u32 = 0x100;
    /// Game globals: the options' mode (1 campaign) at `+8`.
    pub const OPTIONS_MODE: u32 = 8;
    pub const CAMPAIGN: u32 = 1;
    /// Players a game holds.
    pub const MAX_PLAYERS: u32 = 16;
}

use layout::*;

pub const NONE: u32 = u32::MAX;
pub const NO_LOCATION: u16 = 0xffff;

// ---- reading ----

/// One player slot in use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Player {
    pub index: u16,
    pub handle: u32,
    pub flags: u8,
    /// The unit's handle, or [`NONE`].
    pub unit: u32,
}

/// The players in use, in table order.
pub fn players<M: GuestMemory + ?Sized>(m: &M) -> Vec<Player> {
    let Ok(hdr) = m.u32(points::PLAYERS.base) else {
        return Vec::new();
    };
    let (Ok(data), Ok(limit), Ok(bits)) = (
        m.u32(hdr.wrapping_add(ARRAY_DATA)),
        m.u32(hdr.wrapping_add(ARRAY_LIMIT)),
        m.u32(hdr.wrapping_add(ARRAY_IN_USE)),
    ) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0..limit.min(MAX_PLAYERS) {
        let word = m.u32(bits.wrapping_add(4 * (i >> 5))).unwrap_or(0);
        if word & (1 << (i & 31)) == 0 {
            continue;
        }
        let at = data.wrapping_add(i * PLAYER_SIZE);
        let (Ok(salt), Ok(flags), Ok(unit)) = (
            m.u16(at),
            m.u8(at.wrapping_add(PLAYER_FLAGS)),
            m.u32(at.wrapping_add(PLAYER_UNIT)),
        ) else {
            continue;
        };
        out.push(Player {
            index: i as u16,
            handle: (u32::from(salt) << 16) | i,
            flags,
            unit,
        });
    }
    out
}

/// The player with this handle's index (the low word).
pub fn index_of(handle: u32) -> u16 {
    handle as u16
}

/// An object, read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Object {
    pub parent: u32,
    pub position: [f32; 3],
}

pub fn object<M: GuestMemory + ?Sized>(m: &M, handle: u32) -> Option<Object> {
    if handle == NONE {
        return None;
    }
    let hdr = m.u32(points::OBJECTS.base).ok()?;
    let data = m.u32(hdr.wrapping_add(ARRAY_DATA)).ok()?;
    let entry = data.wrapping_add(u32::from(handle as u16) * OBJECT_HEADER);
    let obj = m.u32(entry.wrapping_add(OBJECT_ADDRESS)).ok()?;
    if obj == 0 {
        return None;
    }
    Some(Object {
        parent: m.u32(obj.wrapping_add(OBJECT_PARENT)).ok()?,
        position: m.vec3(obj.wrapping_add(OBJECT_POSITION)).ok()?,
    })
}

/// A campaign is running (options mode 1).
pub fn is_campaign<M: GuestMemory + ?Sized>(m: &M) -> bool {
    m.u32(points::GAME_GLOBALS.base)
        .and_then(|g| m.u32(g.wrapping_add(OPTIONS_MODE)))
        .is_ok_and(|mode| mode == CAMPAIGN)
}

/// The scenario's starting-location count.
pub fn start_count<M: GuestMemory + ?Sized>(m: &M) -> u32 {
    m.u32(points::SCENARIO.base)
        .and_then(|s| m.u32(s.wrapping_add(SCENARIO_STARTS)))
        .unwrap_or(0)
}

// ---- decisions ----

/// The location a spawn of player `index` gets when the retail pick
/// answered `picked`: retail's, unless every spot was blocked for a player
/// of index 2 or more, then spot `index % count` (it is moved next to the
/// anchor at once).
pub fn location_fallback(index: u16, picked: u16, count: u32) -> Option<u16> {
    if picked != NO_LOCATION || index < FIRST_EXTRA || count == 0 {
        return None;
    }
    Some((u32::from(index) % count) as u16)
}

/// Whether the spawn of `p` is one this module takes over: a first spawn
/// of a player of index 2 or more.
pub fn takes_spawn(p: &Player) -> bool {
    p.index >= FIRST_EXTRA && p.flags & FIRST_SPAWN != 0
}

/// The anchor the others gather at: player 0's unit when it stands on its
/// own (not in a vehicle), else player 1's. `(unit, position)`.
pub fn anchor<M: GuestMemory + ?Sized>(m: &M, ps: &[Player]) -> Option<(u32, [f32; 3])> {
    ps.iter()
        .filter(|p| p.index < FIRST_EXTRA && p.unit != NONE)
        .find_map(|p| {
            let o = object(m, p.unit)?;
            (o.parent == NONE).then_some((p.unit, o.position))
        })
}

pub(crate) fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// The players of index 2 or more to bring to the anchor at `at`: alive,
/// on foot, and farther than [`LEASH`].
pub fn stragglers<M: GuestMemory + ?Sized>(m: &M, ps: &[Player], at: [f32; 3]) -> Vec<u32> {
    ps.iter()
        .filter(|p| p.index >= FIRST_EXTRA && p.unit != NONE)
        .filter(|p| {
            object(m, p.unit).is_some_and(|o| o.parent == NONE && distance(o.position, at) > LEASH)
        })
        .map(|p| p.handle)
        .collect()
}

/// Whether a teleported object is player 0's or 1's unit: the scripts'
/// `player0` and `player1` (the first two players with a unit).
pub fn is_named_player_unit(ps: &[Player], object: u32) -> bool {
    object != NONE
        && ps
            .iter()
            .filter(|p| p.unit != NONE)
            .take(usize::from(FIRST_EXTRA))
            .any(|p| p.unit == object)
}

// ---- the feature ----

#[derive(Debug, Default)]
pub struct State {
    /// The player whose spawn this module is running (handle).
    spawning: Option<u32>,
    /// Stragglers are to be brought over at the next player update.
    pending: bool,
    /// Between `cinematic_start` and `cinematic_stop`.
    cinematic: bool,
}

pub struct CoopPlayers {
    arm: Arc<Arm>,
    state: Arc<Mutex<State>>,
}

impl CoopPlayers {
    pub fn new(arm: Arc<Arm>) -> CoopPlayers {
        CoopPlayers {
            arm,
            state: Arc::default(),
        }
    }
}

fn lock(s: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    s.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why `coop-players` can't be snapshotted now
/// (docs/runtime/snapshots.md): armed ([`Arm::snapshot_live`]), or a
/// spawn of ours is running. Disarmed, the gather and cinematic flags are
/// read only while armed, and a load starts them fresh as a new session
/// would; the spawn and gather calls themselves are continuations the
/// machine check sees.
pub fn players_live(arm: &Arm, st: &Mutex<State>) -> Option<String> {
    if let Some(why) = arm.snapshot_live() {
        return Some(why);
    }
    lock(st)
        .spawning
        .map(|p| format!("player {}'s spawn is running", index_of(p)))
}

fn say(s: &str) {
    eprintln!("coop-players: {s}");
}

/// Moves `who` next to the anchor: the position into `scratch`, then
/// `PLAYER_PLACE_NEAR`. The number placed.
async fn place(
    cx: &Cx,
    who: &[u32],
    anchor: (u32, [f32; 3]),
    scratch: u32,
) -> Result<usize, EntryError> {
    let mut placed = 0;
    for &p in who {
        cx.write(|m| -> Result<(), engine::memory::MemFault> {
            for (i, v) in anchor.1.iter().enumerate() {
                m.set_f32(scratch + 4 * i as u32, *v)?;
            }
            Ok(())
        })
        .map_err(EntryError::Fault)?;
        let r = cx
            .call(
                GuestCall::new(points::PLAYER_PLACE_NEAR.base)
                    .ebx(p)
                    .esi(scratch)
                    .args([anchor.0]),
            )
            .await?;
        let ok = r.eax & 0xff != 0;
        say(&format!(
            "player {} placed next to the anchor{}",
            index_of(p),
            if ok {
                ""
            } else {
                ": REFUSED (it stays where it spawned)"
            }
        ));
        placed += usize::from(ok);
    }
    Ok(placed)
}

/// `PLAYER_SPAWN` for a first spawn of a player of index 2 or more: the
/// original (with the location fallback armed), then the placement.
pub async fn spawn(
    cx: Cx,
    state: Arc<Mutex<State>>,
    reentry: Arc<AtomicBool>,
    scratch: u32,
) -> Result<Ret, EntryError> {
    let player = cx.arg(0).map_err(EntryError::Fault)?;
    lock(&state).spawning = Some(player);
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::PLAYER_SPAWN.base).args([player]))
        .await;
    reentry.store(false, Ordering::SeqCst);
    lock(&state).spawning = None;
    let eax = r?.eax;
    if eax & 0xff == 0 {
        return Ok(Ret::new(eax, 4));
    }
    let anchor = cx.read(|m| anchor(m, &players(m)));
    match anchor {
        Some(a) => {
            say(&format!(
                "player {} spawned; placing it next to the anchor",
                index_of(player)
            ));
            place(&cx, &[player], a, scratch).await?;
        }
        None => say(&format!(
            "player {} spawned; no anchor on foot, it stays on its starting location",
            index_of(player)
        )),
    }
    Ok(Ret::new(eax, 4))
}

/// `PLAYER_UPDATE` with stragglers pending: bring them over, then the
/// original.
pub async fn gather(cx: Cx, reentry: Arc<AtomicBool>, scratch: u32) -> Result<Ret, EntryError> {
    let update = cx.arg(0).map_err(EntryError::Fault)?;
    let (anchor, who) = cx.read(|m| {
        let ps = players(m);
        match anchor(m, &ps) {
            Some(a) => (Some(a), stragglers(m, &ps, a.1)),
            None => (None, Vec::new()),
        }
    });
    if let Some(a) = anchor
        && !who.is_empty()
    {
        say(&format!(
            "{} player(s) of index 2 or more left behind; bringing them over",
            who.len()
        ));
        place(&cx, &who, a, scratch).await?;
    }
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::PLAYER_UPDATE.base).args([update]))
        .await;
    reentry.store(false, Ordering::SeqCst);
    Ok(Ret::new(r?.eax, 4))
}

/// Every point the feature hooks.
pub static HOOKED: &[&points::Point] = &[
    &points::PLAYER_SPAWN,
    &points::PLAYER_SPAWN_LOCATION,
    &points::PLAYER_UPDATE,
    &points::SCRIPT_TELEPORT,
    &points::BSP_SWITCH,
    &points::CINEMATIC_START,
    &points::CINEMATIC_STOP,
    &points::SAVE_SAFE,
];

impl feature::Feature for CoopPlayers {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            FEATURE,
            "Co-op over Live: players 3 to 16 start next to player 0 and follow the scripts' teleports",
        )
        .doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let scratch = b.guest_alloc(0x10, Permissions::RW)?;

        // The spawn of a third (or later) player: original, then placement.
        let reentry = Arc::new(AtomicBool::new(false));
        {
            let (arm, st, reentry) = (
                Arc::clone(&self.arm),
                Arc::clone(&self.state),
                Arc::clone(&reentry),
            );
            b.raw().before(
                &points::PLAYER_SPAWN,
                "players 3+: spawn next to player 0",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !arm.active() {
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let take = {
                        let m = rt.memory();
                        let handle = m.u32(c.cpu.esp.wrapping_add(4))?;
                        is_campaign(&*m)
                            && players(&*m)
                                .iter()
                                .any(|p| p.handle == handle && takes_spawn(p))
                    };
                    if !take {
                        return Ok(Flow::Continue);
                    }
                    let (st, reentry) = (Arc::clone(&st), Arc::clone(&reentry));
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(spawn(cx, st, reentry, scratch))
                    })
                },
            );
        }
        // Its location: a blocked pick becomes a spot (it moves at once).
        {
            let st = Arc::clone(&self.state);
            b.raw()
                .after(&points::PLAYER_SPAWN_LOCATION, "players 3+: a spot even when blocked", move |c| {
                    let Some(player) = lock(&st).spawning else {
                        return Ok(());
                    };
                    let rt = Arc::clone(c.ctx.runtime());
                    let count = start_count(&*rt.memory());
                    if let Some(spot) = location_fallback(index_of(player), c.cpu.eax as u16, count) {
                        say(&format!(
                            "player {}: every starting location is taken; spawning on {spot} and moving it",
                            index_of(player)
                        ));
                        c.cpu.eax = u32::from(spot);
                    }
                    Ok(())
                });
        }

        // What leaves the others behind.
        {
            let (arm, st) = (Arc::clone(&self.arm), Arc::clone(&self.state));
            b.raw()
                .observe_before(&points::SCRIPT_TELEPORT, "player0/1 teleported", move |o| {
                    if !arm.active() {
                        return;
                    }
                    let m = o.guest();
                    let Ok(object) = m.u32(o.cpu.esp.wrapping_add(4)) else {
                        return;
                    };
                    let ps = players(&*m);
                    if ps.iter().any(|p| p.index >= FIRST_EXTRA)
                        && is_named_player_unit(&ps, object)
                    {
                        lock(&st).pending = true;
                    }
                })
                .optional();
        }
        for (point, what, cinematic) in [
            (&points::BSP_SWITCH, "bsp switch", None),
            (&points::CINEMATIC_START, "cinematic start", Some(true)),
            (&points::CINEMATIC_STOP, "cinematic stop", Some(false)),
        ] {
            let (arm, st) = (Arc::clone(&self.arm), Arc::clone(&self.state));
            b.raw()
                .observe_before(point, what, move |_| {
                    if !arm.active() {
                        return;
                    }
                    let mut s = lock(&st);
                    match cinematic {
                        Some(true) => s.cinematic = true,
                        Some(false) => {
                            s.cinematic = false;
                            s.pending = true;
                        }
                        None => s.pending = true,
                    }
                })
                .optional();
        }

        // Checkpoints: players 2+ who are dead or far do not count.
        {
            let arm = Arc::clone(&self.arm);
            let reentry = Arc::new(AtomicBool::new(false));
            let last = Arc::new(Mutex::new(None));
            b.raw().before(
                &points::SAVE_SAFE,
                "players 3+: only those near count for a checkpoint",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !arm.active() {
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let hidden = {
                        let m = rt.memory();
                        if !is_campaign(&*m) {
                            return Ok(Flow::Continue);
                        }
                        save::not_counted(&*m, &players(&*m))
                    };
                    if hidden.is_empty() {
                        return Ok(Flow::Continue);
                    }
                    if let Some(line) = save::note(&last, &hidden) {
                        eprintln!("{line}");
                    }
                    let reentry = Arc::clone(&reentry);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(save::safe_to_save(cx, hidden, reentry))
                    })
                },
            );
        }

        // Bring them over at the next player update, outside a cinematic.
        {
            let (arm, st) = (Arc::clone(&self.arm), Arc::clone(&self.state));
            let reentry = Arc::new(AtomicBool::new(false));
            b.raw().before(
                &points::PLAYER_UPDATE,
                "players 3+: follow player 0",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !arm.active() {
                        return Ok(Flow::Continue);
                    }
                    {
                        let mut s = lock(&st);
                        if !s.pending || s.cinematic {
                            return Ok(Flow::Continue);
                        }
                        s.pending = false;
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    if !is_campaign(&*rt.memory()) {
                        return Ok(Flow::Continue);
                    }
                    let reentry = Arc::clone(&reentry);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(gather(cx, reentry, scratch))
                    })
                },
            );
        }
        let (arm, st) = (Arc::clone(&self.arm), Arc::clone(&self.state));
        b.snapshot(feature::SnapshotPolicy::forbid_while(move || players_live(&arm, &st)));
        Ok(())
    }
}

/// Adds the feature, armed by `arm` (the co-op over Live hooks' arm).
pub fn install(registry: &mut feature::Registry, arm: Arc<Arm>) -> anyhow::Result<bool> {
    registry
        .add(&CoopPlayers::new(arm))
        .map_err(|e| anyhow::anyhow!("coop-players: {e}"))
}

pub mod save;

#[cfg(test)]
mod tests;
