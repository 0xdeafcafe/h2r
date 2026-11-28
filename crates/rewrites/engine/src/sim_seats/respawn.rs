//! The respawn gate `15dd40` (`PLAYER_RESPAWN_READY`) as a shadow arm
//! (docs/engine/sim-rewrite.md, "Seats and rules (phase 7)"; the pattern of
//! [`crate::sim_seats::aim::arm`]): `eax` is a player's slot, the answer
//! `al` says whether `players_before` may spawn it now.
//!
//! **What the kernel covers.** The body and its three callees that call
//! nothing, `158a50` (the player can take part: flags `+2` bit 0 set and bit 1
//! clear, `+0xc0` not `-1`, then either a unit slot `+0x2c` held or no
//! lives-out hold) with its `15db30` (out of lives: a positive lives setting
//! `[[4e6948] + 0x1b8]`, no unit, lives `+0x1ac` zero) and `15b2f0` (a local
//! user is mapped, `[game + 0x6c] == 1`, and the machine is a client or the
//! game's `+0xc04` is 1), then the respawn timer `+0x164`: a negative timer
//! answers no, a timer of zero answers yes unless the staggered check (the
//! ticks since `[game + 0x70]` above 3 and their remainder by 32 differing
//! from the slot's) says wait. None of these writes memory: the answer is
//! `eax` (and `ecx`, `edx` as the guest leaves them), `esp` and the popped
//! return.
//!
//! **What it refuses, by name.** A timer above zero ([`Decline::Countdown`])
//! is the countdown: the body decrements the timer, writes the fade at
//! `+0x170` and calls `a7840` (the entity dirty mark, a tail call into the
//! network entity queue `b58c0`) and, on the ticks at one, two and three
//! seconds, the death event `19eb90` (`a7c50` posts through `b5a70`;
//! `19eb30` walks every player through `19de80`, `19df10` and the
//! announcement formatter `19e890`, which formats text). Those trees are
//! not ported, so this arm is **not** `replaces_guest`: the guest answers
//! every call it declines, and a read that faults ([`Decline::Fault`]).
//!
//! **Proof, then take-over**, as the aim arm: until [`PROVEN_AFTER`] calls
//! have matched with no mismatch the guest answers and the prediction is
//! compared at the return; then the kernel answers and one answerable call
//! in [`AUDIT_EVERY`] is still compared. A mismatch hands every call back
//! to the guest until the next map.

use crate::points;
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest};
use std::sync::atomic::{AtomicU64, Ordering};

/// The stack words `ret` pops besides the return address.
pub const ARGS: u32 = 0;
pub const PROVEN_AFTER: u64 = 4096;
pub const AUDIT_EVERY: u64 = 64;

/// `[game globals]`'s pointer cell.
const GAME: u32 = 0x004e_9ae8;
/// The players' datum array header pointer.
const PLAYERS: u32 = 0x004e_8c24;
/// The game engine globals pointer: `+0xc` is the machine's mode.
const MODE: u32 = 0x004e_6948;
const TICKS: u32 = 0x0051_0c54;
/// The local-user table, indexed by `[game + 0xc14]`.
const USERS: u32 = 0x0055_e4d0;
const STRIDE: u32 = 0x21c;

/// Why a call was left to the guest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// A read of the call's inputs faulted.
    Fault,
    /// The respawn timer is above zero: the countdown's event and entity
    /// calls are the guest's.
    Countdown,
}

impl Decline {
    pub const ALL: [Decline; 2] = [Decline::Fault, Decline::Countdown];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Fault => "fault",
            Decline::Countdown => "countdown",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The arm's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub checked: u64,
    pub matched: u64,
    pub mismatched: u64,
    pub answered: u64,
    /// By [`Decline::ALL`] order.
    pub declined: [u64; 2],
    pub dropped: u64,
}

impl Status {
    pub fn declined_total(&self) -> u64 {
        self.declined.iter().sum()
    }
}

static CHECKED: AtomicU64 = AtomicU64::new(0);
static MATCHED: AtomicU64 = AtomicU64::new(0);
static MISMATCHED: AtomicU64 = AtomicU64::new(0);
static ANSWERED: AtomicU64 = AtomicU64::new(0);
static ANSWERABLE: AtomicU64 = AtomicU64::new(0);
static DROPPED: AtomicU64 = AtomicU64::new(0);
static DECLINED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status {
        checked: read(&CHECKED),
        matched: read(&MATCHED),
        mismatched: read(&MISMATCHED),
        answered: read(&ANSWERED),
        declined: std::array::from_fn(|i| read(&DECLINED[i])),
        dropped: read(&DROPPED),
    }
}

/// Forget the map's proof and counters. Called at map load.
pub fn reset() {
    for c in [&CHECKED, &MATCHED, &MISMATCHED, &ANSWERED, &ANSWERABLE, &DROPPED] {
        c.store(0, Ordering::Relaxed);
    }
    for c in &DECLINED {
        c.store(0, Ordering::Relaxed);
    }
}

