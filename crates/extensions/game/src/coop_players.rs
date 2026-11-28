//! Co-op with more than two players (docs/live/live-coop-join.md section
//! 13): where players 3 to 16 start, respawn and stand when the level's
//! scripts move only the first two.
//!
//! The engine already spawns, respawns and ends the game over all 16
//! player slots. What holds at two is the levels' data and scripts:
//!
//! * **Level start.** Every campaign scenario has two starting locations
//!   (00a one), and `PLAYER_SPAWN_LOCATION` (`0x14ef00`) skips a spot a
//!   biped stands on. For a first spawn of a player of index 2 or more,
//!   when retail finds no free spot, the pick tries the grid of
//!   [`spots::start_candidates`] (rings around starting locations 0 and 1)
//!   with retail's own free-spot test (`SPAWN_LOCATION_FREE`, `0x23bb70`)
//!   at the spot, and spawns the player there. No spot is nearer than
//!   [`spots::MIN_GAP`] to another player. With no spot on any ring the
//!   spawn fails, and the engine retries it next tick, as retail does.
//! * **Gathers.** A teleport (`SCRIPT_TELEPORT`, `0x29ffb0`) of the
//!   scripts' `player0` or `player1`, a BSP switch, the end of a cinematic,
//!   or a scripts' player entering a trigger volume ([`volumes`]) brings
//!   every player of index 2 or more past [`LEASH`] to a grid spot around
//!   the anchor, each its own. A player 3+ past the leash inside a volume
//!   neither scripts' player is in comes back the same way. Never during a
//!   cinematic.
//! * **Join in progress.** A first spawn once the game is under way
//!   ([`joins_in_progress`]: past [`JOIN_SPAWN_TICKS`], someone alive),
//!   of any index, is a friend who joined the running game (section 14):
//!   retail would put them on a level-start location. The pick puts them
//!   on a grid spot around [`spots::group_anchor`] instead, through the
//!   same moved starting location as the level-start grid.
//! * **Respawn.** Retail's co-op respawn places a dead player next to the
//!   last player in table order; with players 3+ it is placed on a grid
//!   spot around [`spots::group_anchor`] instead.
//! * **Checkpoints** ([`save`]): a majority rule over the squad near the
//!   anchor.
//!
//! The hooks act only while co-op over Live is armed ([`Arm`]), and, but
//! the join-in-progress spawn, only when a player of index 2 or more
//! exists, which happens only when the host's `coop.max_players` let a
//! third console in. They are part of the
//! simulation, so they must run alike on every console: they never read a
//! per-console setting, only the game.
//!
//! The decisions are pure functions over guest memory, tested on a fake
//! guest; the guest calls are async entries run by the engine's test
//! driver.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use hooks::Flow;
use runtime::guest::Permissions;

use crate::coop_probe::Arm;

/// The feature id (`H2_FEATURES=-coop-players` turns it off).
pub const FEATURE: &str = "coop-players";

/// How far (world units) a player of index 2 or more may be from the
/// anchor before a gather brings them over, and how near one counts for a
/// checkpoint. [guess] A world unit is about three metres; 8 keeps a squad
/// in one room and does not chase someone a few steps away.
pub const LEASH: f32 = 8.0;

/// The first player index the scripts do not name.
pub const FIRST_EXTRA: u16 = 2;

/// How far below a level-start spot the free-spot test is asked first: a
/// spot whose capsule is blocked there has ground under it. [guess] that
/// the test sees the structure; spots with no ground found are tried only
/// when none has it.
pub const GROUND_PROBE: f32 = 0.5;

/// The co-op respawn's call of `PLAYER_PLACE_NEAR` (`0x14cffd`, a 5-byte
/// `call rel32`): the return address its placement is known by.
pub const RESPAWN_PLACE_RETURN: u32 = 0x0014_cffd + 5;

/// Guest layouts (docs/live/live-coop-join.md section 13, all read in the
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
    /// Scenario: starting locations (count, then the address of the
    /// first); `0x34` each, the position at `+0` (docs/editor/scenario-editor.md).
    pub const SCENARIO_STARTS: u32 = 0x100;
    pub const START_STRIDE: u32 = 0x34;
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

