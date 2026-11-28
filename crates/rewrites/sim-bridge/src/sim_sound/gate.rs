//! `0x126c30`: the start gate, whether a sound request starts at all and
//! which listener hears it (docs/engine/sound-instances.md, "The start
//! layer"). It reads the tag, the gestalt, the class rows, the listeners
//! and the BSP's cluster distances, and writes only the random seed.
//!
//! | Guest | Rust |
//! |---|---|
//! | `0x126c30` | [`start_gate`] |
//! | `0x126bd0` | [`class_plays`] |
//! | `0x124fc0`, `0x125010` | [`distances`] |
//! | `0x127d00` | [`select_listener`] |
//! | `0x127e20` | [`source_distance`] |
//! | `0x221da0` | [`obstructed`] |
//! | `0x249d60`, `0x249ed0` | [`cluster_distance`] |
//! | `0x12ac20`, `0x12aff0` | [`distance_gain`], [`ramp`] |
//! | `0x1251e0`, `0x2197f0` | [`super::levels::GuestLevels`], [`sim_sound::levels::to_decibels`] |
//!
//! The x87 sequences run on [`X87`] under [`FPU_CONTROL`]; the SSE ones
//! round each operation in binary32 with every exception masked.

use std::cmp::Ordering;

use super::levels::{GuestLevels, class_row};
use super::playback::{GESTALT, definition};
use engine::memory::{GuestMemory, MemFault};
use engine::points::{PLAYER_GLOBALS, RANDOM_SEEDS, SOUND_GLOBALS};
use sim_core::det::X87;
use sim_sound::environment::ClusterPair;
use sim_sound::instances::{FPU_CONTROL, SourceParams, TagIndex};
use sim_sound::levels::{Constants, to_decibels};
use x87::abi::{self as fp, X87Abi};

const NONE: u32 = u32::MAX;
/// `[0x47f0e4]`: the second test of a class whose `+4` is 2.
pub const CLASS_SWITCH: u32 = 0x0047_f0e4;
/// `[0x4e0348]`: the running structure BSP.
pub const BSP: u32 = 0x004e_0348;
/// `0x45dc50`: the seed's scale to `[0, 1)`.
pub const SEED_SCALE: u32 = 0x0045_dc50;
/// `0x44a0b4`: the dB floor (-64).
pub const FLOOR: u32 = 0x0044_a0b4;
/// `0x45dc70`: the shortest minimum distance (0.001).
pub const SHORTEST: u32 = 0x0045_dc70;
/// `0x45dbdc`: bounds this close share one distance.
pub const CLOSE: u32 = 0x0045_dbdc;
/// `0x45dbc0`: one.
pub const ONE: u32 = 0x0045_dbc0;
/// `0x44aed4`: the distance of a sound no listener hears.
pub const FAR: u32 = 0x0044_aed4;
/// `0x45dd5c`: a cluster distance that blocks.
pub const OBSTRUCTION_LIMIT: u32 = 0x0045_dd5c;
/// `0x45ddc4`, `0x45ddc8`: a cluster distance byte's scales.
pub const COARSE_STEP: u32 = 0x0045_ddc4;
pub const FINE_STEP: u32 = 0x0045_ddc8;
/// Sound globals `+0x88`: four listeners, `0x48` bytes each.
const LISTENERS: u32 = 0x88;
const LISTENER: u32 = 0x48;
const USERS: u32 = 4;
/// Gestalt `+4`: the class defaults, `0x38` bytes a row (tag `+6`).
const DEFAULT_ROW: u32 = 0x38;

/// Why the gate refused (the word it writes through its third argument).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Refusal {
    /// The sound is off, or the class does not play.
    Off = 5,
    /// The skip chance drew it.
    Skipped = 1,
    /// No listener hears it, or it is under the floor.
    Inaudible = 4,
}

