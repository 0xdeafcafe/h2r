//! The players' tick phases in Rust (docs/engine/sim-rewrite-tick.md).
//!
//! Phase 26, `first_person` `0x00165dc1`, is here: for each of the four
//! local users it finds the user's player, refreshes the first-person
//! block's unit and weapon-selection state when they differ, and for a user
//! whose block holds a unit, checks its two weapon slots. The leaf work
//! (`0x16896f`, `0x1682bf`, `0xcbd50`, `0x167e86`, `0x166d75`) stays the
//! lifted functions, one guest call each. Phases 1 (`players_before`
//! `0x14d3a0`) and 18 (`players_after` `0x14d8b0`) are still guest.
//!
//! The guest body, whole (`0x165dc1..0x165e9e`, read from `h2 disasm`).
//! `F` is the block `[0x4e9bc8] + 0x20cc * user`, `P` the players
//! datum array `[[0x4e8c24] + 0x44]`:
//!
//! | Step | Guest | Calls |
//! |---|---|---|
//! | 1 | `i = [[0x4e8c20] + 0xc + 4 * user]`, the local user's player; `-1`: next user | |
//! | 2 | `player = P + 0x21c * (i & 0xffff)`; `sel = (i8) player+0x88`; `eax` = f(sel) | `16896f(eax = sel)` |
//! | 3 | `F+4 != player+0x2c` or `F+8 != eax`: refresh the block | `1682bf(eax = player+0x2c, user, eax)` |
//! | 4 | `F+4` still `-1`: next user | |
//! | 5 | slots 0 and 1: the unit's `(i8) +0x212+slot` through `cbd50`; `!= F+0x10+0x1010*slot`: refresh the slot | `cbd50`, `167e86(user, slot)` |
//! | 6 | the user's last step | `166d75(user)` |

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::{GuestMemory, MemFault};

/// Phase 26's guest entry (the table's name for the row).
pub const FIRST_PERSON: u32 = 0x0016_5dc1;
/// The first-person block pointer, `0x20cc` bytes per user.
pub const BLOCK: u32 = 0x004e_9bc8;
pub const USER_SIZE: u32 = 0x20cc;
pub const USERS: u32 = 4;
/// The players' globals pointer; `+0xc + 4 * user` is the user's player.
pub const GLOBALS: u32 = 0x004e_8c20;
/// The players data array header pointer; `+0x44` is its data.
pub const PLAYERS: u32 = 0x004e_8c24;
/// Bytes per player datum.
pub const PLAYER_SIZE: u32 = 0x21c;
/// The object table header pointer; `+0x44` is its data (12-byte slots, the
/// object at `+8`).
pub const OBJECTS: u32 = 0x004e_0300;
/// The unit's selection byte, through `0x16896f`.
pub const SELECTED: u32 = 0x88;
/// The player's unit.
pub const UNIT: u32 = 0x2c;
/// A weapon slot's byte in the unit object, `+ slot`.
pub const SLOT_BYTE: u32 = 0x212;
/// A slot's state in the block, `+ 0x1010 * slot`.
pub const SLOT_STATE: u32 = 0x10;
pub const SLOT_STRIDE: u32 = 0x1010;

pub const SELECTION: u32 = 0x0016_896f;
pub const REFRESH: u32 = 0x0016_82bf;
pub const SLOT_ID: u32 = 0x000c_bd50;
pub const SLOT_REFRESH: u32 = 0x0016_7e86;
pub const FINISH: u32 = 0x0016_6d75;
/// Every function the phase calls as a guest call.
pub const CALLEES: [u32; 5] = [SELECTION, REFRESH, SLOT_ID, SLOT_REFRESH, FINISH];

/// The frame the guest keeps under its return address: `ebp`, three locals
/// and three saved registers.
const FRAME: u32 = 0x20;

/// A user's player datum, `None` for an unmapped user (`0x14de70` returns
/// `-1`, and the player index is `-1` for a user with no seat).
pub fn player_of(m: &dyn GuestMemory, user: u32) -> Result<Option<u32>, MemFault> {
    let index = m.u32(m.u32(GLOBALS)?.wrapping_add(0xc + 4 * user))?;
    if index == u32::MAX {
        return Ok(None);
    }
    let data = m.u32(m.u32(PLAYERS)?.wrapping_add(0x44))?;
    Ok(Some(data.wrapping_add((index & 0xffff).wrapping_mul(PLAYER_SIZE))))
}

/// The object a unit handle names: the object table's slot, `+8`.
pub fn object_of(m: &dyn GuestMemory, handle: u32) -> Result<u32, MemFault> {
    let data = m.u32(m.u32(OBJECTS)?.wrapping_add(0x44))?;
    m.u32(data.wrapping_add((handle & 0xffff).wrapping_mul(12)).wrapping_add(8))
}

/// `ecx` at `0xcbd50`: the object's pointer with its low word replaced by
/// the slot byte sign-extended (`movsx cx, byte`), as the guest leaves it.
pub fn slot_ecx(object: u32, byte: u8) -> u32 {
    (object & 0xffff_0000) | (byte as i8 as i16 as u16 as u32)
}

/// The phase. `reserve` is the stack the driver keeps below its entry.
pub async fn first_person(cx: &Cx, reserve: u32) -> Result<(), EntryError> {
    let below = reserve.wrapping_add(FRAME);
    let call = |target: u32| GuestCall::new(target).reserve(below);
    for user in 0..USERS {
        let block = cx.read(|m| m.u32(BLOCK))?.wrapping_add(user * USER_SIZE);
        let Some(player) = cx.read(|m| player_of(m, user))? else { continue };
        let selected = cx.read(|m| m.u8(player.wrapping_add(SELECTED)))? as i8 as i32 as u32;
        let found = cx.call(call(SELECTION).eax(selected)).await?.eax;
        let unit = cx.read(|m| m.u32(player.wrapping_add(UNIT)))?;
        let (held, was) = cx.read(|m| -> Result<_, MemFault> { Ok((m.u32(block + 4)?, m.u32(block + 8)?)) })?;
        if held != unit || was != found {
            cx.call(call(REFRESH).eax(unit).args([user, found])).await?;
        }
        let held = cx.read(|m| m.u32(block + 4))?;
        if held == u32::MAX {
            continue;
        }
        for slot in 0..2 {
            let object = cx.read(|m| object_of(m, held))?;
            let byte = cx.read(|m| m.u8(object.wrapping_add(SLOT_BYTE + slot)))?;
            let id = cx.call(call(SLOT_ID).ecx(slot_ecx(object, byte))).await?.eax;
            let state = cx.read(|m| m.u32(block + SLOT_STATE + SLOT_STRIDE * slot))?;
            if state != id {
                cx.call(call(SLOT_REFRESH).args([user, slot])).await?;
            }
        }
        cx.call(call(FINISH).args([user])).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
