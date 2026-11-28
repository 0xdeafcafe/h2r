//! A weapon spawner's spare clips in the game
//! (docs/game/forge-map-variants.md, "Properties without a retail field";
//! the reading is docs/engine/notes/2026-11-27-forge-spawner-rows.md).
//!
//! The spawner keeps 8 bytes of state per netgame item entry at
//! `[GAME_ENGINE]+0x7dc`, the object's handle first. A spawner with spare
//! clips gives each weapon it makes a reserve of `clips` clips: the first
//! magazine's reserve (`Weapon::MAGAZINE_RESERVE`) becomes `clips` times the
//! definition magazine's clip (`+0x0a`), capped at its most (`+0x0c`).
//!
//! [`TABLE`] holds the clips by entry index for the loaded map. A Forge
//! session's [`ForgeStep::SpareClips`](crate::sim::calls::ForgeStep) sets it
//! and fills the weapon the entry has out, and a built map's Forge record
//! sets it at activation (`game::forge_record`); the game's `forge_spare_clips`
//! feature fills each weapon the spawner makes after `NETGAME_ITEM_SPAWN`
//! returns, and empties the table at `GAME_ACTIVATE`.
use std::collections::BTreeMap;
use std::sync::Mutex;

use guest_layout::view::objects;
use guest_layout::view::Tags;
use guest_layout::{DatumHandle, GuestMemory, MemFault};
use guest_layout::{Weapon, global};

/// The spawner's state array, from the game engine globals.
pub const STATE: u32 = 0x7dc;
/// Bytes of state per entry: the object (`+0`), the countdown (`+4`) and
/// the abandon count (`+6`).
pub const STATE_STRIDE: u32 = 8;
/// The weapon definition's magazines: count, then the elements.
pub const MAGAZINES: u32 = 0x2c0;
pub const MAGAZINE_LEN: u32 = 0x5c;
/// In a magazine element: the rounds a clip holds and the most the
/// reserve holds.
pub const CLIP: u32 = 0x0a;
pub const RESERVE_MAX: u32 = 0x0c;

const WEAP: u32 = u32::from_be_bytes(*b"weap");

/// The spare clips of each netgame item entry of the loaded map.
pub static TABLE: Mutex<BTreeMap<u32, u8>> = Mutex::new(BTreeMap::new());

/// Give entry `index` spare clips, or none.
pub fn set(index: u32, clips: Option<u8>) {
    let mut t = TABLE.lock().unwrap_or_else(|e| e.into_inner());
    match clips {
        Some(c) => t.insert(index, c),
        None => t.remove(&index),
    };
}

/// Entry `index`'s spare clips.
pub fn get(index: u32) -> Option<u8> {
    TABLE.lock().unwrap_or_else(|e| e.into_inner()).get(&index).copied()
}

/// Forget every entry's clips: a new map is starting.
pub fn clear() {
    TABLE.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// The spawner's state array, or none with no game engine.
pub fn state(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let e = m.u32(global::GAME_ENGINE.base)?;
    Ok((e != 0).then_some(e + STATE))
}

/// The reserve `clips` clips make for the weapon definition at `def`: its
/// first magazine's clip times `clips`, capped. None for a weapon with no
/// magazine.
pub fn reserve(m: &dyn GuestMemory, def: u32, clips: u8) -> Result<Option<i16>, MemFault> {
    if m.i32(def + MAGAZINES)? <= 0 {
        return Ok(None);
    }
    let first = m.u32(def + MAGAZINES + 4)?;
    let clip = i32::from(m.u16(first + CLIP)? as i16).max(0);
    let most = i32::from(m.u16(first + RESERVE_MAX)? as i16).max(0);
    Ok(Some((clip * i32::from(clips)).min(most) as i16))
}

/// Fill the weapon entry `index` has out (the handle at `state + 8 *
/// index`) with `clips` spare clips. The reserve written; none when the
/// entry has no object, or it is no weapon with a magazine.
pub fn fill(
    m: &mut dyn GuestMemory,
    state: u32,
    index: u32,
    clips: u8,
) -> Result<Option<i16>, MemFault> {
    let Some(handle) = DatumHandle::from_raw(m.u32(state + index * STATE_STRIDE)?) else {
        return Ok(None);
    };
    let Some((kind, body)) = objects::get(&*m, handle)? else {
        return Ok(None);
    };
    if kind != objects::WEAPON {
        return Ok(None);
    }
    let Some(def) = body.definition(&*m)? else {
        return Ok(None);
    };
    let Some(tag) = Tags.entry(&*m, def)? else {
        return Ok(None);
    };
    if tag.group(&*m)?.0 != WEAP {
        return Ok(None);
    }
    let Some(n) = reserve(&*m, tag.address(&*m)?, clips)? else {
        return Ok(None);
    };
    Weapon::at(body.addr).set(m, Weapon::MAGAZINE_RESERVE, n)?;
    Ok(Some(n))
}

#[cfg(test)]
#[path = "spare_clips_tests.rs"]
mod tests;