/// The gate's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gate {
    /// `None`: the sound starts.
    pub refused: Option<Refusal>,
    /// The listener `0x127d00` chose, written through the second argument
    /// when the gate reached it (`-1` for none).
    pub listener: Option<u32>,
}

struct Sse(u32);

impl Sse {
    fn new() -> Self {
        Self(0x1f80)
    }
    fn op(&mut self, f: fn(&mut u32, u128, u32) -> Result<u128, simd::Xm>, a: u32, b: u32) -> u32 {
        f(&mut self.0, u128::from(a), b).expect("masked sound SSE") as u32
    }
    fn add(&mut self, a: u32, b: u32) -> u32 {
        self.op(simd::addss, a, b)
    }
    fn sub(&mut self, a: u32, b: u32) -> u32 {
        self.op(simd::subss, a, b)
    }
    fn mul(&mut self, a: u32, b: u32) -> u32 {
        self.op(simd::mulss, a, b)
    }
    fn div(&mut self, a: u32, b: u32) -> u32 {
        self.op(simd::divss, a, b)
    }
    fn int(&mut self, i: u32) -> u32 {
        simd::cvtsi2ss(&mut self.0, 0, i).expect("masked sound SSE") as u32
    }
    /// `comiss a, b; ja`: ordered and greater.
    fn above(&mut self, a: u32, b: u32) -> bool {
        simd::comiss(&mut self.0, a, b).expect("masked sound SSE") & 0x41 == 0
    }
    /// `ucomiss a, b`, then `ZF` without `PF`: ordered and equal.
    fn equal(&mut self, a: u32, b: u32) -> bool {
        simd::ucomiss(&mut self.0, a, b).expect("masked sound SSE") & 0x44 == 0x40
    }
}

/// Byte 3's low nibble, sign-extended: 0 none, 1 positioned, else a
/// direction.
fn kind(m: &dyn GuestMemory, source: u32) -> Result<i32, MemFault> {
    Ok(i32::from((m.u8(source.wrapping_add(3))? << 4) as i8) >> 4)
}

/// A tag's class allows a start: `+5` 1 with `+4` not 2, or `+4` 2 with
/// `[0x47f0e4]` set.
fn class_allows(m: &dyn GuestMemory, def: u32) -> Result<bool, MemFault> {
    let (four, five) = (m.u8(def.wrapping_add(4))?, m.u8(def.wrapping_add(5))?);
    if five == 1 && four != 2 {
        return Ok(true);
    }
    Ok(four == 2 && m.u32(CLASS_SWITCH)? != 0)
}

/// The class default row of a definition (gestalt `+4`, tag `+6`).
fn default_row(m: &dyn GuestMemory, def: u32) -> Result<u32, MemFault> {
    let index = i32::from(m.u16(def.wrapping_add(6))? as i16);
    Ok(m.u32(m.u32(GESTALT)?.wrapping_add(4))?.wrapping_add((index as u32).wrapping_mul(DEFAULT_ROW)))
}

/// `0x126bd0(eax = tag)`: the class allows it, the tag's `+0xa` is set,
/// and its gestalt row (`+0x24`, 12 bytes, tag `+8`) has a word `+0xa`.
pub fn class_plays(m: &dyn GuestMemory, tag: u32) -> Result<bool, MemFault> {
    let def = definition(m, tag)?;
    if !class_allows(m, def)? || m.u8(def.wrapping_add(0xa))? == 0 {
        return Ok(false);
    }
    let index = i32::from(m.u16(def.wrapping_add(8))? as i16);
    let rows = m.u32(m.u32(GESTALT)?.wrapping_add(0x24))?;
    Ok(m.u16(rows.wrapping_add((index as u32).wrapping_mul(12)).wrapping_add(0xa))? != 0)
}

