//! Tick phase 17, `post_game` `0x0015ca10`, in Rust (docs/engine/sim-rewrite.md,
//! "The tick driver"). The phase's own body is Rust; the twelve functions it
//! calls are still the lifted ones, one guest call each, counted in
//! [`Stats::calls`].
//!
//! The guest body, whole (`0x15ca10..0x15cb93`, read from `h2 disasm`).
//! `G` is the game globals `[0x4e9ae8]`, `E` the engine globals `[0x4e6948]`:
//!
//! | Step | Guest | Calls |
//! |---|---|---|
//! | 1 | `[0x55e4d0 + 4 * [G+0xc14]]` 0, or `E+0xc` 4 (a client): return | |
//! | 2 | `E+0x1128` clear: the round countdown, by the state `(i16) G+0x6c` | |
//! | 2a | state 3: `G+0xc00` falls by 1; at 0 or below `edx` 1, then `[G] \|= 0x20` | `15fe50`, `15c550` |
//! | 2b | state 2: `G+0xc00` falls by 1; at 0 or below, `(i16) G+0x6e` < `0x1f`: `edx` 3, else `15ba00` | `15fe50` or `15ba00`, `15c550` |
//! | 2c | any other state, or the countdown still above 0 | `15c550` |
//! | 3 | `E+0x1128` set: once (`G+0xc08` clear, then set) the game ends | `a7810(4)` |
//! | 3a | state 1 and `G+0xc04` 1: | `15b330`, `15b3a0(0)` |
//! | 3b | state 1 and `G+0xc04` (read again, `G` too) not 1 | `15c550` |
//! | 3c | then a `0x22`-byte request on the frame (`eax`), `ecx` 0 | `15dd10`, `19eb90` |
//! | 3d | `[0x510ca0]` set and `[0x510cb1]` clear: `162030`, and if it answers a pointer, `162030` again and its byte cleared; `[0x510ca1]` 0 around the next two, then 1 | `162030`, `15c6f0`, `196430`, `196390` |
//!
//! The frame is where the guest keeps it, `0x24` bytes under the return
//! address with `esi` and `ebx` pushed below, and the calls go below it.
//! `esi` is `G` and `ebx` 1 in step 3, as the guest leaves them for its
//! callees. A read that faults is the guest's own: the lifted body faults
//! on the same read.

use crate::entry::{Cx, EntryError, GuestCall};
use crate::memory::{GuestMemory, MemFault};
use std::sync::Mutex;

/// The phase's guest entry.
pub const ENTRY: u32 = 0x0015_ca10;
/// The game globals pointer.
pub const GAME: u32 = 0x004e_9ae8;
/// The game engine globals pointer; byte `+0xc` is the machine's mode.
pub const ENGINE: u32 = 0x004e_6948;
/// The game engine table, indexed by `G+0xc14`: no engine, no phase.
pub const ENGINES: u32 = 0x0055_e4d0;
const CLIENT: u8 = 4;
/// `E+0x1128`: the game is over.
const OVER: u32 = 0x1128;
/// `(i16) G+0x6c`: the round state.
const STATE: u32 = 0x6c;
/// `(i16) G+0x6e`: compared with `0x1f` when state 2's countdown ends.
const ROUND: u32 = 0x6e;
const LAST_ROUND: i16 = 0x1f;
/// `G+0xc00`: the countdown.
const COUNTDOWN: u32 = 0xc00;
/// `G+0xc04`: compared with 1 at the game's end.
const ENDING: u32 = 0xc04;
/// `G+0xc08`: the end ran.
const ENDED: u32 = 0xc08;
/// The flags of step 3d.
pub const SHOW: u32 = 0x0051_0ca0;
pub const HELD: u32 = 0x0051_0cb1;
pub const QUIET: u32 = 0x0051_0ca1;

pub const STATE_CHANGE: u32 = 0x0015_fe50;
pub const AFTER: u32 = 0x0015_c550;
pub const LAST: u32 = 0x0015_ba00;
pub const END: u32 = 0x000a_7810;
pub const SCORE: u32 = 0x0015_b330;
pub const SCORED: u32 = 0x0015_b3a0;
pub const FINISH: u32 = 0x0015_dd10;
pub const REQUEST: u32 = 0x0019_eb90;
pub const POINTER: u32 = 0x0016_2030;
pub const QUIET_A: u32 = 0x0015_c6f0;
pub const QUIET_B: u32 = 0x0019_6430;
pub const LOUD: u32 = 0x0019_6390;
/// Every function the phase calls.
pub const CALLEES: [u32; 12] =
    [STATE_CHANGE, AFTER, LAST, END, SCORE, SCORED, FINISH, REQUEST, POINTER, QUIET_A, QUIET_B, LOUD];

/// The guest's frame under its return address: `sub esp, 0x24`, `esi`,
/// `ebx`, and the return address's own slot.
const FRAME: u32 = 0x30;
/// The request's place above the bottom of the frame: `esp + 8` after both
/// pushes, `0x24` under the return address's slot.
const REQUEST_AT: u32 = 8;

/// The request step 3c builds, in guest order.
pub const REQUEST_WORDS: [(u32, u32); 8] =
    [(0, 0), (4, 0x13), (8, !0), (0xc, !0), (0x10, !0), (0x14, !0), (0x18, !0), (0x1c, 0)];

/// What the phase does this tick, from step 1 and 2's reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// No engine, or a client.
    Idle,
    /// The round countdown (step 2), with `G`.
    Round(u32),
    /// The game is over (step 3), with `G`.
    Over(u32),
}