/// An audit the runtime could not watch to its return.
pub fn dropped() {
    DROPPED.fetch_add(1, Ordering::Relaxed);
}

fn proven() -> bool {
    MISMATCHED.load(Ordering::Relaxed) == 0 && MATCHED.load(Ordering::Relaxed) >= PROVEN_AFTER
}

/// `eax`, `ecx` and `edx` as a callee leaves them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Regs {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// The kernel's answer: `al` and the registers at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answer {
    pub ready: bool,
    pub regs: Regs,
}

/// `15db30 (player: ecx) -> al`: no lives left. `eax` and `edx` come in as
/// the caller holds them.
pub fn out_of_lives(g: &Guest, mut r: Regs) -> Result<(bool, Regs), Decline> {
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let idx = r.ecx;
    r.edx = word(MODE)?;
    let lives = word(r.edx.wrapping_add(0x1b8))? as i32;
    r.eax &= !0xff;
    if lives <= 0 {
        return Ok((false, r));
    }
    let players = word(PLAYERS)?;
    r.edx = players;
    let data = word(players.wrapping_add(0x44))?;
    let slot = (idx & 0xffff).wrapping_mul(STRIDE);
    r.edx = word(slot.wrapping_add(data).wrapping_add(0x2c))?;
    r.ecx = slot.wrapping_add(data);
    if r.edx != u32::MAX {
        return Ok((false, r));
    }
    let held = g.u16(r.ecx.wrapping_add(0x1ac)).map_err(|_| Decline::Fault)?;
    if held != 0 {
        return Ok((false, r));
    }
    r.eax |= 1;
    Ok((true, r))
}

/// `158a50 (player: esi) -> al`: the player takes part in the round.
/// It overwrites `eax`, `ecx` and `edx` from its own reads, so the caller's
/// values do not show except `edx`'s high half (the datum array's data
/// pointer) and `eax`'s low byte, which it replaces.
pub fn takes_part(g: &Guest, esi: u32) -> Result<Answer, Decline> {
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let players = word(PLAYERS)?;
    let data = word(players.wrapping_add(0x44))?;
    let player = (esi & 0xffff).wrapping_mul(STRIDE).wrapping_add(data);
    let flags = g.u16(player.wrapping_add(2)).map_err(|_| Decline::Fault)?;
    let mut out = Regs {
        eax: player & !0xff,
        ecx: players,
        edx: (data & 0xffff_0000) | u32::from(flags),
    };
    let fail = |out: Regs| Ok(Answer { ready: false, regs: out });
    if flags & 1 == 0 || flags & 2 != 0 {
        return fail(out);
    }
    out.ecx = u32::MAX;
    if g.u8(player.wrapping_add(0xc0)).map_err(|_| Decline::Fault)? == 0xff {
        return fail(out);
    }
    if word(player.wrapping_add(0x2c))? != u32::MAX {
        out.eax |= 1;
        return Ok(Answer { ready: true, regs: out });
    }
    if flags & 0x4000 != 0 {
        return fail(out);
    }
    // `mov ecx, esi; call 15db30`, with `eax` the player pointer.
    out.ecx = esi;
    let (lives_out, r) = out_of_lives(g, out)?;
    out = r;
    // The call's `al` is not kept: `mov al, 1` then `je`/`mov al, bl`.
    out.eax = (out.eax & !0xff) | u32::from(!lives_out);
    Ok(Answer { ready: !lives_out, regs: out })
}

/// `15b2f0 -> al`: a local user is mapped and the machine can spawn.
/// `eax` as the caller holds it. The local-user table's cell is already
/// known non-zero here.
pub fn user_ready(g: &Guest, eax: u32) -> Result<Answer, Decline> {
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let game = word(GAME)?;
    let mut r = Regs {
        eax: eax & !0xff,
        ecx: game,
        edx: word(game.wrapping_add(0xc14))?,
    };
    let user = word(r.edx.wrapping_mul(4).wrapping_add(USERS))?;
    if user == 0 {
        return Ok(Answer { ready: false, regs: r });
    }
    r.edx = 1;
    if g.u16(game.wrapping_add(0x6c)).map_err(|_| Decline::Fault)? != 1 {
        return Ok(Answer { ready: false, regs: r });
    }
    let mode = g.u8(word(MODE)?.wrapping_add(0xc)).map_err(|_| Decline::Fault)?;
    let ready = mode == 4 || word(game.wrapping_add(0xc04))? == 1;
    r.eax |= u32::from(ready);
    Ok(Answer { ready, regs: r })
}