/// `0x124fc0`, `0x125010` (`eax = tag`, `ecx = source`): the minimum and
/// maximum distance as bits. Source word 0 bit 2 (bit 3) takes word 13
/// (14); a tag flagged `+1` bit 2 (bit 3) takes its class's `+0x18`
/// (`+0x1c`); else the class default row's `+0` (`+4`).
pub fn distances(m: &dyn GuestMemory, tag: u32, source: u32) -> Result<[u32; 2], MemFault> {
    let word0 = m.u8(source)?;
    let def = definition(m, tag)?;
    let flags = m.u8(def.wrapping_add(1))?;
    let class = i32::from(m.u8(def.wrapping_add(2))? as i8);
    let mut out = [0; 2];
    for (i, (bit, at, class_at)) in [(2, 0x34, 0x18), (3, 0x38, 0x1c)].into_iter().enumerate() {
        out[i] = if word0 & (1 << bit) != 0 {
            m.u32(source.wrapping_add(at))?
        } else if flags & (1 << bit) != 0 {
            m.u32(class_row(m, class)?.wrapping_add(class_at))?
        } else {
            m.u32(default_row(m, def)?.wrapping_add(4 * i as u32))?
        };
    }
    Ok(out)
}

/// The `n`th listener's row in the sound globals.
fn listener_row(m: &dyn GuestMemory, n: u32) -> Result<u32, MemFault> {
    Ok(m.u32(SOUND_GLOBALS.base)?.wrapping_add(LISTENERS).wrapping_add(n.wrapping_mul(LISTENER)))
}

/// `0x127e20(ecx = source, edx = listener)`: the source's distance to the
/// listener as bits, squared for a positioned source. A positioned source
/// with `+2` set is a column (top `+0x30`): its flat distance while the
/// listener is inside it, else [`FAR`]. A directional source answers the
/// length of its vector (`+0xc`), on the x87.
pub fn source_distance(m: &dyn GuestMemory, source: u32, listener: u32) -> Result<u32, MemFault> {
    let at = |o: u32| m.u32(source.wrapping_add(o));
    let mut s = Sse::new();
    match kind(m, source)? {
        0 => Ok(0),
        1 => {
            let row = listener_row(m, listener)?;
            let l = |o: u32| m.u32(row.wrapping_add(o));
            let height = s.sub(l(0x38)?, at(0x14)?);
            if m.u8(source.wrapping_add(2))? == 0 {
                let dy = s.sub(l(0x34)?, at(0x10)?);
                let dx = s.sub(l(0x30)?, at(0xc)?);
                let yy = s.mul(dy, dy);
                let zz = s.mul(height, height);
                let d = s.add(zz, yy);
                let xx = s.mul(dx, dx);
                return Ok(s.add(d, xx));
            }
            let clamped = if s.above(0, height) {
                0
            } else if s.above(height, at(0x30)?) {
                at(0x30)?
            } else {
                height
            };
            if !s.equal(clamped, height) {
                return m.u32(FAR);
            }
            let dy = s.sub(at(0x10)?, l(0x34)?);
            let dx = s.sub(at(0xc)?, l(0x30)?);
            let xx = s.mul(dx, dx);
            let yy = s.mul(dy, dy);
            Ok(s.add(xx, yy))
        }
        _ => {
            let x = X87::new(FPU_CONTROL);
            let (vz, vy, vx) = (x.load32(at(0x14)?), x.load32(at(0x10)?), x.load32(at(0xc)?));
            let sum = x.add(x.mul(vx, vx), x.mul(vy, vy));
            let sum = x.add(sum, x.mul(vz, vz));
            Ok(x.store32(x.sqrt(sum)))
        }
    }
}

/// `0x249ed0(ax = a, cx = b, edi = bsp)`: the coarse table's byte for two
/// clusters (`+0xe8`), 0 for one cluster.
fn coarse_byte(m: &dyn GuestMemory, bsp: u32, a: i16, b: i16) -> Result<u8, MemFault> {
    if a == b {
        return Ok(0);
    }
    let p = ClusterPair::new(i32::from(a), i32::from(b), m.i32(bsp.wrapping_add(0x9c))?);
    let row = (p.row_bias as u32).wrapping_add(m.u32(bsp.wrapping_add(0xe8))?);
    m.u8(row.wrapping_add(p.upper as u32).wrapping_sub(1))
}

