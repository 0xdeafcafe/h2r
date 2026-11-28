//! The player's pickup scan `1508e0` (`PLAYER_PICKUP_SCAN`, `ret 4`, the
//! argument a player index) with its callees in Rust
//! (docs/engine/sim-rewrite.md, "Seats and rules (phase 7)"; the pattern of
//! [`crate::sim_seats::respawn`]). `players_after` calls it for each player
//! once the BSP switch volumes are done.
//!
//! **What the scan does, and what the kernel covers.**
//!
//! | Step | Guest | Kernel |
//! |---|---|---|
//! | the player has no unit (`+0x2c` is `-1`) | out | yes, no reads after the unit slot |
//! | the unit's object has a parent (`+0x14`) | out | yes |
//! | the object query | `bb050`, with the cluster flood `14a5b0`, its portal test `14a370`, the polygon test `23a160` and the list heads `b8940` and `b8a10` | `engine::sim_collision::plan_visit_at`, the kernel entry H runs on, planned with the arguments the scan pushes (all lists, types `0x89c`, the unit's location and centre, radius `[unit + 0x3c] + [0x44ae90]`, capacity 16) |
//! | for each handle, in order, type `2` or `3` | `150e30` (a pickup candidate) | [`candidate_exits`], the three exits before its first call |
//! | for each handle, type `7` or `11` | the link walk from `+0x10` along `+0xc`, appended while the list holds fewer than 16 | yes |
//!
//! The visit leaves the flood's epoch, the clusters' and objects' marks, the
//! visit generation and the two active bytes
//! ([`crate::sim_collision::visit_writes`]); nothing else is written, and
//! the frame's output list is dead stack. `eax`, `ecx` and `edx` come back
//! as the scan leaves them from its callees' scratch and are not
//! reproduced: its caller reloads all three before using them. The
//! callee-saved registers and `esp` are reproduced and compared.
//!
//! **What it refuses, by name** ([`Decline`]): named, counted errors
//! (logged the first time each map) that the lifted body still runs, with
//! nothing written:
//!
//! * `visit`: the collision kernel cannot plan the query (its own `Bail`:
//!   an x87 or SSE state it does not reproduce, an alias, a graph or list
//!   it does not model).
//! * `candidate`: a type 2 or 3 handle is in the list, so the guest would
//!   call `150e30`. That call is observable: the Halo 1 prompt feature
//!   (`extensions/game`, `h1_hud_text`) has a `before` hook on it that
//!   records the in-range powerup swap for every call, early exit or not.
//!   [`CANDIDATES`] is [`Candidates::Observed`] while that hook exists.
//!   With [`Candidates::Port`] the kernel instead takes the early exits and
//!   refuses `pickup` for a candidate that passes them.
//! * `pickup`: a candidate that is not an early exit of `150e30`. Past
//!   them it loops the unit's four seats through `ffc60`, posts events
//!   (`e6900`, `191ec4`) and calls `badc0`, `ccba0`, `ccb80`, `138880`,
//!   `151ea0`, `cd7b0`, `150dd0`, `a8f10` and `15e8e0`, none ported.
//! * `fault`: a read of the scan's inputs faulted.
//!
//! The point is therefore **not** `replaces_guest` until those are ported
//! ([`hooks::HookRef::replaces_guest`]). Every other call the kernel
//! answers; the differential test compares all of RAM against the lifted
//! body.

use crate::points;
use crate::sim_collision::{CollisionWorld, plan_visit_at, visit_writes};
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest, GuestFault};
use std::sync::atomic::{AtomicU64, Ordering};

/// The stack words `ret` pops besides the return address.
pub const ARGS: u32 = 4;

const PLAYERS: u32 = 0x004e_8c24;
const OBJECTS: u32 = 0x004e_0300;
/// The float the scan adds to the unit's radius.
const REACH: u32 = 0x0044_ae90;
const STRIDE: u32 = 0x21c;
/// The object types the scan asks the query for (2, 3, 4, 7 and 11).
const TYPES: u32 = 0x89c;
/// The frame's output list: sixteen handles.
const LIST: usize = 16;
/// Where the scan's frame puts the query's output, and the query's entry
/// `esp`, below the scan's own entry `esp`.
const LIST_BELOW: u32 = 0x40;
const QUERY_BELOW: u32 = 0x6c;

/// Whether a type 2 or 3 handle sends the scan to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Candidates {
    /// Every `150e30` call has an observer: refuse the list.
    Observed,
    /// Take `150e30`'s early exits; refuse a candidate that passes them.
    Port,
}

/// What the installed arm runs with: the prompt feature's `before` hook on
/// `PLAYER_PICKUP_CANDIDATE` sees every call.
pub const CANDIDATES: Candidates = Candidates::Observed;

/// Why a call was left to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    Fault,
    Visit,
    Candidate,
    Pickup,
}