/// The whole call from the registers at its entry (`eax` the slot).
pub fn kernel(g: &Guest, cpu: &CpuAbi) -> Result<Answer, Decline> {
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let game = word(GAME)?;
    let mut edx = word(game.wrapping_add(0xc14))?;
    let user = word(edx.wrapping_mul(4).wrapping_add(USERS))?;
    if user == 0 {
        // `mov al, 1` and out through the shortest exit.
        return Ok(Answer {
            ready: true,
            regs: Regs { eax: (cpu.eax & !0xff) | 1, ecx: 0, edx },
        });
    }
    let players = word(PLAYERS)?;
    let data = word(players.wrapping_add(0x44))?;
    let slot = cpu.eax & 0xffff;
    let player = slot.wrapping_mul(STRIDE).wrapping_add(data);
    let part = takes_part(g, cpu.eax)?;
    if !part.ready {
        return Ok(Answer { ready: false, regs: part.regs });
    }
    let ready = user_ready(g, part.regs.eax)?;
    if !ready.ready {
        return Ok(Answer { ready: false, regs: ready.regs });
    }
    let mut r = ready.regs;
    let timer = word(player.wrapping_add(0x164))? as i32;
    if timer > 0 {
        return Err(Decline::Countdown);
    }
    r.eax = timer as u32;
    r.eax = (r.eax & !0xff) | u32::from(timer == 0);
    if timer != 0 {
        return Ok(Answer { ready: false, regs: r });
    }
    let ticks = word(TICKS)?;
    r.ecx = word(ticks.wrapping_add(8))?;
    edx = word(GAME)?;
    r.edx = edx;
    r.ecx = r.ecx.wrapping_sub(word(edx.wrapping_add(0x70))?);
    let delta = r.ecx as i32;
    if delta <= 3 {
        return Ok(Answer { ready: true, regs: r });
    }
    r.ecx &= 0x1f;
    let ready = r.ecx == slot & 0x1f;
    r.eax = u32::from(ready);
    Ok(Answer { ready, regs: r })
}

/// One call's prediction: the CPU as the return leaves it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prediction {
    pub cpu: CpuAbi,
}

impl Prediction {
    /// The first register the guest's return differs from the prediction in.
    pub fn differs(&self, cpu: &CpuAbi) -> Option<&'static str> {
        let p = &self.cpu;
        [
            ("eax", p.eax, cpu.eax),
            ("ecx", p.ecx, cpu.ecx),
            ("edx", p.edx, cpu.edx),
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
}

/// What [`enter`] did with a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// The kernel answered; the caller returns to `target`.
    Answered { target: u32 },
    /// The guest answers; compare at its return ([`compare`]).
    Audit(Prediction),
    /// The guest answers alone: declined, counted.
    Guest,
}

/// The kernel's prediction for the call `cpu` is entering.
pub fn predict(cpu: &CpuAbi, g: &Guest) -> Result<Prediction, Decline> {
    let answer = kernel(g, cpu)?;
    let mut out = *cpu;
    out.eax = answer.regs.eax;
    out.ecx = answer.regs.ecx;
    out.edx = answer.regs.edx;
    out.esp = cpu.esp.wrapping_add(4 + ARGS);
    out.eflags_defined &= !ARITH;
    Ok(Prediction { cpu: out })
}

/// Put `prediction` in place of the call; returns the return address.
pub fn apply(prediction: &Prediction, cpu: &mut CpuAbi, guest: &Guest) -> Result<u32, runtime::GuestFault> {
    let target = guest.u32(cpu.esp)?;
    let eip = cpu.eip;
    *cpu = prediction.cpu;
    cpu.eip = eip;
    Ok(target)
}

/// At the entry: predict the call and decide who answers.
pub fn enter(cpu: &mut CpuAbi, guest: &Guest) -> Step {
    let prediction = match predict(cpu, guest) {
        Ok(p) => p,
        Err(why) => {
            DECLINED[why.index()].fetch_add(1, Ordering::Relaxed);
            return Step::Guest;
        }
    };
    let slot = ANSWERABLE.fetch_add(1, Ordering::Relaxed);
    if !proven() || slot % AUDIT_EVERY == 0 {
        return Step::Audit(prediction);
    }
    match apply(&prediction, cpu, guest) {
        Ok(target) => {
            ANSWERED.fetch_add(1, Ordering::Relaxed);
            Step::Answered { target }
        }
        Err(_) => {
            DECLINED[Decline::Fault.index()].fetch_add(1, Ordering::Relaxed);
            Step::Guest
        }
    }
}

/// At the guest's return: compare with the prediction. The guest's answer
/// stands either way.
pub fn compare(prediction: &Prediction, cpu: &CpuAbi) {
    CHECKED.fetch_add(1, Ordering::Relaxed);
    match prediction.differs(cpu) {
        None => {
            MATCHED.fetch_add(1, Ordering::Relaxed);
        }
        Some(what) => {
            if MISMATCHED.fetch_add(1, Ordering::Relaxed) == 0 {
                eprintln!(
                    "sim: seats respawn-ready arm: the guest's {what} differs from the prediction; \
                     the guest answers every call until the next map"
                );
            }
        }
    }
}

/// The point the arm is installed at.
pub fn point() -> &'static crate::points::Point {
    &points::PLAYER_RESPAWN_READY
}