/// `0x249d60(eax = a, ecx = b, edx = bsp)`: the sound distance between two
/// clusters as bits. A BSP with a fine table (`+0x224` rows, `+0x228`)
/// maps its byte between the table's bounds (`+4`, `+8`), `0xff` to
/// [`OBSTRUCTION_LIMIT`]; else the coarse byte's low seven bits scale.
pub fn cluster_distance(m: &dyn GuestMemory, bsp: u32, a: i32, b: i32) -> Result<u32, MemFault> {
    if a == b {
        return Ok(0);
    }
    let mut s = Sse::new();
    if m.i32(bsp.wrapping_add(0x224))? <= 0 {
        let byte = coarse_byte(m, bsp, a as i16, b as i16)?;
        let v = s.int(u32::from(byte & 0x7f));
        return Ok(s.mul(v, m.u32(COARSE_STEP)?));
    }
    let fine = m.u32(bsp.wrapping_add(0x228))?;
    let p = ClusterPair::new(a, b, m.i32(bsp.wrapping_add(0x9c))?);
    let row = (p.row_bias as u32).wrapping_add(m.u32(fine.wrapping_add(0x28))?);
    match m.u8(row.wrapping_add(p.upper as u32).wrapping_sub(1))? {
        0 => Ok(0),
        0xff => m.u32(OBSTRUCTION_LIMIT),
        byte => {
            let (low, high) = (m.u32(fine.wrapping_add(4))?, m.u32(fine.wrapping_add(8))?);
            let v = s.int(u32::from(byte) - 1);
            let v = s.mul(v, m.u32(FINE_STEP)?);
            let span = s.sub(high, low);
            let v = s.mul(v, span);
            Ok(s.add(v, low))
        }
    }
}

/// `0x221da0(ecx = listener, edx = source, ratio)`: the listener cannot
/// hear a positioned source through the BSP: none chosen, or the cluster
/// distance times `ratio` reaches [`OBSTRUCTION_LIMIT`]. A source flagged
/// bit 0, one not positioned, one on the listener's object or cluster is
/// heard; an unknown cluster on either side is not.
pub fn obstructed(m: &dyn GuestMemory, listener: u32, source: u32, ratio: u32) -> Result<bool, MemFault> {
    if listener == NONE {
        return Ok(true);
    }
    if m.u8(source)? & 1 != 0 || m.u8(source.wrapping_add(3))? & 0xf != 1 {
        return Ok(false);
    }
    let row = listener_row(m, listener)?;
    let source_cluster = m.u16(source.wrapping_add(0x2c))?;
    if source_cluster == u16::MAX {
        return Ok(true);
    }
    let listener_cluster = m.u16(row.wrapping_add(4))?;
    if listener_cluster == u16::MAX {
        return Ok(true);
    }
    if m.u32(row)? == m.u32(source.wrapping_add(0x28))? || listener_cluster == source_cluster {
        return Ok(false);
    }
    let d = cluster_distance(m, m.u32(BSP)?, i32::from(listener_cluster as i16), i32::from(source_cluster as i16))?;
    let mut s = Sse::new();
    let d = s.mul(d, ratio);
    Ok(!s.above(m.u32(OBSTRUCTION_LIMIT)?, d))
}

/// The first mapped local user (`[0x4e8c20] + 0xc`), or [`NONE`].
fn first_user(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    let users = m.u32(PLAYER_GLOBALS.base)?.wrapping_add(0xc);
    for n in 0..USERS {
        if m.u32(users.wrapping_add(4 * n))? != NONE {
            return Ok(n);
        }
    }
    Ok(NONE)
}

