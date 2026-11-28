//! The ocean's resource and texture cache calls in Rust
//! (docs/graphics/retire-d3d-display.md, "The water end"): the texture
//! block's residency `0x0012de70(ecx definition + 0x10, eax 3)` and the
//! stamp `0x0003bcb0(eax image)` of each image, with the texture cache's
//! lookup `0x0012ccf0` for the flags `0xe` it passes.
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x0012de70`: a loaded block (`+0x1e`) stamps its cache entry with the cache's frame, AL 1; an empty one (`+8` at most 0, or `+0xc` below 0) AL 0 | [`resident`] |
//! | `0x0003bcb0`: an image whose `+0x70` is past `[0x004e6488]` with a texture `+0x50` is done | [`stamp`] |
//! | `0x0012ccf0(esi image, edx 0xe, xmm1 0)`: an image not cached (`+0xf` bit 1 clear) gives `+0x50`; else its level-0 handle's entry stamped, and, loaded, its texture and stamp taken | [`stamp`] |
//!
//! What streams is a cause: `texture_cold` (`0x0012de70`'s request of a
//! block not loaded) and `texture_request` (`0x0012ce00`, the texture
//! cache's request of an image not loaded).

use engine::memory::GuestMemory;

use super::Cause;

/// The resource cache `0x0012de70` stamps.
pub const RESOURCES: u32 = 0x004e_649c;
/// The texture cache's stamp an image's `+0x70` is held against.
pub const TEXTURE_STAMP: u32 = 0x004e_6488;
/// `0x0012ccf0`'s caches: the textures' entries (`+0x44`, 40 bytes each)
/// and the resource cache whose entries it stamps.
pub const TEXTURES: u32 = 0x004e_6454;
pub const TEXTURE_RESOURCES: u32 = 0x004e_6464;
/// The byte that enables the format check `0x0012ccb0`.
pub const FORMAT_GATE: u32 = 0x004e_647b;
/// `0x0012ccf0`'s level bounds (`[0x0045dc0c]` 2.0, `[0x0045dbc0]` 1.0).
pub const LEVEL_TWO: u32 = 0x0045_dc0c;
pub const LEVEL_ONE: u32 = 0x0045_dbc0;

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(|_| "unread")
}

fn byte(mem: &dyn GuestMemory, at: u32) -> Result<u8, Cause> {
    mem.u8(at).map_err(|_| "unread")
}

fn set(mem: &mut dyn GuestMemory, at: u32, v: u32) -> Result<(), Cause> {
    mem.set_u32(at, v).map_err(|_| "unwritable")
}

/// `[cache + 0x64] + 0x44`'s entry `index` (24 bytes) stamped `+0x14`
/// with `[cache + 0x38]`.
fn touch(mem: &mut dyn GuestMemory, cache: u32, index: u32) -> Result<(), Cause> {
    let entries = word(&*mem, word(&*mem, cache.wrapping_add(0x64))?.wrapping_add(0x44))?;
    let frame = word(&*mem, cache.wrapping_add(0x38))?;
    set(mem, entries.wrapping_add((index & 0xffff) * 0x18).wrapping_add(0x14), frame)
}

/// `0x0012de70(ecx block, eax 3)`: whether the block is resident.
pub fn resident(mem: &mut dyn GuestMemory, block: u32) -> Result<bool, Cause> {
    if byte(&*mem, block.wrapping_add(0x1e))? != 0 {
        let cache = word(&*mem, RESOURCES)?;
        let index = word(&*mem, block.wrapping_add(0x20))?;
        touch(mem, cache, index)?;
        return Ok(true);
    }
    if word(&*mem, block.wrapping_add(8))? as i32 <= 0 || (word(&*mem, block.wrapping_add(0xc))? as i32) < 0 {
        return Ok(false);
    }
    Err("texture_cold")
}

/// `0x0012ccb0(ecx format)`: the formats its jump table at `0x0012ccd4`
/// answers 1 for (1..=11 and 17..=18).
pub fn format_check(format: u8) -> bool {
    matches!(format, 1..=11 | 17..=18)
}

/// `0x0012ccf0(esi image, edx 0xe, xmm1 0.0)`: the image's texture, 0
/// when not loaded. Bit 2 of the flags zeroes the level's scale, so the
/// level is 0 unless the bounds are at most 0.
pub fn lookup(mem: &mut dyn GuestMemory, image: u32) -> Result<u32, Cause> {
    let mut texture = 0;
    let level = 0.0f32;
    if byte(&*mem, image.wrapping_add(0xf))? & 2 == 0 {
        texture = word(&*mem, image.wrapping_add(0x50))?;
    } else if byte(&*mem, FORMAT_GATE)? != 0 && format_check(byte(&*mem, image.wrapping_add(0x17))?) {
        // The flags' bit 2: no texture.
    } else if mem.f32(image.wrapping_add(0x58)).map_err(|_| "unread")? > level {
        let f = |at| mem.f32(at).map_err(|_| "unread");
        let n = if level >= f(LEVEL_TWO)? { 2 } else if level >= f(LEVEL_ONE)? { 1 } else { 0 };
        let handle = word(&*mem, image.wrapping_add(0x28 + 4 * n))?;
        if handle != u32::MAX {
            let index = handle & 0xffff;
            let entry = word(&*mem, word(&*mem, TEXTURES)?.wrapping_add(0x44))?.wrapping_add(index * 40);
            touch(mem, word(&*mem, TEXTURE_RESOURCES)?, index)?;
            if byte(&*mem, entry.wrapping_add(3))? != 0 {
                texture = entry.wrapping_add(0x14);
                if n == 0 {
                    let stamp = word(&*mem, TEXTURE_STAMP)?.wrapping_add(2);
                    set(mem, image.wrapping_add(0x70), stamp)?;
                    set(mem, image.wrapping_add(0x50), texture)?;
                }
            }
        }
    }
    let current = word(&*mem, image.wrapping_add(0x70))? as i32 >= word(&*mem, TEXTURE_STAMP)? as i32;
    let held = word(&*mem, image.wrapping_add(0x50))?;
    Ok(if current && held != 0 { held } else { texture })
}

/// `0x0003bcb0(eax image)`: nothing for an image already current; else
/// the cache's lookup, and an image it does not hold is the request.
pub fn stamp(mem: &mut dyn GuestMemory, image: u32) -> Result<(), Cause> {
    if word(&*mem, image.wrapping_add(0x70))? as i32 > word(&*mem, TEXTURE_STAMP)? as i32
        && word(&*mem, image.wrapping_add(0x50))? != 0
    {
        return Ok(());
    }
    if lookup(mem, image)? == 0 {
        return Err("texture_request");
    }
    Ok(())
}

#[cfg(test)]
mod tests;
