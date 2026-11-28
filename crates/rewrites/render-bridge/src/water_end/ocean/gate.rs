//! The ocean's visibility gate `0x0003c270(definition)` in Rust
//! (docs/graphics/retire-d3d-display.md, "The water end"), answered from
//! the native flare query's count pass ([`crate::native_flare_query`]).
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x000209b0(key {0, view [0x004b9ed4], class 5, 0, 0}, &definition, 4)` | [`flare::register`] |
//! | the entry's visibility byte `+3` times `[0x005234c8]`, held to `[0, 1]`, and its bit in the read set | [`visible`] |
//! | the walker `0x00020bb0(view, 0x20, &index, 1, 1, 0x0003bf20)`: the callback's quad of the four corners at `0x004c1b08` (the grid's), its count's area `0x4b000` | [`flare::walk_listed`] with [`test`] |
//! | AL 0: an entry tested two frames back whose fraction is at most `[0x0045dbd8]`; else AL 1 | [`visible`] |
//!
//! The callback draws the corners through the shader's vertex program
//! with `LESSEQUAL` against the bound depth. Rust counts their screen
//! rectangle at the nearest corner's depth instead, in tiles the count
//! pass takes: every sample the quad passes, that rectangle passes too,
//! so the gate never closes on water the guest would see. A corner behind
//! the eye or the near plane leaves the rectangle unbounded: the test is
//! the whole area, seen.

use guest_layout::memory::GuestMemory;
use scene_extract::GuestRead;
use scene_extract::flares::{self as fl, Lens};

use super::Cause;
use crate::native_flare_query::{self as flare, Key, Listed, Quad, Test};

/// The view the gate registers in (`0x0003c286`).
pub const VIEW: u32 = 0x004b_9ed4;
/// The water's class and the walker mask with only it.
pub const CLASS: u32 = 5;
pub const MASK: u32 = 1 << CLASS;
/// The byte's scale and the fraction at or below which the gate closes.
pub const BYTE_SCALE: u32 = 0x0052_34c8;
pub const SEEN: u32 = 0x0045_dbd8;
/// The four corners `0x0003bf20` draws (twelve floats).
pub const CORNERS: u32 = 0x004c_1b08;
/// `0x0003bf20`'s return, the expected count, held to the walker's 16 bits.
pub const CALLBACK_AREA: u32 = 0x4_b000;
/// The widest side of a quad the count pass takes
/// (`native_render::flare_gpu::TILES`' 257 pixels).
pub const TILE: i32 = 256;

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The guest's float 1.0 (`[0x0045dbc0]`).
const ONE: u32 = 0x0045_dbc0;
fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(|_| "unread")
}

fn float(mem: &dyn GuestMemory, at: u32) -> Result<f32, Cause> {
    mem.f32(at).map_err(|_| "unread")
}

/// The callback's test of the corners: their screen rectangle held to the
/// viewport at the nearest corner's `z` and `w`, cut into tiles; an
/// empty rectangle counts 0; a corner the projection drops is all seen.
pub fn test(mem: &dyn GuestMemory) -> Result<Test, Cause> {
    let lens = Lens::read(&Ro(mem)).ok_or("unread")?;
    let mut corners = Vec::with_capacity(4);
    for k in 0..4 {
        let at = CORNERS + 12 * k;
        let p = [float(mem, at)?, float(mem, at + 4)?, float(mem, at + 8)?];
        match lens.project(p, lens.one) {
            Some((out, _)) => corners.push(out),
            None => return Ok(Test::Known(CALLBACK_AREA.min(0xffff))),
        }
    }
    let fold = |i: usize, f: fn(f32, f32) -> f32| corners.iter().map(|c| c[i]).reduce(f).unwrap_or(0.0);
    let [vy0, vx0, vy1, vx1] = lens.viewport.map(i32::from);
    let x0 = (fold(0, f32::min).floor() as i32).max(vx0);
    let y0 = (fold(1, f32::min).floor() as i32).max(vy0);
    let x1 = (fold(0, f32::max).ceil() as i32).min(vx1);
    let y1 = (fold(1, f32::max).ceil() as i32).min(vy1);
    if x1 <= x0 || y1 <= y0 {
        return Ok(Test::Known(0));
    }
    let depth = |v: f32| v.max(0.0).min(lens.z_scale);
    let (z, w) = (depth(fold(2, f32::min)), depth(fold(3, f32::min)));
    let mut quads = Vec::new();
    for ty in (y0..y1).step_by(TILE as usize) {
        for tx in (x0..x1).step_by(TILE as usize) {
            let (ex, ey) = ((tx + TILE).min(x1), (ty + TILE).min(y1));
            quads.push(Quad { x0: tx, y0: ty, x1: ex, y1: ey, z, w, area: ((ex - tx) * (ey - ty)) as u32 });
        }
    }
    Ok(Test::Quads(quads))
}

/// `0x0003c270(definition)`: whether the ocean draws. The entry is
/// registered, its last read taken, then its test for this frame placed.
pub fn visible(mem: &mut dyn GuestMemory, host: &dyn flare::Host, state: &mut flare::State, definition: u32) -> Result<bool, Cause> {
    let view = word(&*mem, VIEW)?;
    let key = Key { view, class: CLASS, ..Key::default() };
    let Some(index) = flare::register(mem, state, key, &definition.to_le_bytes())? else { return Ok(false) };
    let at = fl::entry_addr(index);
    // 0x0003c2c7: the read set's bit and the byte, for an active entry
    // with the array on.
    let mut read = None;
    if mem.u8(fl::ON).map_err(|_| "unread")? != 0 && word(&*mem, at + 4)? & 2 != 0 {
        // `idiv`: the signed remainder, the word at `[set * 16 + index >>
        // 5]` of the bitmaps.
        let set = (word(&*mem, fl::FRAME)? as i32).wrapping_add(1).wrapping_rem(3);
        let bits = fl::ENDED.wrapping_add((set.wrapping_mul(16).wrapping_add((index >> 5) as i32) as u32).wrapping_mul(4));
        let ended = word(&*mem, bits)? & (1 << (index & 31)) != 0;
        let byte = f32::from(mem.u8(at + 3).map_err(|_| "unread")?);
        let x = byte * float(&*mem, BYTE_SCALE)?;
        let one = float(&*mem, ONE)?;
        // `comiss`: below 0 is 0; above one (ordered) is one.
        let fraction = if 0.0 > x { 0.0 } else if x > one { one } else { x };
        read = Some((ended, fraction));
    }
    let listed = [Listed { index, test: test(&*mem)?, area: CALLBACK_AREA.min(0xffff) as u16 }];
    flare::walk_listed(mem, host, state, view, MASK, &listed)?;
    let seen = float(&*mem, SEEN)?;
    Ok(match read {
        // `comiss fraction, seen; ja`: unordered closes too.
        Some((true, fraction)) => fraction > seen,
        _ => true,
    })
}

#[cfg(test)]
mod tests;