/// `0x127d00(source, range, &distance) -> eax`: the nearest active
/// listener of a positioned source and its distance (bits), the first
/// mapped user at distance 0 for any other. An obstructed listener is
/// none; past `range` none at [`FAR`], unless source word 0 bit 8 or 11
/// plays it anyway (then the first mapped user when none was near).
pub fn select_listener(m: &dyn GuestMemory, source: u32, range: u32) -> Result<(u32, u32), MemFault> {
    if m.u8(source.wrapping_add(3))? & 0xf != 1 {
        return Ok((first_user(m)?, 0));
    }
    let far = m.u32(FAR)?;
    let mut s = Sse::new();
    let (mut best, mut nearest) = (NONE, far);
    for n in 0..USERS {
        let row = listener_row(m, n)?;
        if m.u8(row.wrapping_add(6))? == 0 {
            continue;
        }
        let d = source_distance(m, source, n)?;
        if s.above(nearest, d) {
            best = n;
            nearest = d;
        }
    }
    let x = X87::new(FPU_CONTROL);
    let root = x.sqrt(x.load32(nearest));
    let distance = x.store32(root);
    let ratio = x.store32(x.div(root, x.load32(range)));
    if obstructed(m, best, source, ratio)? {
        best = NONE;
    }
    let flags = m.u16(source)?;
    if flags & 0x100 != 0 || flags & 0x800 != 0 {
        if best == NONE {
            best = first_user(m)?;
        }
        return Ok((best, distance));
    }
    let squared = s.mul(range, range);
    if s.above(nearest, squared) {
        return Ok((NONE, far));
    }
    Ok((best, distance))
}

/// `0x12aff0(cl = inside, outer, inner, d)`: where `d` falls from `outer`
/// (0) to `inner` (1), clamped. Bounds closer than [`CLOSE`] (x87 `fabs`)
/// answer 1 when `outer > d` differs from `inside`, else 0.
pub fn ramp(m: &dyn GuestMemory, inside: bool, outer: u32, inner: u32, d: u32) -> Result<u32, MemFault> {
    let x = X87::new(FPU_CONTROL);
    let gap = x.abs(x.sub(x.load32(outer), x.load32(inner)));
    let mut s = Sse::new();
    let one = m.u32(ONE)?;
    if x.compare(x.load32(m.u32(CLOSE)?), gap, false) == Some(Ordering::Greater) {
        return Ok(if s.above(outer, d) != inside { one } else { 0 });
    }
    let t = s.sub(d, outer);
    let span = s.sub(inner, outer);
    let t = s.div(t, span);
    if s.above(0, t) {
        Ok(0)
    } else if s.above(t, one) {
        Ok(one)
    } else {
        Ok(t)
    }
}

/// `0x12ac20(eax = tag, edi = source, d)`: the linear gain at distance `d`
/// (bits): the ramp from the maximum distance to the minimum (at least
/// [`SHORTEST`]), times `max / max(max, d)`.
pub fn distance_gain(m: &dyn GuestMemory, tag: u32, source: u32, d: u32) -> Result<u32, MemFault> {
    let [min, max] = distances(m, tag, source)?;
    let mut s = Sse::new();
    let shortest = m.u32(SHORTEST)?;
    let inner = if s.above(min, shortest) { min } else { shortest };
    let r = ramp(m, false, max, inner, d)?;
    let reach = if s.above(max, d) { max } else { d };
    let g = s.div(max, reach);
    Ok(s.mul(g, r))
}