pub fn plan(m: &dyn GuestMemory) -> Result<Plan, MemFault> {
    let g = m.u32(GAME)?;
    let index = m.u32(g.wrapping_add(0xc14))?;
    if m.u32(ENGINES.wrapping_add(index.wrapping_mul(4)))? == 0 {
        return Ok(Plan::Idle);
    }
    let e = m.u32(ENGINE)?;
    if m.u8(e.wrapping_add(0xc))? == CLIENT {
        return Ok(Plan::Idle);
    }
    Ok(if m.u8(e.wrapping_add(OVER))? != 0 { Plan::Over(g) } else { Plan::Round(g) })
}

/// Step 2's decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Round {
    /// `15fe50` with `edx`; `or` marks `[G] |= 0x20` after it.
    Change { edx: u32, or: bool },
    /// The last round's call.
    Last,
    /// Only the closing call.
    Tick,
}

/// Step 2's countdown and decision: the countdown falls for states 2 and 3.
pub fn round(m: &mut dyn GuestMemory, g: u32) -> Result<Round, MemFault> {
    let state = m.u16(g.wrapping_add(STATE))? as i16;
    if state != 2 && state != 3 {
        return Ok(Round::Tick);
    }
    let left = m.u32(g.wrapping_add(COUNTDOWN))?.wrapping_sub(1);
    m.set_u32(g.wrapping_add(COUNTDOWN), left)?;
    if left as i32 > 0 {
        return Ok(Round::Tick);
    }
    Ok(match state {
        3 => Round::Change { edx: 1, or: true },
        _ if (m.u16(g.wrapping_add(ROUND))? as i16) < LAST_ROUND => Round::Change { edx: 3, or: false },
        _ => Round::Last,
    })
}

/// The phase's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Ticks the round countdown ran.
    pub rounds: u64,
    /// Games ended.
    pub ends: u64,
    /// Guest calls into functions not ported yet.
    pub calls: u64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats { rounds: 0, ends: 0, calls: 0 });

pub fn stats() -> Stats {
    *STATS.lock().unwrap_or_else(|e| e.into_inner())
}

fn count(f: impl FnOnce(&mut Stats)) {
    f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The phase. `reserve` is the stack the driver keeps below its entry.
pub async fn run(cx: &Cx, reserve: u32) -> Result<(), EntryError> {
    let below = reserve.wrapping_add(FRAME);
    let call = |target: u32, g: u32| {
        count(|s| s.calls += 1);
        GuestCall::new(target).reserve(below).esi(g)
    };
    match cx.read(|m| plan(m))? {
        Plan::Idle => {}
        Plan::Round(g) => {
            count(|s| s.rounds += 1);
            match cx.write(|m| round(m, g))? {
                Round::Change { edx, or } => {
                    cx.call(call(STATE_CHANGE, g).edx(edx)).await?;
                    if or {
                        cx.write(|m| -> Result<(), MemFault> {
                            let v = m.u32(g)?;
                            m.set_u32(g, v | 0x20)
                        })?;
                    }
                }
                Round::Last => {
                    cx.call(call(LAST, g)).await?;
                }
                Round::Tick => {}
            }
            cx.call(call(AFTER, g)).await?;
        }
        Plan::Over(g) => {
            if cx.read(|m| m.u8(g.wrapping_add(ENDED)))? != 0 {
                return Ok(());
            }
            count(|s| s.ends += 1);
            cx.write(|m| m.set_u8(g.wrapping_add(ENDED), 1))?;
            cx.call(call(END, g).ebx(1).args([4])).await?;
            let mut g = g;
            let state = cx.read(|m| m.u16(g.wrapping_add(STATE)))? as i16;
            if state == 1 {
                let mut closing = cx.read(|m| m.u32(g.wrapping_add(ENDING)))? != 1;
                if !closing {
                    let score = cx.call(call(SCORE, g).ebx(1)).await?.eax;
                    cx.call(call(SCORED, g).ebx(score).args([0])).await?;
                    g = cx.read(|m| m.u32(GAME))?;
                    closing = cx.read(|m| m.u32(g.wrapping_add(ENDING)))? != 1;
                }
                if closing {
                    cx.call(call(AFTER, g).ebx(1)).await?;
                }
            }
            cx.call(call(FINISH, g).ebx(1)).await?;
            let at = cx.entry().esp.wrapping_sub(reserve).wrapping_sub(FRAME).wrapping_add(REQUEST_AT);
            cx.write(|m| -> Result<(), MemFault> {
                for (offset, value) in REQUEST_WORDS {
                    m.set_u32(at.wrapping_add(offset), value)?;
                }
                m.set_u16(at.wrapping_add(0x20), 0xffff)
            })?;
            cx.call(call(REQUEST, g).ebx(1).eax(at).ecx(0)).await?;
            let show = cx.read(|m| -> Result<bool, MemFault> { Ok(m.u8(SHOW)? != 0 && m.u8(HELD)? == 0) })?;
            if show {
                if cx.call(call(POINTER, g).ebx(1)).await?.eax != 0 {
                    let p = cx.call(call(POINTER, g).ebx(1)).await?.eax;
                    cx.write(|m| m.set_u8(p, 0))?;
                }
                cx.write(|m| m.set_u8(QUIET, 0))?;
                cx.call(call(QUIET_A, g).ebx(1)).await?;
                cx.call(call(QUIET_B, g).ebx(1)).await?;
                cx.write(|m| m.set_u8(QUIET, 1))?;
                cx.call(call(LOUD, g).ebx(1)).await?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
