//! The game engine's round state machine `0x0015c550`, which `post_game`
//! (crate::post_game) closes every round tick with, in Rust. On most
//! ticks the next state `G+0xc04` is the state `(i16) G+0x6c` and it
//! returns at once, with no guest call. A change runs its arm, the
//! engine's state virtual, and the new state is stored; the leaves of a
//! change stay guest calls.
//!
//! The guest body, whole (`0x15c550..0x15c6eb`, read from `h2 disasm`).
//! `G` is the game globals `[0x4e9ae8]`, `E` the engine globals
//! `[0x4e6948]`, `T` the game time `[0x510c54]`:
//!
//! | Step | Guest | Calls |
//! |---|---|---|
//! | 1 | `G+0xc04` equal to `(i16) G+0x6c`: return | |
//! | 2a | next 3: `(i16) G+0x6e` += 1, then `G+0xc00` = `(i16) T+2` | `a7810(8)` |
//! | 2b | next 2: `162030`, and if it answers a pointer, `162030` again and its byte 0; not a client and `[0x510ca0]` set: `[0x510ca1]` = 1; `G+0xc00` = `(i16) T+2 * 5` | `162030` |
//! | 2c | next 1: `157ae0`; `162030` as 2b with the byte 1; `G` read again, `G+0x70` = `T+8`; not a client: every player (the iterator `19f240` on the frame) gets `E+0x1b8` (0 as -1) at `+0x1ac` and `+2 \|= 8`, and with an engine its unit `G+0x2c + 4 * (i16) index`, if any, `b58c0(unit, 0x20)` | `157ae0`, `162030`, `19f240`, `b58c0` |
//! | 3 | the engine `[0x55e4d0 + 4 * G+0xc14]`'s virtual `+0x90` (`ecx` the engine, the next state) | the virtual |
//! | 4 | `G` read again: `G+0x6c` = `(u16) G+0xc04`; with an engine and `G+0x24` not -1, `b58c0(G+0x24, 2)` | `b58c0` |
//!
//! `a7810`, `b58c0` and the virtual pop their arguments. The frame is the
//! guest's, `0x10` bytes of locals under the return address with `edi`,
//! `ebx` and `esi` pushed below; the player iterator is the locals. A read
//! that faults is the guest's own: the lifted body faults on the same read.

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::{GuestMemory, MemFault};

/// The function's guest entry.
pub const ENTRY: u32 = 0x0015_c550;
pub const GAME: u32 = 0x004e_9ae8;
pub const ENGINE: u32 = 0x004e_6948;
pub const ENGINES: u32 = 0x0055_e4d0;
pub const TIME: u32 = 0x0051_0c54;
pub const PLAYERS: u32 = 0x004e_8c24;
pub const SHOW: u32 = 0x0051_0ca0;
pub const QUIET: u32 = 0x0051_0ca1;
const CLIENT: u8 = 4;

pub const END: u32 = 0x000a_7810;
pub const POINTER: u32 = 0x0016_2030;
pub const ROUND_RESET: u32 = 0x0015_7ae0;
pub const NEXT_PLAYER: u32 = 0x0019_f240;
pub const OBJECT_FLAGS: u32 = 0x000b_58c0;
/// Every fixed function a change calls (the virtual is read from the engine).
pub const CALLEES: [u32; 5] = [END, POINTER, ROUND_RESET, NEXT_PLAYER, OBJECT_FLAGS];
/// The engine virtual's slot.
pub const VIRTUAL: u32 = 0x90;

/// The frame under the caller's reserve: the return address, `0x10` of
/// locals, `edi`, `ebx`, `esi`.
const FRAME: u32 = 0x20;
/// The locals' base under the caller's reserve: the iterator.
const ITERATOR: u32 = 0x14;

const STATE: u32 = 0x6c;
const ROUND: u32 = 0x6e;
const ROUND_PLAYED: u32 = 0x70;
const UNITS: u32 = 0x2c;
const OBJECT: u32 = 0x24;
const COUNTDOWN: u32 = 0xc00;
const NEXT: u32 = 0xc04;
const INDEX: u32 = 0xc14;

/// What a tick does, from step 1's reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The state stays: nothing.
    None,
    /// Step 2's arm for the next state, then steps 3 and 4.
    To(u32),
}

pub fn change(m: &dyn GuestMemory) -> Result<Change, MemFault> {
    let g = m.u32(GAME)?;
    let state = m.u16(g.wrapping_add(STATE))? as i16 as i32 as u32;
    let next = m.u32(g.wrapping_add(NEXT))?;
    Ok(if next == state { Change::None } else { Change::To(next) })
}

fn engine(m: &dyn GuestMemory, g: u32) -> Result<u32, MemFault> {
    m.u32(ENGINES.wrapping_add(m.u32(g.wrapping_add(INDEX))?.wrapping_mul(4)))
}

/// `(i16) T+2`, the countdown the arms set.
fn ticks(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    Ok(m.u16(m.u32(TIME)?.wrapping_add(2))? as i16 as i32 as u32)
}

/// The machine is a client.
fn client(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(m.u8(m.u32(ENGINE)?.wrapping_add(0xc))? == CLIENT)
}

/// `162030`, and if it answers a pointer, `162030` again and that byte
/// set to `to`.
async fn mark(cx: &Cx, call: impl Fn() -> GuestCall, to: u8) -> Result<(), EntryError> {
    if cx.call(call()).await?.eax != 0 {
        let p = cx.call(call()).await?.eax;
        cx.write(|m| m.set_u8(p, to))?;
    }
    Ok(())
}

