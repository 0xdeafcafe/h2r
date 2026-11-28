//! The respawn gate `15dd40` (`PLAYER_RESPAWN_READY`) in Rust
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
//! **The countdown.** A timer above zero runs the countdown in Rust
//! (`respawn/countdown.rs`): on the ticks at three, two and one seconds
//! and at the last tick it builds the death event on its stack and calls
//! `19eb90`, then decrements the timer, writes the fade `+0x170` (the
//! seconds left, held to `0..=0x3ff`), calls the entity mark `a7840` (its
//! tail call `b58c0` ORs `0x80` into the entity's network update word)
//! and, at zero, takes the staggered check. Its writes (the timer, the fade, the update
//! word) ride in the [`Prediction`] and land in [`apply`].
//!
//! **What it refuses, by name.** The death event's paths that reach
//! further guest systems: [`Decline::Post`] (a host's event message,
//! `8b4d0`), [`Decline::Announce`] (a local player's HUD text and announcer
//! sound in `19e890`) and [`Decline::EveryPlayer`] (an event naming no
//! player, which `15dd40` never builds); and [`Decline::Fault`], a read
//! that faults or a seconds divisor of 0. Each is
//! `unimplemented!("seats: respawn-ready: <cause>")`; no lifted body runs
//! in its place. Every other call the kernel answers.

mod countdown;

pub use countdown::{
    Mem, WRITES, Writes, announce, announce_event, announce_player, announcement, death_event,
    entity_mark, entity_queue, event_post,
};

use crate::points;
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest};
use std::sync::atomic::{AtomicU64, Ordering};

/// The stack words `ret` pops besides the return address.
pub const ARGS: u32 = 0;

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

/// Why the kernel cannot answer a call: the cause its panic names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// A read of the call's inputs faulted, or the seconds divisor
    /// `(i16)[[510c54] + 2]` is 0.
    Fault,
    /// The death event on a host in network state 4 or 5: `b5a70` queues
    /// its message through `8b4d0`.
    Post,
    /// The death event's announcement for a local player: `19e890` draws
    /// its HUD text and plays the announcer.
    Announce,
    /// A death event naming no player: `19eb30` walks every player
    /// through `19f240`.
    EveryPlayer,
}

impl Decline {
    pub const ALL: [Decline; 4] = [Decline::Fault, Decline::Post, Decline::Announce, Decline::EveryPlayer];

    pub fn name(self) -> &'static str {
        match self {
            Decline::Fault => "fault",
            Decline::Post => "post",
            Decline::Announce => "announce",
            Decline::EveryPlayer => "every-player",
        }
    }
}

/// The arm's counters since the map loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub answered: u64,
}

static ANSWERED: AtomicU64 = AtomicU64::new(0);

pub fn status() -> Status {
    let read = |c: &AtomicU64| c.load(Ordering::Relaxed);
    Status { answered: read(&ANSWERED) }
}

/// Clear the counter. Called at map load.
pub fn reset() {
    ANSWERED.store(0, Ordering::Relaxed);
}