impl Decline {
    pub const ALL: [Decline; 4] = [Decline::Fault, Decline::Visit, Decline::Candidate, Decline::Pickup];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Fault => "fault",
            Decline::Visit => "visit",
            Decline::Candidate => "candidate",
            Decline::Pickup => "pickup",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The arm's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub answered: u64,
    /// Calls the kernel cannot answer yet, by [`Decline::ALL`] order:
    /// named errors, each logged once a map, that the lifted body still
    /// runs.
    pub declined: [u64; 4],
}

impl Status {
    pub fn declined_total(&self) -> u64 {
        self.declined.iter().sum()
    }
}

static ANSWERED: AtomicU64 = AtomicU64::new(0);
static DECLINED: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status { answered: read(&ANSWERED), declined: std::array::from_fn(|i| read(&DECLINED[i])) }
}

/// Clear the counters. Called at map load, so each map logs its first
/// error of each kind.
pub fn reset() {
    ANSWERED.store(0, Ordering::Relaxed);
    for c in &DECLINED {
        c.store(0, Ordering::Relaxed);
    }
}

/// Count a call the kernel cannot answer, and say so the first time each
/// map: it is an unported path, not a fallback.
fn decline(why: Decline) -> Step {
    if DECLINED[why.index()].fetch_add(1, Ordering::Relaxed) == 0 {
        eprintln!("sim: seats player-objects: unported path `{}`; the lifted body runs it (counted in sim.arms)", why.name());
    }
    Step::Guest
}

fn fault<T>(r: Result<T, GuestFault>) -> Result<T, Decline> {
    r.map_err(|_| Decline::Fault)
}

fn object_at(g: &Guest, table: u32, handle: u32) -> Result<u32, Decline> {
    fault(g.u32(table.wrapping_add((handle & 0xffff).wrapping_mul(12)).wrapping_add(8)))
}

fn player_at(g: &Guest, index: u32) -> Result<u32, Decline> {
    let players = fault(g.u32(PLAYERS))?;
    let data = fault(g.u32(players.wrapping_add(0x44)))?;
    Ok((index & 0xffff).wrapping_mul(STRIDE).wrapping_add(data))
}

fn float(g: &Guest, at: u32) -> Result<f32, Decline> {
    fault(g.u32(at)).map(f32::from_bits)
}

/// `150e30 (player: stack 0, object: stack 1) ret 8`: whether the call
/// leaves through one of the three exits before its first call, with
/// nothing written. `index` is the player's slot, `handle` the object.
///
/// 1. the player has no unit;
/// 2. the object has a parent that is neither a machine (type 7) nor a
///    crate (11), or its `+0x14c` is the player's unit;
/// 3. the object's centre is not within the unit's radius plus
///    `[0x44ae90]` (squared distance summed z, x, y; the compare is `jbe`,
///    so a NaN leaves).
pub fn candidate_exits(g: &Guest, index: u32, handle: u32) -> Result<bool, Decline> {
    let player = player_at(g, index)?;
    let unit = fault(g.u32(player.wrapping_add(0x2c)))?;
    if unit == u32::MAX {
        return Ok(true);
    }
    let pool = fault(g.u32(OBJECTS))?;
    let table = fault(g.u32(pool.wrapping_add(0x44)))?;
    let unit_object = object_at(g, table, unit)?;
    let object = object_at(g, table, handle)?;
    let parent = fault(g.u32(object.wrapping_add(0x14)))?;
    if parent != u32::MAX {
        // `movzx ecx, byte [data + (parent & 0xffff) * 12 + 3]`.
        let kind = fault(g.u8(table.wrapping_add((parent & 0xffff).wrapping_mul(12)).wrapping_add(3)))?;
        if kind != 0xb && kind != 7 {
            return Ok(true);
        }
    }
    if fault(g.u32(object.wrapping_add(0x14c)))? == unit {
        return Ok(true);
    }
    let dz = float(g, object.wrapping_add(0x38))? - float(g, unit_object.wrapping_add(0x38))?;
    let dx = float(g, object.wrapping_add(0x30))? - float(g, unit_object.wrapping_add(0x30))?;
    let dy = float(g, object.wrapping_add(0x34))? - float(g, unit_object.wrapping_add(0x34))?;
    let reach = float(g, unit_object.wrapping_add(0x3c))? + float(g, REACH)?;
    let distance = dz * dz + dx * dx + dy * dy;
    // `comiss reach², distance²; jbe exit`.
    Ok(!(reach * reach > distance))
}

/// What the kernel predicts for a call: the words the query leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// (address, value, width in bytes), from [`visit_writes`]; empty when
    /// the scan leaves before the query.
    pub writes: Vec<(u32, u32, u8)>,
}