/// The function. `reserve` is the stack the caller keeps below its
/// entry; `esi` and `ebx` are the registers it leaves for the callees
/// (`None`: as the caller found it).
pub async fn run(cx: &Cx, reserve: u32, esi: u32, ebx: Option<u32>) -> Result<(), EntryError> {
    let Change::To(next) = cx.read(|m| change(m))? else { return Ok(()) };
    let below = reserve.wrapping_add(FRAME);
    let g = cx.read(|m| m.u32(GAME))?;
    let call = |target: u32, g: u32| {
        let c = GuestCall::new(target).reserve(below).edi(g).esi(esi);
        match ebx {
            Some(ebx) => c.ebx(ebx),
            None => c,
        }
    };
    let g = match next {
        3 => {
            cx.write(|m| -> Result<(), MemFault> {
                let at = g.wrapping_add(ROUND);
                let round = m.u16(at)?;
                m.set_u16(at, round.wrapping_add(1))
            })?;
            cx.call(call(END, g).args([8])).await?;
            cx.write(|m| -> Result<(), MemFault> {
                let t = ticks(m)?;
                m.set_u32(g.wrapping_add(COUNTDOWN), t)
            })?;
            g
        }
        2 => {
            mark(cx, || call(POINTER, g), 0).await?;
            cx.write(|m| -> Result<(), MemFault> {
                if !client(m)? && m.u8(SHOW)? != 0 {
                    m.set_u8(QUIET, 1)?;
                }
                let t = ticks(m)?;
                m.set_u32(g.wrapping_add(COUNTDOWN), t.wrapping_mul(5))
            })?;
            g
        }
        1 => {
            cx.call(call(ROUND_RESET, g)).await?;
            mark(cx, || call(POINTER, g), 1).await?;
            let (g, e, is_client) = cx.write(|m| -> Result<(u32, u32, bool), MemFault> {
                let played = m.u32(m.u32(TIME)?.wrapping_add(8))?;
                let g = m.u32(GAME)?;
                let e = m.u32(ENGINE)?;
                m.set_u32(g.wrapping_add(ROUND_PLAYED), played)?;
                Ok((g, e, m.u8(e.wrapping_add(0xc))? == CLIENT))
            })?;
            if !is_client {
                players(cx, reserve, below, g, e).await?;
            }
            g
        }
        _ => g,
    };
    // Step 3: the engine's state virtual.
    let (this, vtable, arg) = cx.read(|m| -> Result<(u32, u32, u32), MemFault> {
        let this = engine(m, g)?;
        Ok((this, m.u32(this)?, m.u32(g.wrapping_add(NEXT))?))
    })?;
    let target = cx.read(|m| m.u32(vtable.wrapping_add(VIRTUAL)))?;
    cx.call(call(target, g).ecx(this).edx(vtable).eax(arg).args([arg])).await?;
    // Step 4: the new state stored, the game object told.
    let object = cx.write(|m| -> Result<Option<u32>, MemFault> {
        let g = m.u32(GAME)?;
        let next = m.u16(g.wrapping_add(NEXT))?;
        m.set_u16(g.wrapping_add(STATE), next)?;
        if engine(m, g)? == 0 {
            return Ok(None);
        }
        let object = m.u32(g.wrapping_add(OBJECT))?;
        Ok((object != u32::MAX).then_some(object))
    })?;
    if let Some(object) = object {
        cx.call(call(OBJECT_FLAGS, g).args([object, 2])).await?;
    }
    Ok(())
}

/// Step 2c's player walk: the iterator is the frame's locals.
async fn players(cx: &Cx, reserve: u32, below: u32, g: u32, e: u32) -> Result<(), EntryError> {
    let iter = cx.entry().esp.wrapping_sub(reserve).wrapping_sub(ITERATOR);
    cx.write(|m| -> Result<(), MemFault> {
        let pool = m.u32(PLAYERS)?;
        m.set_u32(iter.wrapping_add(4), pool)?;
        m.set_u32(iter.wrapping_add(8), u32::MAX)?;
        m.set_u32(iter.wrapping_add(0xc), u32::MAX)
    })?;
    let call = |target: u32| GuestCall::new(target).reserve(below).edi(g).ebx(e);
    while cx.call(call(NEXT_PLAYER).esi(iter)).await?.eax as u8 != 0 {
        let (player, unit) = cx.write(|m| -> Result<(u32, Option<u32>), MemFault> {
            let index = m.u32(iter.wrapping_add(8))? as u16 as i16 as i32;
            let mut seat = m.u32(e.wrapping_add(0x1b8))?;
            if seat == 0 {
                seat = u32::MAX;
            }
            let player = m.u32(iter)?;
            m.set_u16(player.wrapping_add(0x1ac), seat as u16)?;
            if engine(m, g)? == 0 {
                return Ok((player, None));
            }
            let unit = m.u32(g.wrapping_add(UNITS).wrapping_add((index as u32).wrapping_mul(4)))?;
            Ok((player, (unit != u32::MAX).then_some(unit)))
        })?;
        if let Some(unit) = unit {
            cx.call(call(OBJECT_FLAGS).esi(player).args([unit, 0x20])).await?;
        }
        cx.write(|m| -> Result<(), MemFault> {
            let flags = m.u8(player.wrapping_add(2))?;
            m.set_u8(player.wrapping_add(2), flags | 8)
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