/// The bits of the skip draw: `(seed >> 16) * [0x45dc50] > lerp * chance`
/// on the x87, the lerp from the scale row's `+0xc` to `+0x10` at source
/// word 1, the chance the class default row's `+8`.
fn draw(m: &mut dyn GuestMemory, def: u32, scale: u32) -> Result<bool, MemFault> {
    // `[0x4e7408] + 4`: the seed the gate steps.
    let random = m.u32(RANDOM_SEEDS.base)?;
    let seed = m.u32(random.wrapping_add(4))?.wrapping_mul(0x0019_660d).wrapping_add(0x3c6e_f35f);
    m.set_u32(random.wrapping_add(4), seed)?;
    let row_index = i32::from(m.u8(def.wrapping_add(0xb))? as i8);
    let rows = m.u32(m.u32(GESTALT)?.wrapping_add(0xc))?;
    let row = rows.wrapping_add((row_index as u32).wrapping_mul(20)).wrapping_add(0xc);
    let x = X87::new(FPU_CONTROL);
    let low = x.load32(m.u32(row)?);
    // `seed >> 16` is never negative: the `+2^32` fix-up is not taken.
    let r = x.mul(x.int(i64::from(seed >> 16)), x.load32(m.u32(SEED_SCALE)?));
    let span = x.mul(x.sub(x.load32(m.u32(row.wrapping_add(4))?), low), x.load32(scale));
    let lerp = x.add(low, span);
    let chance = x.mul(lerp, x.load32(m.u32(default_row(&*m, def)?.wrapping_add(8))?));
    Ok(x.compare(r, chance, false) == Some(Ordering::Greater))
}

/// `0x126c30(eax = record, tag, &listener, &reason) -> al`: the sound is
/// on and its class plays, the skip draw passes, a listener hears it, and
/// its level (`0x1251e0`) plus its distance gain in dB plus the class
/// default row's `+0x10` is over the floor (or word 0 bit 8 or 11 plays
/// it anyway). The source is the record's `+0xc`.
pub fn start_gate(m: &mut dyn GuestMemory, record: u32, tag: u32) -> Result<Gate, MemFault> {
    let source = record.wrapping_add(0xc);
    let refused = |r| Gate { refused: Some(r), listener: None };
    let globals = m.u32(SOUND_GLOBALS.base)?;
    for o in [0x78, 0x79, 0x7a] {
        if m.u8(globals.wrapping_add(o))? == 0 {
            return Ok(refused(Refusal::Off));
        }
    }
    let def = definition(&*m, tag)?;
    if !class_allows(&*m, def)? {
        return Ok(refused(Refusal::Off));
    }
    let scale = m.u32(source.wrapping_add(4))?;
    if !draw(m, def, scale)? {
        return Ok(refused(Refusal::Skipped));
    }
    if !class_plays(&*m, tag)? {
        return Ok(refused(Refusal::Inaudible));
    }
    let [_, max] = distances(&*m, tag, source)?;
    let (listener, d) = select_listener(&*m, source, max)?;
    let heard = Gate { refused: None, listener: Some(listener) };
    if listener == NONE {
        return Ok(Gate { refused: Some(Refusal::Inaudible), ..heard });
    }
    let gain = distance_gain(&*m, tag, source, d)?;
    let mut params = SourceParams::default();
    for (i, w) in params.0.iter_mut().enumerate() {
        *w = m.u32(source.wrapping_add(4 * i as u32))?;
    }
    let level = GuestLevels::new(&*m).level_bits(TagIndex(tag), &params)?;
    let k = Constants::RETAIL;
    let mut fpu = X87Abi::default();
    fp::ldcw(&mut fpu, FPU_CONTROL);
    let mut mxcsr = 0x1f80;
    let gain_db = to_decibels(&mut fpu, &mut mxcsr, &k, gain);
    let mut s = Sse::new();
    let total = s.add(level, gain_db);
    let total = s.add(m.u32(default_row(&*m, def)?.wrapping_add(0x10))?, total);
    let flags = m.u16(source)?;
    if flags & 0x800 != 0 || flags & 0x100 != 0 || s.above(total, m.u32(FLOOR)?) {
        return Ok(heard);
    }
    Ok(Gate { refused: Some(Refusal::Inaudible), ..heard })
}

#[cfg(test)]
mod tests;