/// Whether a player of index 2 or more is in the game.
pub fn has_extras(ps: &[Player]) -> bool {
    ps.iter().any(|p| p.index >= FIRST_EXTRA)
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

/// Where a unit is in the world: its own position on foot, its vehicle's
/// (the parent's) when it rides.
pub fn world_position<M: GuestMemory + ?Sized>(m: &M, unit: u32) -> Option<[f32; 3]> {
    let o = object(m, unit)?;
    if o.parent == NONE {
        return Some(o.position);
    }
    object(m, o.parent).map(|v| v.position)
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

/// The scenario's starting locations: the first's address and each
/// position, at most [`MAX_STARTS`]. `None` when they do not read.
pub fn starts<M: GuestMemory + ?Sized>(m: &M) -> Option<Starts> {
    let s = m.u32(points::SCENARIO.base).ok().filter(|&s| s != 0)?;
    let n = m
        .i32(s.wrapping_add(SCENARIO_STARTS))
        .ok()?
        .clamp(0, MAX_STARTS);
    let base = m.u32(s.wrapping_add(SCENARIO_STARTS + 4)).ok()?;
    let at = (0..n as u32)
        .map(|i| m.vec3(base.wrapping_add(i * START_STRIDE)).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(Starts { base, at })
}

/// More starting locations than a scenario holds; a bad count reads no
/// further.
pub const MAX_STARTS: i32 = 256;

#[derive(Clone, Debug, PartialEq)]
pub struct Starts {
    pub base: u32,
    pub at: Vec<[f32; 3]>,
}

// ---- decisions ----

/// Whether the spawn of `p` is one this module takes over: a first spawn
/// of a player of index 2 or more.
pub fn takes_spawn(p: &Player) -> bool {
    p.index >= FIRST_EXTRA && p.flags & FIRST_SPAWN != 0
}

/// Game ticks (30 a second) after which a first spawn is a join in
/// progress, not the level's start: every player in the lobby spawns on
/// the first ticks (a third waits for a free spot, retried each tick).
/// [guess] 5 s covers a level start's waits; a later wait is placed by
/// the squad, which is where the others are by then anyway.
pub const JOIN_SPAWN_TICKS: u32 = 150;

/// The game's tick (`[GAME_TIME]+8`, the state every console restores).
pub fn game_tick<M: GuestMemory + ?Sized>(m: &M) -> Option<u32> {
    let t = m.u32(points::GAME_TIME.base).ok().filter(|&t| t != 0)?;
    m.u32(t.wrapping_add(8)).ok()
}

/// Whether the spawn of `p` is a friend joining the running game
/// (docs/live/live-coop-join.md 14.9.5): a first spawn at tick
/// [`JOIN_SPAWN_TICKS`] or later, with another player alive to stand by
/// ([`spots::group_anchor`]). Any index: a host that started alone has
/// its first friend at index 1.
pub fn joins_in_progress<M: GuestMemory + ?Sized>(m: &M, ps: &[Player], p: &Player) -> bool {
    p.flags & FIRST_SPAWN != 0
        && game_tick(m).is_some_and(|t| t >= JOIN_SPAWN_TICKS)
        && spots::group_anchor(m, ps, p.handle).is_some()
}

/// The scripts' `player0` and `player1`: the first two players with a
/// unit, in table order (`(list_get (players) n)`).
pub fn script_players(ps: &[Player]) -> Vec<Player> {
    ps.iter()
        .filter(|p| p.unit != NONE)
        .take(usize::from(FIRST_EXTRA))
        .copied()
        .collect()
}

/// The anchor a gather comes to: the scripts' `player0` when it stands on
/// its own (not in a vehicle), else their `player1`. `(unit, position)`.
pub fn anchor<M: GuestMemory + ?Sized>(m: &M, ps: &[Player]) -> Option<(u32, [f32; 3])> {
    script_players(ps).iter().find_map(|p| {
        let o = object(m, p.unit)?;
        (o.parent == NONE).then_some((p.unit, o.position))
    })
}

pub fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
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

/// Whether a teleported object is the scripts' `player0` or `player1`.
pub fn is_named_player_unit(ps: &[Player], object: u32) -> bool {
    object != NONE && script_players(ps).iter().any(|p| p.unit == object)
}

// ---- the feature ----

#[derive(Debug, Default)]
pub struct State {
    /// The player whose spawn this module is running (handle).
    spawning: Option<u32>,
    /// A starting location the level-start pick moved for that spawn:
    /// (its address, its own position), put back when the spawn returns.
    moved_start: Option<(u32, [f32; 3])>,
    /// Players (bit per index) whose level-start refusal is logged, so a
    /// spawn retried each tick says it once.
    refused: u16,
    /// The same for a join-in-progress spawn that found no spot by the
    /// squad.
    join_refused: u16,
    /// Stragglers are to be brought over at the next player update.
    pending: bool,
    /// Between `cinematic_start` and `cinematic_stop`.
    cinematic: bool,
    /// The gate volumes the scripts' players stood in at the last player
    /// update; `None` before the first.
    named_volumes: Option<Vec<u16>>,
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
/// spawn of ours is running. Disarmed, the gather, cinematic and volume
/// state is read only while armed, and a load starts it fresh as a new
/// session would; the spawn and gather calls themselves are continuations
/// the machine check sees.
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

async fn place_near(
    cx: &Cx,
    player: u32,
    at: [f32; 3],
    anchor: u32,
    scratch: u32,
) -> Result<bool, EntryError> {
    cx.write(|m| -> Result<(), guest_layout::memory::MemFault> {
        for (i, v) in at.iter().enumerate() {
            m.set_f32(scratch + 4 * i as u32, *v)?;
        }
        Ok(())
    })?;
    let r = cx
        .call(
            GuestCall::new(points::PLAYER_PLACE_NEAR.base)
                .ebx(player)
                .esi(scratch)
                .args([anchor]),
        )
        .await?;
    Ok(r.eax & 0xff != 0)
}

/// Moves each of `who` to its own grid spot around the anchor
/// ([`spots::spots_around`], clear of every other player, read afresh
/// after each placement) with `PLAYER_PLACE_NEAR`, the next spot when the
/// game refuses one. The number placed.
async fn place(
    cx: &Cx,
    who: &[u32],
    anchor: (u32, [f32; 3]),
    scratch: u32,
) -> Result<usize, EntryError> {
    let mut placed = 0;
    for &p in who {
        let spots =
            cx.read(|m| spots::spots_around(anchor.1, &spots::unit_positions(m, &players(m), p)));
        let mut ring = None;
        for &(r, at) in &spots {
            if place_near(cx, p, at, anchor.0, scratch).await? {
                ring = Some(r);
                break;
            }
        }
        match ring {
            Some(r) => say(&format!(
                "player {} placed next to the anchor (ring {r})",
                index_of(p)
            )),
            None => say(&format!(
                "player {} placed next to the anchor: REFUSED ({} spots on rings 1 to {}; it stays where it is)",
                index_of(p),
                spots.len(),
                spots::RINGS
            )),
        }
        placed += usize::from(ring.is_some());
    }
    Ok(placed)
}

/// `PLAYER_SPAWN` for a first spawn of a player of index 2 or more: the
/// original with the level-start pick ([`pick`]) armed, then the starting
/// location it moved put back.
pub async fn spawn(
    cx: Cx,
    state: Arc<Mutex<State>>,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    let player = cx.arg(0).map_err(EntryError::Fault)?;
    lock(&state).spawning = Some(player);
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::PLAYER_SPAWN.base).args([player]))
        .await;
    reentry.store(false, Ordering::SeqCst);
    let moved = {
        let mut s = lock(&state);
        s.spawning = None;
        s.moved_start.take()
    };
    if let Some((at, pos)) = moved {
        cx.write(|m| -> Result<(), guest_layout::memory::MemFault> {
            for (i, v) in pos.iter().enumerate() {
                m.set_f32(at + 4 * i as u32, *v)?;
            }
            Ok(())
        })?;
    }
    let eax = r?.eax;
    if eax & 0xff != 0 {
        let mut s = lock(&state);
        s.refused &= !(1 << index_of(player));
        s.join_refused &= !(1 << index_of(player));
    }
    Ok(Ret::new(eax, 4))
}

fn set_pos(m: &mut dyn GuestMemory, at: u32, p: [f32; 3]) -> Result<(), guest_layout::memory::MemFault> {
    for (i, v) in p.iter().enumerate() {
        m.set_f32(at + 4 * i as u32, *v)?;
    }
    Ok(())
}

async fn spot_free(
    cx: &Cx,
    entry: u32,
    at: [f32; 3],
    unit: u32,
    player: u32,
) -> Result<bool, EntryError> {
    cx.write(|m| set_pos(m, entry, at))?;
    let r = cx
        .call(
            GuestCall::new(points::SPAWN_LOCATION_FREE.base)
                .ecx(entry)
                .eax(unit)
                .edi(player),
        )
        .await?;
    Ok(r.eax & 0xff != 0)
}

/// `PLAYER_SPAWN_LOCATION` inside [`spawn`]: retail's pick, and when it
/// found every spot taken, the first level-start grid spot
/// ([`spots::start_candidates`]) clear of every player that retail's
/// free-spot test passes, ground under it first. The starting location the
/// spot belongs to is moved to the spot for the spawn (put back by
/// [`spawn`]) and is the answer. No spot: `0xffff`, and the spawn waits a
/// tick as retail.
pub async fn pick(
    cx: Cx,
    state: Arc<Mutex<State>>,
    reentry: Arc<AtomicBool>,
) -> Result<Ret, EntryError> {
    let player = cx.entry().ecx;
    let (unit, a, b) = (cx.arg(0)?, cx.arg(1)?, cx.arg(2)?);
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(
            GuestCall::new(points::PLAYER_SPAWN_LOCATION.base)
                .ecx(player)
                .args([unit, a, b]),
        )
        .await;
    reentry.store(false, Ordering::SeqCst);
    let eax = r?.eax;
    let index = index_of(player);
    let join = cx.read(|m| {
        let ps = players(m);
        ps.iter()
            .find(|p| p.handle == player)
            .is_some_and(|p| joins_in_progress(m, &ps, p))
    });
    if join && let Some(ret) = join_pick(&cx, &state, player, unit, eax).await? {
        return Ok(ret);
    }
    if eax as u16 != NO_LOCATION {
        say(&format!(
            "player {index} placed on starting location {} (free)",
            eax as u16
        ));
        return Ok(Ret::new(eax, 12));
    }
    let (starts, candidates) = cx.read(|m| {
        let Some(st) = starts(m) else {
            return (None, Vec::new());
        };
        let units = spots::unit_positions(m, &players(m), player);
        let c = spots::start_candidates(&st.at)
            .into_iter()
            .filter(|c| spots::clear_of(c.at, &units))
            .collect();
        (Some(st), c)
    });
    let Some(starts) = starts.filter(|s| !s.at.is_empty()) else {
        return Ok(Ret::new(eax, 12));
    };
    let entry = |s: u16| starts.base + u32::from(s) * START_STRIDE;
    let mut found = None;
    'passes: for ground in [true, false] {
        for c in &candidates {
            if ground {
                let below = [c.at[0], c.at[1], c.at[2] - GROUND_PROBE];
                if spot_free(&cx, entry(c.start), below, unit, player).await? {
                    continue;
                }
            }
            if spot_free(&cx, entry(c.start), c.at, unit, player).await? {
                found = Some(*c);
                break 'passes;
            }
        }
    }
    // Every starting location tried goes back but the one spawned on.
    let tried: Vec<u16> = (0..starts.at.len().min(2) as u16).collect();
    cx.write(|m| -> Result<(), guest_layout::memory::MemFault> {
        for &s in &tried {
            if found.is_none_or(|c| c.start != s) {
                set_pos(m, entry(s), starts.at[usize::from(s)])?;
            }
        }
        Ok(())
    })?;
    match found {
        Some(c) => {
            lock(&state).moved_start = Some((entry(c.start), starts.at[usize::from(c.start)]));
            say(&format!(
                "player {index} placed at starting location {}, ring {} ({:.2} {:.2} {:.2})",
                c.start, c.ring, c.at[0], c.at[1], c.at[2]
            ));
            Ok(Ret::new((eax & 0xffff_0000) | u32::from(c.start), 12))
        }
        None => {
            let first = {
                let mut s = lock(&state);
                let first = s.refused & (1 << index) == 0;
                s.refused |= 1 << index;
                first
            };
            if first {
                say(&format!(
                    "player {index}: REFUSED: no free spot on rings 1 to {} of starting locations 0 and 1 ({} clear of the others); it waits and retries each tick",
                    spots::RINGS,
                    candidates.len()
                ));
            }
            Ok(Ret::new(eax, 12))
        }
    }
}

/// The pick for a join in progress ([`joins_in_progress`]): the first grid
/// spot around [`spots::group_anchor`] clear of every player that retail's
/// free-spot test passes, ground under it first, with starting location 0
/// moved there for the spawn (put back by [`spawn`]); the answer is
/// location 0. `None` when no spot passes (starting location 0 back as it
/// was): the pick goes on as for any spawn, and says so once.
async fn join_pick(
    cx: &Cx,
    state: &Mutex<State>,
    player: u32,
    unit: u32,
    eax: u32,
) -> Result<Option<Ret>, EntryError> {
    let (starts, anchor, spots) = cx.read(|m| {
        let ps = players(m);
        let a = spots::group_anchor(m, &ps, player).map(|(u, at)| {
            let i = ps.iter().find(|p| p.unit == u).map_or(0, |p| p.index);
            (i, at)
        });
        let around = a.map_or_else(Vec::new, |(_, at)| {
            spots::spots_around(at, &spots::unit_positions(m, &ps, player))
        });
        (starts(m), a, around)
    });
    let (Some(starts), Some((by, _))) = (starts.filter(|s| !s.at.is_empty()), anchor) else {
        return Ok(None);
    };
    let index = index_of(player);
    let entry = starts.base;
    let mut found = None;
    'passes: for ground in [true, false] {
        for &(ring, at) in &spots {
            if ground {
                let below = [at[0], at[1], at[2] - GROUND_PROBE];
                if spot_free(cx, entry, below, unit, player).await? {
                    continue;
                }
            }
            if spot_free(cx, entry, at, unit, player).await? {
                found = Some((ring, at));
                break 'passes;
            }
        }
    }
    match found {
        Some((ring, at)) => {
            cx.write(|m| set_pos(m, entry, at))?;
            lock(state).moved_start = Some((entry, starts.at[0]));
            say(&format!(
                "player {index} joins in progress: placed next to player {by} (the nearest the squad's middle), ring {ring} ({:.2} {:.2} {:.2})",
                at[0], at[1], at[2]
            ));
            Ok(Some(Ret::new(eax & 0xffff_0000, 12)))
        }
        None => {
            cx.write(|m| set_pos(m, entry, starts.at[0]))?;
            let first = {
                let mut s = lock(state);
                let first = s.join_refused & (1 << index) == 0;
                s.join_refused |= 1 << index;
                first
            };
            if first {
                say(&format!(
                    "player {index} joins in progress: REFUSED: no free spot on rings 1 to {} next to player {by} ({} clear of the others); it spawns where retail puts it",
                    spots::RINGS,
                    spots.len()
                ));
            }
            Ok(None)
        }
    }
}

