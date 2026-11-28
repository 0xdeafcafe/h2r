//! The bitmap texture cache's get of an image's texture header in Rust:
//! `0x00012310(eax image, xmm2 lod)` and what it calls, read from guest
//! memory alone. The native fog cuts take the header of a streamed image
//! here (docs/graphics/retire-d3d-fog-fans.md, "Interfaces";
//! docs/graphics/retire-d3d-fog-quad.md).
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x00012310`: an image whose `+0x70` is past `[0x004e6488]` with `+0x50` set gives `+0x50`; else the lookup with flags 3, and, 0, the request with flags 3 | [`header`] |
//! | `0x0012ccf0(esi image, edx flags, xmm1 lod)`: the cached texture of the level the LOD picks, 0 when not loaded | [`lookup`] |
//! | `0x0012ce00(image, flags)` (`xmm1` the LOD): the lookup's level chosen by size, asked of the cache, else the best level loaded, else the type's default texture | [`request`] |
//! | `0x0012c990`: one level's cache entry, loaded or not | [`level_request`] |
//! | `0x0012cb10`: the level the LOD reaches among those with data | [`choose_level`] |
//! | `0x0012c960`, `0x0012c820`: the texture header held in the image (`+0x5c`), built over its pixels | [`inline_header`], [`build_inline`] |
//! | `0x000359e0`: the mip count of a power-of-two size, capped | [`mip_levels`] |
//! | `0x0012ccb0`: the formats the format gate holds back | [`format_check`] |
//!
//! The LOD math is the guest's SSE on f32: `cvtsi2ss`, `mulss`, `addss`
//! in its order, `comiss` as Rust's ordered comparisons (a NaN compares
//! false either way, as the guest's branches take it).
//!
//! A level whose read is queued and not done is not resident yet: the
//! entry's `+3` is 0, a wait or a higher priority raises the read
//! ([`texture_io::raise`], `0x00120ce0`), and the answer is no texture
//! (the held header, the last loaded level, or the type's default stand in,
//! as the guest's request does). [`texture_io::pump`] finishes the read
//! through the kernel's file model.
//!
//! What the request cannot do from guest memory is `Err`:
//!
//! | Cause | Why |
//! |---|---|
//! | [`ALLOCATE_SLOT`] | a level with no cache entry: `0x0012d5a0` takes a slot for it (`0x0013d370`, a ring allocator with an eviction sweep) before it queues the read (`0x00213760`, [`texture_io::queue`]) |
//! | `texture_unread`, `texture_unwritable` | guest memory does not read or write |

use crate::texture_io;
use guest_layout::memory::GuestMemory;

/// Why the cache's get cannot finish in Rust.
pub type Cause = &'static str;

pub const ALLOCATE_SLOT: Cause = "texture_request: 0x0012d5a0 takes a cache slot for the level (the ring allocator 0x0013d370 and its eviction sweep)";
const UNREAD: Cause = "texture_unread";
const UNWRITABLE: Cause = "texture_unwritable";

/// The cache's stamp an image's `+0x70` is held against.
pub const TEXTURE_STAMP: u32 = 0x004e_6488;
/// The textures' entries (`[+0x44]`, 40 bytes each: `+3` loaded, `+8`
/// the read's priority, `+0xc` its request, `+0x14` the header).
pub const TEXTURES: u32 = 0x004e_6454;
/// The resource cache whose entries (`[[+0x64] + 0x44]`, 24 bytes) are
/// stamped `+0x14` with its frame `+0x38`.
pub const TEXTURE_RESOURCES: u32 = 0x004e_6464;
/// The byte that enables the format check `0x0012ccb0`.
pub const FORMAT_GATE: u32 = 0x004e_647b;
/// The level bounds (`[0x0045dc0c]` 2.0, `[0x0045dbc0]` 1.0).
pub const LEVEL_TWO: u32 = 0x0045_dc0c;
pub const LEVEL_ONE: u32 = 0x0045_dbc0;
/// The byte that biases the LOD by the image's `+0x16`: times
/// `[0x0045dc7c]`, plus `[0x004e647c]`.
pub const LOD_BIAS_ON: u32 = 0x0046_8841;
pub const LOD_STEP: u32 = 0x0045_dc7c;
pub const LOD_BIAS: u32 = 0x004e_647c;
/// The request's LOD overrides: `[0x0046883d]` zeroes it, then
/// `[0x0046883c]` makes it `[0x0045dbc0]`.
pub const LOD_ZERO: u32 = 0x0046_883d;
pub const LOD_ONE: u32 = 0x0046_883c;
/// The byte that enables the header held in the image (`+0x5c`).
pub const INLINE_ON: u32 = 0x004e_6468;
/// The base added to an image's `+0x60` for its pixels.
pub const PIXEL_BASE: u32 = 0x004e_646c;
/// The bitmap format's D3D format, by format word.
pub const FORMATS: u32 = 0x0045_0768;
/// The pins: a count `[0x004e6470]` above 0 with mode `[0x004e6474]` 0
/// (or 1 for format 3) makes the request fall back to a default.
pub const PIN_COUNT: u32 = 0x004e_6470;
pub const PIN_MODE: u32 = 0x004e_6474;
/// The I/O load: `[0x004e28f0]` above `0x19` or `[0x004e6484]` at least
/// 10 holds back a request at priority 2 or below that does not wait.
pub const IO_LOAD: u32 = 0x004e_28f0;
pub const REQUESTS: u32 = 0x004e_6484;
/// The default textures by image type (2D, 3D, cube), and the byte the
/// request sets when it returns a default or an allocation failed.
pub const DEFAULTS: [u32; 3] = [0x0048_5ae4, 0x0048_5ae8, 0x0048_5aec];
pub const DEFAULTED: u32 = 0x004e_647a;

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(|_| UNREAD)
}

fn half(mem: &dyn GuestMemory, at: u32) -> Result<u16, Cause> {
    mem.u16(at).map_err(|_| UNREAD)
}

fn byte(mem: &dyn GuestMemory, at: u32) -> Result<u8, Cause> {
    mem.u8(at).map_err(|_| UNREAD)
}

fn float(mem: &dyn GuestMemory, at: u32) -> Result<f32, Cause> {
    mem.f32(at).map_err(|_| UNREAD)
}

fn set(mem: &mut dyn GuestMemory, at: u32, v: u32) -> Result<(), Cause> {
    mem.set_u32(at, v).map_err(|_| UNWRITABLE)
}

/// The image's level `n` slot at `base` (`+0x1c` size code, `+0x28`
/// handle, `+0x34` byte size).
fn slot(image: u32, base: u32, n: u32) -> u32 {
    image.wrapping_add(base + 4 * n)
}

/// `[cache + 0x64] + 0x44`'s entry `index` (24 bytes) stamped `+0x14`
/// with `[cache + 0x38]`.
fn touch(mem: &mut dyn GuestMemory, cache: u32, index: u32) -> Result<(), Cause> {
    let entries = word(&*mem, word(&*mem, cache.wrapping_add(0x64))?.wrapping_add(0x44))?;
    let frame = word(&*mem, cache.wrapping_add(0x38))?;
    set(mem, entries.wrapping_add((index & 0xffff) * 0x18).wrapping_add(0x14), frame)
}

/// The texture entry of `handle`, its resource entry touched.
fn entry(mem: &mut dyn GuestMemory, handle: u32) -> Result<u32, Cause> {
    let index = handle & 0xffff;
    let at = word(&*mem, word(&*mem, TEXTURES)?.wrapping_add(0x44))?.wrapping_add(index * 40);
    let resources = word(&*mem, TEXTURE_RESOURCES)?;
    touch(mem, resources, index)?;
    Ok(at)
}

fn loaded(mem: &dyn GuestMemory, entry: u32) -> Result<bool, Cause> {
    Ok(byte(mem, entry.wrapping_add(3))? != 0)
}

/// `0x0012ccb0(ecx format)`: the formats its jump table at `0x0012ccd4`
/// answers 1 for (1..=11 and 17..=18).
pub fn format_check(format: u8) -> bool {
    matches!(format, 1..=11 | 17..=18)
}

/// The LOD with `[0x00468841]`'s bias: `((f32)+0x16 * [0x0045dc7c] +
/// lod) + [0x004e647c]`, else the LOD.
pub fn biased(mem: &dyn GuestMemory, image: u32, lod: f32) -> Result<f32, Cause> {
    if byte(mem, LOD_BIAS_ON)? == 0 {
        return Ok(lod);
    }
    let steps = i32::from(byte(mem, image.wrapping_add(0x16))?) as f32;
    Ok(steps * float(mem, LOD_STEP)? + lod + float(mem, LOD_BIAS)?)
}

/// `0x000359e0(ecx width, [esp+4] height, eax depth, [esp+8] format,
/// [esp+0xc] flag, [esp+0x10] cap)`: the floor log2 of the largest side
/// (a quarter of width and height for formats 14..=16), 0 for a side
/// not a power of two or the flag set; at most `cap` (16-bit signed).
pub fn mip_levels(width: i16, height: i16, depth: i16, format: i16, flag: u8, cap: i16) -> i16 {
    let (w, h, d) = (i32::from(width), i32::from(height), i32::from(depth));
    let pow2 = |v: i32| v & v.wrapping_sub(1) == 0;
    let levels = if !pow2(w) || !pow2(h) || !pow2(d) || flag != 0 {
        0
    } else {
        let side = if (14..=16).contains(&format) { (w / 4).max(h / 4).max(d) } else { i32::from(width.max(height).max(depth)) };
        log2(side as u32) as i16
    };
    levels.min(cap)
}

/// `0x0013e3a0`: the shifts that bring a value above 1 down to 1; 0 for
/// 0 and 1.
fn log2(mut v: u32) -> u32 {
    let mut n = 0;
    if v > 1 {
        while v != 1 {
            v >>= 1;
            n += 1;
        }
    }
    n
}

/// `0x0012c820(esi image)`: the texture header at `+0x5c` over the
/// image's pixels: `+0x5c` `0x40001`, `+0x60` the pixels (`+0x60` plus
/// `[0x004e646c]`, low 28 bits), `+0x64` 0, `+0x68` the format word,
/// `+0x6c` 0.
pub fn build_inline(mem: &mut dyn GuestMemory, image: u32) -> Result<(), Cause> {
    let pixels = word(&*mem, image.wrapping_add(0x60))?.wrapping_add(word(&*mem, PIXEL_BASE)?);
    let kind = half(&*mem, image.wrapping_add(0x68))? as i16;
    let mut format = word(&*mem, FORMATS.wrapping_add((i32::from(kind) as u32).wrapping_mul(4)))?;
    let flags = byte(&*mem, image.wrapping_add(0xe))?;
    if flags & 0x20 != 0 && (kind == 0xa || kind == 0xb) {
        format = 0x33;
    }
    let width = half(&*mem, image.wrapping_add(0x6a))? as i16;
    let height = half(&*mem, image.wrapping_add(0x6c))? as i16;
    let depth = half(&*mem, image.wrapping_add(0x6e))? as i16;
    let levels = mip_levels(width, height, depth, kind, (flags >> 4) & 1, 0);
    let ty = half(&*mem, image.wrapping_add(0xa))?;
    let dimension = if ty == 1 { 3 } else { 2 };
    let base = if ty == 2 { 0xd } else { 9 };
    let side = |v: i16| log2(i32::from(v) as u32);
    let format_word = base
        | (((dimension | (format << 4)) << 4) | ((i32::from(levels) as u32) << 16).wrapping_add(0x10000))
        | (side(width) << 20)
        | (side(height) << 24)
        | (side(depth) << 28);
    set(mem, image.wrapping_add(0x64), 0)?;
    set(mem, image.wrapping_add(0x5c), 0x0004_0001)?;
    set(mem, image.wrapping_add(0x68), format_word)?;
    set(mem, image.wrapping_add(0x6c), 0)?;
    set(mem, image.wrapping_add(0x60), pixels & 0x0fff_ffff)
}

/// `0x0012c960(eax image)`: with `[0x004e6468]` set and the image
/// holding a header (`+0x5c` non-zero, or `+0x64` above 0), that header
/// `image + 0x5c`, built first unless `+0x5c` is above 0; else 0.
pub fn inline_header(mem: &mut dyn GuestMemory, image: u32) -> Result<u32, Cause> {
    if byte(&*mem, INLINE_ON)? == 0 {
        return Ok(0);
    }
    let common = word(&*mem, image.wrapping_add(0x5c))? as i32;
    if common == 0 && word(&*mem, image.wrapping_add(0x64))? as i32 <= 0 {
        return Ok(0);
    }
    if common <= 0 {
        build_inline(mem, image)?;
    }
    Ok(image.wrapping_add(0x5c))
}

/// `0x0012ccf0(esi image, edx flags, xmm1 lod)`: the image's texture, 0
/// when not loaded. Flag bit 2 zeroes the LOD and skips the held header.
pub fn lookup(mem: &mut dyn GuestMemory, image: u32, flags: u32, lod: f32) -> Result<u32, Cause> {
    let no_level = flags & 4 != 0;
    let mut level = biased(&*mem, image, lod)?;
    if no_level {
        level = 0.0;
    }
    let mut texture = 0;
    if byte(&*mem, image.wrapping_add(0xf))? & 2 == 0 {
        texture = word(&*mem, image.wrapping_add(0x50))?;
    } else if byte(&*mem, FORMAT_GATE)? != 0 && format_check(byte(&*mem, image.wrapping_add(0x17))?) {
        if !no_level {
            texture = inline_header(mem, image)?;
        }
    } else if float(&*mem, image.wrapping_add(0x58))? > level {
        let n = if level >= float(&*mem, LEVEL_TWO)? {
            2
        } else if level >= float(&*mem, LEVEL_ONE)? {
            1
        } else {
            0
        };
        let handle = word(&*mem, slot(image, 0x28, n))?;
        if handle != u32::MAX {
            let entry = entry(mem, handle)?;
            if loaded(&*mem, entry)? {
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

/// `0x0012cb10(edx image, xmm0 lod)`: the last level, of those with a
/// size code, a byte size, and above `0x400` bytes past level 0, whose
/// bound (0, `[0x0045dbc0]`, `[0x0045dc0c]`) the LOD reaches.
pub fn choose_level(mem: &dyn GuestMemory, image: u32, lod: f32) -> Result<u32, Cause> {
    let bounds = [0.0, float(mem, LEVEL_ONE)?, float(mem, LEVEL_TWO)?];
    let mut level = 0;
    for n in 0..3 {
        if word(mem, slot(image, 0x1c, n))? == u32::MAX {
            continue;
        }
        let size = word(mem, slot(image, 0x34, n))?;
        if size == 0 || (n != 0 && size as i32 <= 0x400) {
            continue;
        }
        if bounds[n as usize] > lod {
            break;
        }
        level = n;
    }
    Ok(level)
}

/// How [`level_request`] asks: `0x0012c990`'s CL, EBX and its first two
/// stack bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ask {
    /// Flag bit 1: ask the cache, not only look.
    pub queue: bool,
    /// Wait for the read.
    pub wait: bool,
    /// The pins' fall-back (`0x0012d5a0`'s second byte).
    pub fallback: bool,
    /// The read's priority.
    pub priority: u32,
}

/// `0x0012c990(eax level, image, ...)`: level `n`'s loaded texture
/// (`entry + 0x14`), else 0. `Err` where it would queue, raise or wait
/// for a read.
pub fn level_request(mem: &mut dyn GuestMemory, image: u32, n: u32, ask: Ask) -> Result<u32, Cause> {
    let busy = word(&*mem, IO_LOAD)? as i32 > 0x19 || word(&*mem, REQUESTS)? as i32 >= 10;
    if !ask.queue || (!ask.wait && ask.priority <= 2 && busy) {
        let handle = word(&*mem, slot(image, 0x28, n))?;
        if handle == u32::MAX {
            return Ok(0);
        }
        let entry = entry(mem, handle)?;
        return Ok(if loaded(&*mem, entry)? { entry.wrapping_add(0x14) } else { 0 });
    }
    if word(&*mem, slot(image, 0x1c, n))? == u32::MAX {
        return Ok(0);
    }
    let size = word(&*mem, slot(image, 0x34, n))?;
    if size == 0 || (n != 0 && size as i32 <= 0x400) {
        return Ok(0);
    }
    let handle = word(&*mem, slot(image, 0x28, n))?;
    if handle == u32::MAX {
        return Err(ALLOCATE_SLOT);
    }
    let entry = entry(mem, handle)?;
    if loaded(&*mem, entry)? {
        return Ok(entry.wrapping_add(0x14));
    }
    // The read is queued (`entry + 0xc` its id) and not done: the level
    // is not resident yet. A wait raises it to priority 7 (`0x0012ca6f`)
    // and does not block; a higher priority than the read's is stored
    // and raised (`0x0012ca9c`). [`crate::texture_io::pump`] finishes it.
    let id = word(&*mem, entry.wrapping_add(0xc))?;
    if ask.wait {
        texture_io::raise(mem, id, 7)?;
    } else if ask.priority as i32 > word(&*mem, entry.wrapping_add(8))? as i32 {
        set(mem, entry.wrapping_add(8), ask.priority)?;
        texture_io::raise(mem, id, ask.priority)?;
    }
    Ok(0)
}

/// `0x0012ce00(image, flags)` with `xmm1` the LOD: the texture to bind.
/// Flags: bit 0 wait, bit 1 ask the cache, bit 2 look only, bit 3
/// priority 5, bit 4 priority 1.
pub fn request(mem: &mut dyn GuestMemory, image: u32, flags: u32, lod: f32) -> Result<u32, Cause> {
    let look_only = flags & 4 != 0;
    let mut level_lod = biased(&*mem, image, lod)?;
    if byte(&*mem, LOD_ZERO)? != 0 {
        level_lod = 0.0;
    }
    if byte(&*mem, LOD_ONE)? != 0 {
        level_lod = float(&*mem, LEVEL_ONE)?;
    }
    if byte(&*mem, image.wrapping_add(0xf))? & 2 == 0 {
        return word(&*mem, image.wrapping_add(0x50));
    }
    let mut wait = flags & 1 != 0;
    let mut fallback = false;
    let mut held = 0;
    if !look_only {
        held = inline_header(mem, image)?;
        let pins = word(&*mem, PIN_COUNT)? as i32;
        let pinned = pins > 0 && {
            let mode = word(&*mem, PIN_MODE)?;
            mode == 0 || (mode == 1 && byte(&*mem, image.wrapping_add(0x17))? == 3)
        };
        if pinned {
            fallback = true;
            wait = pins == 1;
        } else if held != 0 && (byte(&*mem, image.wrapping_add(0xe))? as i8) >= 0 {
            wait = false;
        }
    }
    let enough = if byte(&*mem, FORMAT_GATE)? != 0 && format_check(byte(&*mem, image.wrapping_add(0x17))?) {
        true
    } else {
        lod >= float(&*mem, image.wrapping_add(0x58))?
    };
    if enough && held != 0 {
        return Ok(held);
    }
    let level = if look_only { 0 } else { choose_level(&*mem, image, level_lod)? };
    let priority = if fallback {
        2
    } else if wait {
        7
    } else if flags & 8 != 0 {
        5
    } else if flags & 0x10 != 0 {
        1
    } else {
        2
    };
    let got = level_request(mem, image, level, Ask { queue: flags & 2 != 0, wait, fallback, priority })?;
    if got != 0 || look_only {
        return Ok(if got != 0 { got } else { held });
    }
    // `0x0012cfb6`: the last level loaded, else the held header.
    let mut texture = held;
    for n in 0..3 {
        let handle = word(&*mem, slot(image, 0x28, n))?;
        if handle == u32::MAX {
            continue;
        }
        let entry = entry(mem, handle)?;
        if loaded(&*mem, entry)? && entry.wrapping_add(0x14) != 0 {
            texture = entry.wrapping_add(0x14);
            fallback = false;
        }
    }
    if !fallback || texture != 0 {
        return Ok(texture);
    }
    let ty = half(&*mem, image.wrapping_add(0xa))? as i16;
    let default = DEFAULTS[match ty {
        0 => 0,
        1 => 1,
        _ => 2,
    }];
    let texture = word(&*mem, default)?;
    mem.set_u8(DEFAULTED, 1).map_err(|_| UNWRITABLE)?;
    Ok(texture)
}

/// `0x00012310(eax image, xmm2 lod)`: the image's texture header to
/// bind, as `0x000143c0` asks for it.
pub fn header(mem: &mut dyn GuestMemory, image: u32, lod: f32) -> Result<u32, Cause> {
    if word(&*mem, image.wrapping_add(0x70))? as i32 > word(&*mem, TEXTURE_STAMP)? as i32 {
        let held = word(&*mem, image.wrapping_add(0x50))?;
        if held != 0 {
            return Ok(held);
        }
    }
    match lookup(mem, image, 3, lod)? {
        0 => request(mem, image, 3, lod),
        texture => Ok(texture),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const IMAGE: u32 = 0x0050_0000;
    const ENTRIES: u32 = 0x0051_1000;
    const RESOURCE_ENTRIES: u32 = 0x0052_1000;
    const FRAME: u32 = 77;
    const STAMP: u32 = 10;

    /// A streamed image (`+0x0f` bit 1) at the cache's stamp, no level
    /// with data, the caches empty.
    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
        let mut set = |at: u32, v: u32| m.set_u32(at, v).unwrap();
        set(TEXTURE_STAMP, STAMP);
        set(TEXTURES, 0x0051_0000);
        set(0x0051_0044, ENTRIES);
        set(TEXTURE_RESOURCES, 0x0052_0000);
        set(0x0052_0064, 0x0052_0100);
        set(0x0052_0144, RESOURCE_ENTRIES);
        set(0x0052_0038, FRAME);
        set(IMAGE + 0xc, 0x0200_0000);
        set(IMAGE + 0x70, STAMP);
        for n in 0..3 {
            set(slot(IMAGE, 0x1c, n), u32::MAX);
            set(slot(IMAGE, 0x28, n), u32::MAX);
        }
        m.set_f32(LEVEL_ONE, 1.0).unwrap();
        m.set_f32(LEVEL_TWO, 2.0).unwrap();
        m.set_f32(IMAGE + 0x58, 4.0).unwrap();
        texture_io::init(&mut m);
        m
    }

    fn entry_at(index: u32) -> u32 {
        ENTRIES + index * 40
    }

    /// Level `n` with data of `size` bytes in cache entry `index`.
    fn level(m: &mut FlatMemory, n: u32, index: u32, size: u32, is_loaded: bool) {
        m.set_u32(slot(IMAGE, 0x1c, n), 0).unwrap();
        m.set_u32(slot(IMAGE, 0x34, n), size).unwrap();
        m.set_u32(slot(IMAGE, 0x28, n), 0xabcd_0000 | index).unwrap();
        m.set_u8(entry_at(index) + 3, u8::from(is_loaded)).unwrap();
    }

    /// Level `n` with data in cache entry `index` whose read is queued at
    /// `priority` and not done; its id.
    fn loading(m: &mut FlatMemory, n: u32, index: u32, size: u32, priority: u32) -> u32 {
        level(m, n, index, size, false);
        let read = texture_io::Request {
            handle: 1,
            offset: 0x1000 * index,
            size,
            dest: 0x0054_0000,
            loaded_at: entry_at(index) + 3,
            priority,
        };
        let id = texture_io::queue(m, read).unwrap().unwrap();
        m.set_u32(entry_at(index) + 0xc, id).unwrap();
        m.set_u32(entry_at(index) + 8, priority).unwrap();
        id
    }

    /// The priority of the queued read `id`.
    fn priority_of(m: &FlatMemory, id: u32) -> u32 {
        m.u32(texture_io::NODES + (id & 0xff) * texture_io::NODE_SIZE).unwrap()
    }

    /// The header the image holds, over 64x32x1 pixels of format word 5.
    fn holding(m: &mut FlatMemory) {
        m.set_u8(INLINE_ON, 1).unwrap();
        m.set_u32(IMAGE + 0x64, 1).unwrap();
        m.set_u32(IMAGE + 0x60, 0x10).unwrap();
        m.set_u32(PIXEL_BASE, 0x8000_0000).unwrap();
        m.set_u16(IMAGE + 0x68, 5).unwrap();
        m.set_u32(FORMATS + 4 * 5, 0xc).unwrap();
        m.set_u16(IMAGE + 0x6a, 64).unwrap();
        m.set_u16(IMAGE + 0x6c, 32).unwrap();
        m.set_u16(IMAGE + 0x6e, 1).unwrap();
    }

    #[test]
    fn a_header_past_the_stamp_is_taken_at_once() {
        let mut m = mem();
        m.set_u32(IMAGE + 0x70, STAMP + 1).unwrap();
        m.set_u32(IMAGE + 0x50, 0x1234).unwrap();
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(0x1234));
    }

    #[test]
    fn an_image_not_streamed_gives_its_header() {
        let mut m = mem();
        m.set_u32(IMAGE + 0xc, 0).unwrap();
        m.set_u32(IMAGE + 0x50, 0x99).unwrap();
        assert_eq!(lookup(&mut m, IMAGE, 3, 0.0), Ok(0x99));
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(0x99));
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(0x99));
    }

    #[test]
    fn a_loaded_level_0_is_held_and_its_entries_stamped() {
        let mut m = mem();
        level(&mut m, 0, 3, 0x100, true);
        let texture = entry_at(3) + 0x14;
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(texture));
        assert_eq!(m.u32(IMAGE + 0x70).unwrap(), STAMP + 2);
        assert_eq!(m.u32(IMAGE + 0x50).unwrap(), texture);
        assert_eq!(m.u32(RESOURCE_ENTRIES + 3 * 0x18 + 0x14).unwrap(), FRAME);
    }

    #[test]
    fn the_lod_bias_picks_a_level_not_held() {
        let mut m = mem();
        m.set_u8(LOD_BIAS_ON, 1).unwrap();
        m.set_u8(IMAGE + 0x16, 2).unwrap();
        m.set_f32(LOD_STEP, 0.5).unwrap();
        m.set_f32(LOD_BIAS, 0.25).unwrap();
        level(&mut m, 1, 4, 0x800, true);
        // 2 * 0.5 + 0.5 + 0.25 = 1.75: level 1, not held.
        assert_eq!(lookup(&mut m, IMAGE, 3, 0.5), Ok(entry_at(4) + 0x14));
        assert_eq!(m.u32(IMAGE + 0x50).unwrap(), 0);
        assert_eq!(m.u32(IMAGE + 0x70).unwrap(), STAMP);
        // Flag bit 2 (the ocean's 0xe) zeroes the LOD: level 0 has none.
        assert_eq!(lookup(&mut m, IMAGE, 0xe, 0.5), Ok(0));
    }

    #[test]
    fn the_bias_adds_in_the_guests_order() {
        let mut m = mem();
        m.set_u8(LOD_BIAS_ON, 1).unwrap();
        m.set_u8(IMAGE + 0x16, 1).unwrap();
        m.set_f32(LOD_STEP, 1.0).unwrap();
        m.set_f32(LOD_BIAS, -1.0e8).unwrap();
        // (1 + 1e8) rounds to 1e8 in f32 before the bias.
        assert_eq!(biased(&m, IMAGE, 1.0e8).map(f32::to_bits), Ok(0.0f32.to_bits()));
        m.set_u8(LOD_BIAS_ON, 0).unwrap();
        assert_eq!(biased(&m, IMAGE, 3.5), Ok(3.5));
    }

    #[test]
    fn a_nan_lod_reaches_no_level() {
        let mut m = mem();
        level(&mut m, 0, 3, 0x100, true);
        assert_eq!(lookup(&mut m, IMAGE, 3, f32::NAN), Ok(0));
        assert_eq!(choose_level(&m, IMAGE, f32::NAN), Ok(0));
    }

    #[test]
    fn the_format_gate_builds_the_held_header() {
        let mut m = mem();
        holding(&mut m);
        m.set_u8(FORMAT_GATE, 1).unwrap();
        m.set_u8(IMAGE + 0x17, 1).unwrap();
        assert_eq!(lookup(&mut m, IMAGE, 0xe, 0.0), Ok(0));
        assert_eq!(m.u32(IMAGE + 0x5c).unwrap(), 0);
        assert_eq!(lookup(&mut m, IMAGE, 3, 0.0), Ok(IMAGE + 0x5c));
        assert_eq!(m.u32(IMAGE + 0x5c).unwrap(), 0x0004_0001);
        assert_eq!(m.u32(IMAGE + 0x60).unwrap(), 0x10);
        assert_eq!(m.u32(IMAGE + 0x64).unwrap(), 0);
        // DMA 1, border, 2D, format 0xc, one level, 2^6 by 2^5 by 2^0.
        assert_eq!(m.u32(IMAGE + 0x68).unwrap(), 0x0561_0c29);
        assert_eq!(m.u32(IMAGE + 0x6c).unwrap(), 0);
        // Built once: `+0x5c` above 0 is kept.
        m.set_u32(IMAGE + 0x68, 0xdead).unwrap();
        assert_eq!(inline_header(&mut m, IMAGE), Ok(IMAGE + 0x5c));
        assert_eq!(m.u32(IMAGE + 0x68).unwrap(), 0xdead);
    }

    #[test]
    fn no_held_header_without_its_byte_or_pixels() {
        let mut m = mem();
        holding(&mut m);
        m.set_u32(IMAGE + 0x64, 0).unwrap();
        assert_eq!(inline_header(&mut m, IMAGE), Ok(0));
        m.set_u32(IMAGE + 0x64, 1).unwrap();
        m.set_u8(INLINE_ON, 0).unwrap();
        assert_eq!(inline_header(&mut m, IMAGE), Ok(0));
    }

    #[test]
    fn a_cube_and_a_volume_and_the_swizzle_override() {
        let mut m = mem();
        holding(&mut m);
        m.set_u16(IMAGE + 0xa, 2).unwrap();
        m.set_u8(IMAGE + 0xe, 0x20).unwrap();
        m.set_u16(IMAGE + 0x68, 0xa).unwrap();
        build_inline(&mut m, IMAGE).unwrap();
        assert_eq!(m.u32(IMAGE + 0x68).unwrap(), 0x0561_0000 | (((2 | (0x33 << 4)) << 4) + 0xd));
        holding(&mut m);
        m.set_u16(IMAGE + 0xa, 1).unwrap();
        m.set_u8(IMAGE + 0xe, 0).unwrap();
        build_inline(&mut m, IMAGE).unwrap();
        assert_eq!(m.u32(IMAGE + 0x68).unwrap(), 0x0561_0c39);
    }

    #[test]
    fn mip_levels_take_the_largest_power_of_two_side() {
        assert_eq!(mip_levels(64, 32, 1, 5, 0, 15), 6);
        assert_eq!(mip_levels(64, 256, 1, 14, 0, 15), 6);
        assert_eq!(mip_levels(64, 256, 1, 5, 0, 15), 8);
        assert_eq!(mip_levels(48, 32, 1, 5, 0, 15), 0);
        assert_eq!(mip_levels(64, 32, 1, 5, 1, 15), 0);
        assert_eq!(mip_levels(64, 32, 1, 5, 0, 3), 3);
        assert_eq!(mip_levels(64, 32, 1, 5, 0, 0), 0);
    }

    #[test]
    fn the_level_is_the_last_the_lod_reaches() {
        let mut m = mem();
        level(&mut m, 0, 1, 0x100, false);
        level(&mut m, 1, 2, 0x400, false);
        level(&mut m, 2, 3, 0x800, false);
        assert_eq!(choose_level(&m, IMAGE, 2.5), Ok(2));
        assert_eq!(choose_level(&m, IMAGE, 1.5), Ok(0));
        m.set_u32(slot(IMAGE, 0x34, 1), 0x401).unwrap();
        assert_eq!(choose_level(&m, IMAGE, 1.5), Ok(1));
    }

    #[test]
    fn a_level_with_no_entry_is_a_read_to_queue() {
        let mut m = mem();
        m.set_u32(slot(IMAGE, 0x1c, 0), 0).unwrap();
        m.set_u32(slot(IMAGE, 0x34, 0), 0x100).unwrap();
        assert_eq!(header(&mut m, IMAGE, 0.0), Err(ALLOCATE_SLOT));
    }

    #[test]
    fn an_entry_loading_is_not_resident_yet_and_its_read_is_raised() {
        let mut m = mem();
        let id = loading(&mut m, 0, 3, 0x100, 2);
        // The wait raises the read to priority 7 and answers no texture.
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(0));
        assert_eq!(priority_of(&m, id), 7);
        assert_eq!(m.u8(entry_at(3) + 3).unwrap(), 0);
    }

    #[test]
    fn a_read_completes_through_the_file_source_and_is_then_resident() {
        let mut m = mem();
        let id = loading(&mut m, 0, 3, 0x100, 2);
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(0));
        let mut file = |handle: u32, offset: u32, out: &mut [u8]| -> Result<usize, texture_io::Cause> {
            assert_eq!((handle, offset, out.len()), (1, 0x3000, 0x100));
            out.fill(0xab);
            Ok(out.len())
        };
        assert_eq!(texture_io::pump(&mut m, &mut file), Ok(1));
        assert!(!texture_io::pending(&m, id).unwrap());
        assert_eq!(m.u8(0x0054_0000).unwrap(), 0xab);
        let texture = entry_at(3) + 0x14;
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(texture));
        assert_eq!(m.u32(IMAGE + 0x50).unwrap(), texture);
    }

    #[test]
    fn an_evicted_read_stays_not_resident() {
        let mut m = mem();
        let id = loading(&mut m, 0, 3, 0x100, 2);
        assert_eq!(texture_io::cancel(&mut m, id), Ok(true));
        assert_eq!(header(&mut m, IMAGE, 0.0), Ok(0));
        let mut file = |_: u32, _: u32, _: &mut [u8]| -> Result<usize, texture_io::Cause> { panic!("no read queued") };
        assert_eq!(texture_io::pump(&mut m, &mut file), Ok(0));
        assert_eq!(m.u8(entry_at(3) + 3).unwrap(), 0);
    }

    #[test]
    fn a_held_header_stands_in_while_the_level_loads() {
        let mut m = mem();
        holding(&mut m);
        m.set_u32(IMAGE + 0x5c, 0x0004_0001).unwrap();
        let id = loading(&mut m, 0, 3, 0x100, 2);
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(IMAGE + 0x5c));
        assert_eq!(priority_of(&m, id), 2);
        // A lower priority read is raised.
        m.set_u32(entry_at(3) + 8, 1).unwrap();
        m.set_u32(texture_io::NODES + (id & 0xff) * texture_io::NODE_SIZE, 1).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(IMAGE + 0x5c));
        assert_eq!(m.u32(entry_at(3) + 8).unwrap(), 2);
        assert_eq!(priority_of(&m, id), 2);
        // `+0x0e` bit 7 keeps the wait: the read goes to priority 7.
        m.set_u8(IMAGE + 0xe, 0x80).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(IMAGE + 0x5c));
        assert_eq!(priority_of(&m, id), 7);
        // A LOD at the image's bound takes the held header at once.
        m.set_u32(slot(IMAGE, 0x28, 0), u32::MAX).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 4.0), Ok(IMAGE + 0x5c));
    }

    #[test]
    fn the_pins_under_load_fall_back_to_the_types_default() {
        let mut m = mem();
        m.set_u32(PIN_COUNT, 2).unwrap();
        m.set_u32(IO_LOAD, 0x1a).unwrap();
        m.set_u16(IMAGE + 0xa, 1).unwrap();
        m.set_u32(DEFAULTS[1], 0x777).unwrap();
        level(&mut m, 0, 3, 0x100, false);
        m.set_u32(slot(IMAGE, 0x28, 0), u32::MAX).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(0x777));
        assert_eq!(m.u8(DEFAULTED).unwrap(), 1);
    }

    #[test]
    fn the_pins_take_the_last_level_loaded() {
        let mut m = mem();
        m.set_u32(PIN_COUNT, 2).unwrap();
        loading(&mut m, 0, 3, 0x100, 7);
        level(&mut m, 1, 4, 0x800, true);
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(entry_at(4) + 0x14));
        assert_eq!(m.u8(DEFAULTED).unwrap(), 0);
        // A pin mode of 1 pins format 3 alone: the read is waited for (not
        // resident yet) and the loaded level stands in.
        m.set_u32(PIN_MODE, 1).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(entry_at(4) + 0x14));
    }

    #[test]
    fn looking_only_asks_nothing() {
        let mut m = mem();
        holding(&mut m);
        m.set_u32(slot(IMAGE, 0x1c, 0), 0).unwrap();
        m.set_u32(slot(IMAGE, 0x34, 0), 0x100).unwrap();
        assert_eq!(request(&mut m, IMAGE, 4, 0.0), Ok(0));
        assert_eq!(m.u32(IMAGE + 0x5c).unwrap(), 0);
    }

    #[test]
    fn the_lod_overrides_move_the_requests_level() {
        let mut m = mem();
        m.set_u32(PIN_COUNT, 2).unwrap();
        level(&mut m, 0, 3, 0x100, true);
        let id = loading(&mut m, 1, 4, 0x800, 7);
        // LOD 1 by `[0x0046883c]`: level 1 is asked and the loaded level
        // 0 is taken after.
        m.set_u8(LOD_ONE, 1).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(entry_at(3) + 0x14));
        m.set_u32(entry_at(4) + 8, 1).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(entry_at(3) + 0x14));
        assert_eq!(m.u32(entry_at(4) + 8).unwrap(), 2);
        assert_eq!(priority_of(&m, id), 2);
        m.set_u8(LOD_ONE, 0).unwrap();
        assert_eq!(request(&mut m, IMAGE, 3, 0.0), Ok(entry_at(3) + 0x14));
    }
}