/// The whole call from the registers at its entry (`esp` at the return
/// address, the player index above it).
pub fn kernel(g: &mut Guest, cpu: &CpuAbi, world: &CollisionWorld, candidates: Candidates) -> Result<Plan, Decline> {
    let none = Plan { writes: Vec::new() };
    let index = fault(g.u32(cpu.esp.wrapping_add(4)))?;
    let player = player_at(g, index)?;
    let unit = fault(g.u32(player.wrapping_add(0x2c)))?;
    if unit == u32::MAX {
        return Ok(none);
    }
    let pool = fault(g.u32(OBJECTS))?;
    let table = fault(g.u32(pool.wrapping_add(0x44)))?;
    let body = object_at(g, table, unit)?;
    if fault(g.u32(body.wrapping_add(0x14)))? != u32::MAX {
        return Ok(none);
    }
    let radius = (float(g, body.wrapping_add(0x3c))? + float(g, REACH)?).to_bits();
    // The arguments the scan pushes: lists 0 (all), the types, the unit's
    // location block and centre, the radius, the frame's list, capacity.
    let args = [
        0,
        TYPES,
        body.wrapping_add(0x28),
        body.wrapping_add(0x30),
        radius,
        cpu.esp.wrapping_sub(LIST_BELOW),
        LIST as u32,
    ];
    let plan = plan_visit_at(cpu, g, world, cpu.esp.wrapping_sub(QUERY_BELOW), args).map_err(|_| Decline::Visit)?;
    let mut list: Vec<u32> = plan.visit.handles.iter().copied().take(LIST).collect();
    let mut at = 0;
    while at < list.len() {
        let handle = list[at];
        let object = object_at(g, table, handle)?;
        let kind = fault(g.u8(object.wrapping_add(0xaa)))?;
        if (2..=3).contains(&(kind as i8)) {
            match candidates {
                Candidates::Observed => return Err(Decline::Candidate),
                Candidates::Port => {
                    if !candidate_exits(g, index, handle)? {
                        return Err(Decline::Pickup);
                    }
                }
            }
        }
        if kind == 0xb || kind == 7 {
            let mut link = fault(g.u32(object.wrapping_add(0x10)))?;
            while link != u32::MAX && list.len() < LIST {
                list.push(link);
                let child = object_at(g, table, link)?;
                link = fault(g.u32(child.wrapping_add(0xc)))?;
            }
        }
        at += 1;
    }
    Ok(Plan { writes: visit_writes(&plan) })
}

/// One call's prediction: the CPU as the return leaves the registers the
/// scan preserves, and the words it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prediction {
    pub cpu: CpuAbi,
    pub writes: Vec<(u32, u32, u8)>,
}

impl Prediction {
    /// The first preserved register the guest's return differs from the
    /// prediction in.
    pub fn differs(&self, cpu: &CpuAbi) -> Option<&'static str> {
        let p = &self.cpu;
        [
            ("ebx", p.ebx, cpu.ebx),
            ("esp", p.esp, cpu.esp),
            ("ebp", p.ebp, cpu.ebp),
            ("esi", p.esi, cpu.esi),
            ("edi", p.edi, cpu.edi),
        ]
        .iter()
        .find(|(_, a, b)| a != b)
        .map(|(name, ..)| *name)
    }

    /// The first predicted word the guest's memory does not hold.
    pub fn differs_in(&self, g: &Guest) -> Option<u32> {
        self.writes.iter().find_map(|&(at, value, width)| {
            let held = if width == 1 { g.u8(at).map(u32::from) } else { g.u32(at) };
            (held.ok() != Some(value)).then_some(at)
        })
    }
}

/// What [`enter`] did with a call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The kernel answered; the caller returns to `target`.
    Answered { target: u32 },
    /// An unported path: a counted, logged [`Decline`]; the lifted body runs it.
    Guest,
}

/// The kernel's prediction for the call `cpu` is entering.
pub fn predict(cpu: &CpuAbi, g: &mut Guest, world: &CollisionWorld, candidates: Candidates) -> Result<Prediction, Decline> {
    let plan = kernel(g, cpu, world, candidates)?;
    let mut out = *cpu;
    out.esp = cpu.esp.wrapping_add(4 + ARGS);
    out.eflags_defined &= !ARITH;
    Ok(Prediction { cpu: out, writes: plan.writes })
}

/// Put `prediction` in place of the call: write its words and return the
/// return address.
pub fn apply(prediction: &Prediction, cpu: &mut CpuAbi, g: &mut Guest) -> Result<u32, GuestFault> {
    let target = g.u32(cpu.esp)?;
    for &(at, value, width) in &prediction.writes {
        let bytes = value.to_le_bytes();
        g.write_bytes(at, &bytes[..usize::from(width)], 1)?;
    }
    let eip = cpu.eip;
    *cpu = prediction.cpu;
    cpu.eip = eip;
    Ok(target)
}

/// At the entry: the kernel answers, or the call is a counted [`Decline`].
pub fn enter(cpu: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld) -> Step {
    enter_with(cpu, g, world, CANDIDATES)
}

fn enter_with(cpu: &mut CpuAbi, g: &mut Guest, world: &CollisionWorld, candidates: Candidates) -> Step {
    let prediction = match predict(cpu, g, world, candidates) {
        Ok(p) => p,
        Err(why) => return decline(why),
    };
    match apply(&prediction, cpu, g) {
        Ok(target) => {
            ANSWERED.fetch_add(1, Ordering::Relaxed);
            Step::Answered { target }
        }
        Err(_) => decline(Decline::Fault),
    }
}

/// The point the arm is installed at.
pub fn point() -> &'static crate::points::Point {
    &points::PLAYER_PICKUP_SCAN
}