/// A call the kernel cannot answer: an unported path, by name.
fn decline(why: Decline) -> ! {
    unimplemented!("seats: respawn-ready: {}", why.name())
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

/// The whole call from the registers at its entry (`eax` the slot), with
/// the words it writes left in `m`.
pub fn kernel(m: &mut Mem, cpu: &CpuAbi) -> Result<Answer, Decline> {
    let g = m.guest();
    let word = |a: u32| g.u32(a).map_err(|_| Decline::Fault);
    let game = word(GAME)?;
    let edx = word(game.wrapping_add(0xc14))?;
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
    r.eax = timer as u32;
    if timer > 0 {
        r = count_down(m, r, cpu, player)?;
    }
    // `15de51`: the timer as the countdown left it.
    let zero = m.u32(player.wrapping_add(0x164))? == 0;
    r.eax = (r.eax & !0xff) | u32::from(zero);
    if !zero {
        return Ok(Answer { ready: false, regs: r });
    }
    stagger(m, r, slot)
}

/// `15ddaa..15de51`: the death event on the second ticks, the timer's
/// decrement, the fade and the entity mark. `r.eax` is the timer.
fn count_down(m: &mut Mem, mut r: Regs, cpu: &CpuAbi, player: u32) -> Result<Regs, Decline> {
    let timer = r.eax;
    r.ecx = seconds(m)?;
    r.edx = r.ecx.wrapping_mul(3);
    let kind = if timer == r.edx {
        Some(0x14)
    } else {
        r.edx = r.ecx.wrapping_mul(2);
        if timer == r.edx || timer == r.ecx {
            Some(0x14)
        } else if timer == 1 {
            Some(0x15)
        } else {
            None
        }
    };
    if let Some(kind) = kind {
        // The frame below the four pushes: `esp` is `entry - 0x34`, the
        // event is at `[esp + 0x10]`.
        let esp = cpu.esp.wrapping_sub(0x34);
        let event = esp.wrapping_add(0x10);
        m.scratch_u32(esp.wrapping_add(0x14), kind)?;
        m.scratch_u16(esp.wrapping_add(0x30), 0xffff)?;
        for off in [0x28, 0x24, 0x20] {
            m.scratch_u32(esp.wrapping_add(off), u32::MAX)?;
        }
        m.scratch_u32(esp.wrapping_add(0x18), cpu.eax)?;
        m.scratch_u32(esp.wrapping_add(0x1c), cpu.eax)?;
        m.scratch_u32(esp.wrapping_add(0x2c), 0)?;
        m.scratch_u32(event, 0)?;
        r.eax = event;
        death_event(m, r, esp.wrapping_sub(4), event)?;
    }
    // `15de0a`.
    let left = m.u32(player.wrapping_add(0x164))?.wrapping_sub(1);
    m.set_u32(player.wrapping_add(0x164), left)?;
    let per = seconds(m)? as i32;
    let left = left as i32;
    let (quotient, remainder) = match (left.checked_div(per), left.checked_rem(per)) {
        (Some(q), Some(rem)) => (q, rem),
        _ => return Err(Decline::Fault),
    };
    m.set_u32(player.wrapping_add(0x170), quotient as u32)?;
    let fade = quotient.clamp(0, 0x3ff) as u32;
    m.set_u32(player.wrapping_add(0x170), fade)?;
    let regs = Regs { eax: fade, ecx: per as u32, edx: remainder as u32 };
    entity_mark(m, regs, cpu.eax, 0x80)
}

/// The ticks per second, `movsx [[510c54] + 2]`.
fn seconds(m: &Mem) -> Result<u32, Decline> {
    Ok(m.u16(m.u32(TICKS)?.wrapping_add(2))? as i16 as i32 as u32)
}

/// `15de5e`: a timer at zero waits for the slot's turn once more than
/// three ticks have passed since `[game + 0x70]`. `al` is 1 coming in.
fn stagger(m: &Mem, mut r: Regs, slot: u32) -> Result<Answer, Decline> {
    let ticks = m.u32(TICKS)?;
    r.ecx = m.u32(ticks.wrapping_add(8))?;
    r.edx = m.u32(GAME)?;
    r.ecx = r.ecx.wrapping_sub(m.u32(r.edx.wrapping_add(0x70))?);
    if r.ecx as i32 <= 3 {
        return Ok(Answer { ready: true, regs: r });
    }
    r.ecx &= 0x1f;
    let ready = r.ecx == slot & 0x1f;
    if !ready {
        r.eax &= !0xff;
    }
    Ok(Answer { ready, regs: r })
}

/// One call's prediction: the CPU as the return leaves it, and the words
/// the call writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Prediction {
    pub cpu: CpuAbi,
    pub writes: Writes,
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

    /// Whether a word the prediction writes holds something else in
    /// `guest` after the guest's return.
    pub fn memory_differs(&self, guest: &Guest) -> bool {
        self.writes.iter().any(|(a, v)| guest.u32(a).ok() != Some(v))
    }
}

/// The kernel's prediction for the call `cpu` is entering.
pub fn predict(cpu: &CpuAbi, g: &Guest) -> Result<Prediction, Decline> {
    let mut m = Mem::new(g);
    let answer = kernel(&mut m, cpu)?;
    let mut out = *cpu;
    out.eax = answer.regs.eax;
    out.ecx = answer.regs.ecx;
    out.edx = answer.regs.edx;
    out.esp = cpu.esp.wrapping_add(4 + ARGS);
    out.eflags_defined &= !ARITH;
    Ok(Prediction { cpu: out, writes: m.writes() })
}

/// Put `prediction` in place of the call: its writes, then its registers.
/// Returns the return address. Every written word is written back as it
/// is first, so a fault lands nothing.
pub fn apply(prediction: &Prediction, cpu: &mut CpuAbi, guest: &mut Guest) -> Result<u32, runtime::GuestFault> {
    let target = guest.u32(cpu.esp)?;
    for (a, _) in prediction.writes.iter() {
        let old = guest.u32(a)?;
        guest.set_u32(a, old)?;
    }
    for (a, v) in prediction.writes.iter() {
        guest.set_u32(a, v)?;
    }
    let eip = cpu.eip;
    *cpu = prediction.cpu;
    cpu.eip = eip;
    Ok(target)
}

/// At the entry: the kernel answers, and the caller returns to the
/// target. A call it cannot answer panics by its [`Decline`].
pub fn enter(cpu: &mut CpuAbi, guest: &mut Guest) -> u32 {
    let prediction = predict(cpu, guest).unwrap_or_else(|why| decline(why));
    let target = apply(&prediction, cpu, guest).unwrap_or_else(|_| decline(Decline::Fault));
    ANSWERED.fetch_add(1, Ordering::Relaxed);
    target
}

/// The point the arm is installed at.
pub fn point() -> &'static crate::points::Point {
    &points::PLAYER_RESPAWN_READY
}