/// `PLAYER_UPDATE` with a gather due: bring over the stragglers (`all`)
/// and the players past the leash inside a volume the scripts' players are
/// not in, then the original.
pub async fn gather(
    cx: Cx,
    reentry: Arc<AtomicBool>,
    scratch: u32,
    all: bool,
) -> Result<Ret, EntryError> {
    let update = cx.arg(0).map_err(EntryError::Fault)?;
    let (anchor, behind, ahead) = cx.read(|m| {
        let ps = players(m);
        match anchor(m, &ps) {
            Some(a) => {
                let behind = if all {
                    stragglers(m, &ps, a.1)
                } else {
                    Vec::new()
                };
                let ahead = volumes::ahead(m, &ps, &volumes::gates(m), a.1);
                (Some(a), behind, ahead)
            }
            None => (None, Vec::new(), Vec::new()),
        }
    });
    if let Some(a) = anchor {
        if !behind.is_empty() {
            say(&format!(
                "{} player(s) of index 2 or more left behind; bringing them over",
                behind.len()
            ));
        }
        let ahead: Vec<u32> = ahead.into_iter().filter(|p| !behind.contains(p)).collect();
        if !ahead.is_empty() {
            say(&format!(
                "{} player(s) of index 2 or more past the leash in a trigger volume player0 and player1 are not in; bringing them back",
                ahead.len()
            ));
        }
        let mut who = behind;
        who.extend(ahead);
        who.sort_by_key(|&h| index_of(h));
        if !who.is_empty() {
            place(&cx, &who, a, scratch).await?;
        }
    }
    reentry.store(true, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::PLAYER_UPDATE.base).args([update]))
        .await;
    reentry.store(false, Ordering::SeqCst);
    Ok(Ret::new(r?.eax, 4))
}

/// The co-op respawn's `PLAYER_PLACE_NEAR` with players 3+: a grid spot
/// around the group's anchor ([`spots::group_anchor`]); retail's own call
/// when there is none or every spot is refused.
pub async fn respawn_place(
    cx: Cx,
    reentry: Arc<AtomicBool>,
    scratch: u32,
) -> Result<Ret, EntryError> {
    let e = cx.entry();
    let (player, retail_at, retail_anchor) = (e.ebx, e.esi, cx.arg(0)?);
    let (anchor, spots) = cx.read(|m| {
        let ps = players(m);
        match spots::group_anchor(m, &ps, player) {
            Some(a) => (
                Some(a),
                spots::spots_around(a.1, &spots::unit_positions(m, &ps, player)),
            ),
            None => (None, Vec::new()),
        }
    });
    reentry.store(true, Ordering::SeqCst);
    let out = async {
        if let Some(a) = anchor {
            for &(r, at) in &spots {
                if place_near(&cx, player, at, a.0, scratch).await? {
                    say(&format!(
                        "player {} respawned next to player {} (the nearest the squad's middle), ring {r}",
                        index_of(player),
                        cx.read(|m| players(m).iter().find(|p| p.unit == a.0).map_or(0, |p| p.index))
                    ));
                    return Ok::<u32, EntryError>(1);
                }
            }
        }
        let r = cx
            .call(
                GuestCall::new(points::PLAYER_PLACE_NEAR.base)
                    .ebx(player)
                    .esi(retail_at)
                    .args([retail_anchor]),
            )
            .await?;
        Ok(r.eax)
    }
    .await;
    reentry.store(false, Ordering::SeqCst);
    Ok(Ret::new(out?, 4))
}

/// Every point the feature hooks.
pub static HOOKED: &[&points::Point] = &[
    &points::PLAYER_SPAWN,
    &points::PLAYER_SPAWN_LOCATION,
    &points::PLAYER_UPDATE,
    &points::PLAYER_PLACE_NEAR,
    &points::SCRIPT_TELEPORT,
    &points::BSP_SWITCH,
    &points::CINEMATIC_START,
    &points::CINEMATIC_STOP,
    &points::SAVE_SAFE,
];

impl hooks::Feature for CoopPlayers {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Co-op over Live: players 3 to 16 start on spots around the starting locations, follow the scripts' players and count for checkpoints by majority",
        )
        .doc("docs/live/live-coop-join.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // +0 a gather's spot, +0x10 a respawn's.
        let scratch = b.guest_alloc(0x20, Permissions::RW)?;

        // The first spawn of a third (or later) player.
        {
            let reentry = Arc::new(AtomicBool::new(false));
            let (arm, st, reentry) = (
                Arc::clone(&self.arm),
                Arc::clone(&self.state),
                Arc::clone(&reentry),
            );
            b.raw().before(
                &points::PLAYER_SPAWN,
                "players 3+: spawn on a free spot",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !arm.active() {
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let take = {
                        let m = rt.memory();
                        let handle = m.u32(c.cpu.esp.wrapping_add(4))?;
                        let ps = players(&*m);
                        is_campaign(&*m)
                            && ps.iter().any(|p| {
                                p.handle == handle
                                    && (takes_spawn(p) || joins_in_progress(&*m, &ps, p))
                            })
                    };
                    if !take {
                        return Ok(Flow::Continue);
                    }
                    let (st, reentry) = (Arc::clone(&st), Arc::clone(&reentry));
                    c.answer_async(move |cx| -> EntryFuture { Box::pin(spawn(cx, st, reentry)) })
                },
            );
        }
        // Its location: retail's, else a grid spot.
        {
            let st = Arc::clone(&self.state);
            let reentry = Arc::new(AtomicBool::new(false));
            b.raw().before(
                &points::PLAYER_SPAWN_LOCATION,
                "players 3+: a grid spot when the starts are taken",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || lock(&st).spawning.is_none() {
                        return Ok(Flow::Continue);
                    }
                    let (st, reentry) = (Arc::clone(&st), Arc::clone(&reentry));
                    c.answer_async(move |cx| -> EntryFuture { Box::pin(pick(cx, st, reentry)) })
                },
            );
        }
        // The co-op respawn's placement.
        {
            let arm = Arc::clone(&self.arm);
            let reentry = Arc::new(AtomicBool::new(false));
            b.raw().before(
                &points::PLAYER_PLACE_NEAR,
                "players 3+: respawn by the squad's middle",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !arm.active() {
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let take = {
                        let m = rt.memory();
                        m.u32(c.cpu.esp)? == RESPAWN_PLACE_RETURN
                            && is_campaign(&*m)
                            && has_extras(&players(&*m))
                    };
                    if !take {
                        return Ok(Flow::Continue);
                    }
                    let reentry = Arc::clone(&reentry);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(respawn_place(cx, reentry, scratch + 0x10))
                    })
                },
            );
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
                    if has_extras(&ps) && is_named_player_unit(&ps, object) {
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

        // Checkpoints: a majority of the squad near the anchor.
        {
            let arm = Arc::clone(&self.arm);
            let reentry = Arc::new(AtomicBool::new(false));
            let last = Arc::new(Mutex::new(None));
            let verdict = Arc::new(Mutex::new(None));
            b.raw().before(
                &points::SAVE_SAFE,
                "players 3+: a checkpoint by the squad's majority",
                move |c| {
                    if reentry.load(Ordering::SeqCst) || !arm.active() {
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let squad = {
                        let m = rt.memory();
                        if !is_campaign(&*m) {
                            return Ok(Flow::Continue);
                        }
                        save::squad(&*m, &players(&*m))
                    };
                    let Some(squad) = squad else {
                        return Ok(Flow::Continue);
                    };
                    if let Some(line) = save::note(&last, &squad.left_out) {
                        eprintln!("{line}");
                    }
                    let (reentry, verdict) = (Arc::clone(&reentry), Arc::clone(&verdict));
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(save::safe_to_save(cx, squad, reentry, verdict))
                    })
                },
            );
        }

        // Each player update: the volume gates, then a gather when one is
        // due, outside a cinematic.
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
                    let rt = Arc::clone(c.ctx.runtime());
                    let all = {
                        let m = rt.memory();
                        if !is_campaign(&*m) {
                            return Ok(Flow::Continue);
                        }
                        let ps = players(&*m);
                        if !has_extras(&ps) {
                            return Ok(Flow::Continue);
                        }
                        let vols = volumes::gates(&*m);
                        let now = volumes::named_inside(&*m, &ps, &vols);
                        let mut s = lock(&st);
                        if volumes::entered(s.named_volumes.as_deref(), &now) {
                            s.pending = true;
                        }
                        s.named_volumes = Some(now);
                        if s.cinematic {
                            return Ok(Flow::Continue);
                        }
                        let ahead = anchor(&*m, &ps)
                            .is_some_and(|a| !volumes::ahead(&*m, &ps, &vols, a.1).is_empty());
                        let all = std::mem::take(&mut s.pending);
                        if !all && !ahead {
                            return Ok(Flow::Continue);
                        }
                        all
                    };
                    let reentry = Arc::clone(&reentry);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(gather(cx, reentry, scratch, all))
                    })
                },
            );
        }
        let (arm, st) = (Arc::clone(&self.arm), Arc::clone(&self.state));
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            players_live(&arm, &st)
        }));
        Ok(())
    }
}

/// Adds the feature, armed by `arm` (the co-op over Live hooks' arm).
pub fn install(registry: &mut hooks::Registry, arm: Arc<Arm>) -> anyhow::Result<bool> {
    registry
        .add(&CoopPlayers::new(arm))
        .map_err(|e| anyhow::anyhow!("coop-players: {e}"))
}

pub mod save;
pub mod spots;
pub mod volumes;

#[cfg(test)]
mod tests;
