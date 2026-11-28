//! The sample's reads of a section's vertex streams.
//!
//! A section (the runtime section [`Source::section`] answers, or an
//! object's) holds the triangle indices at `+0x24` (three u16 a
//! triangle) and its stream records at `+0x3c`, `+0x38` of them, 0x20
//! bytes each: byte `+0` the stream's type, `+0xc` its data. The searches
//! start at record 3 and stop at the first record of the type they want;
//! one that finds none reads the last record it looked at, as the guest's
//! loops leave EDI.
//!
//! | Guest | Here | Stream |
//! |---|---|---|
//! | `0x000d1d70` | [`block`] | |
//! | `0x000d2bf0` | [`lightmap_uv`] | `0x1e` two f32, or `0x1f` two packed i16 |
//! | `0x000d2dc0` | [`texcoord`] | record 1: `0x18` two f32, else two packed i16, clamped to +-1000 |
//! | `0x000d31c0` | [`normal`] | `0x30`, an 11-11-10 packed normal |
//! | `0x000d33a0` | [`vertex_colour`] | `0x2e` an A8R8G8B8 word, or `0x2f` three bytes |
//! | `0x000d35a0`, `0x000d3630` | [`instance_stream`], [`instance_colour`] | the instance's vertex buffer, stream record 1 |
use super::{BSP, Run, Source, at, blend, f, unpack};
use runtime::GuestFault;

const NONE: u32 = u32::MAX;
/// `[0x0045dc0c]`, `[0x0045dbc0]`, `[0x0045dc50]` (1/65535): a packed
/// coordinate `s` is `(s * 2 + 1) / 65535`.
const TWO: f32 = 2.0;
const ONE: f32 = 1.0;
const UNIT16: u32 = 0x3780_0080;
/// `[0x0045df28]`, `[0x0045dccc]`.
const TEXCOORD_LO: u32 = 0xc47a_0000;
const TEXCOORD_HI: u32 = 0x447a_0000;
/// `[0x0045dd4c]` (2^-20), `[0x0045dd3c]` (1/2047), `[0x0045dd48]` (2^-21),
/// `[0x0045dd38]` (1/1023).
const N11_SCALE: u32 = 0x3580_0000;
const N11_RANGE: u32 = 0x3a00_1002;
const N10_SCALE: u32 = 0x3500_0000;
const N10_RANGE: u32 = 0x3a80_2008;
/// `[0x0045dbb4]`.
const INV_255: u32 = 0x3b80_8081;

fn packed(s: i32) -> f32 {
    (s as f32 * TWO + ONE) * f(UNIT16)
}

/// `0x000d1d70`: the section of the hit's block, through
/// [`Source::section`]. An instance hit names its definition's block
/// (`[bsp+0x13c] + def * 0xc8 + 0x28`), a cluster hit its cluster's
/// (`[bsp+0xa0] + cluster * 0xb0 + 0x28`); an object hit none.
pub(super) fn block<S: Source>(r: &Run<S>, hit: u32) -> Result<Option<u32>, GuestFault> {
    let bsp = r.u32(BSP)?;
    let instance = r.u32(at(hit, 4))?;
    if instance != NONE {
        let instances = r.u32(at(bsp, 0x144))?;
        let def = r.i16(at(at(instances, instance.wrapping_mul(0x58)), 0x34))? as u32;
        let info = at(at(r.u32(at(bsp, 0x13c))?, def.wrapping_mul(0xc8)), 0x28);
        return r.src.section(info);
    }
    // 0x000d1dc2.
    if r.u32(at(hit, 8))? != NONE {
        return Ok(None);
    }
    let info = at(at(r.u32(at(bsp, 0xa0))?, r.u32(hit)?.wrapping_mul(0xb0)), 0x28);
    // 0x0016e290 asks again with flags 0 when [0x004e6494] is set; the
    // store's answer is the same either way.
    r.src.section(info)
}

/// The search of `0x000d2bf0`, `0x000d31c0` and `0x000d33a0`: the record
/// of type `a` (false) or `b` (true), else the last one looked at
/// (false). `None` when the section has three records or fewer.
fn find<S: Source>(r: &Run<S>, section: u32, a: u8, b: Option<u8>) -> Result<Option<(u32, bool)>, GuestFault> {
    let count = r.u32(at(section, 0x38))? as i32;
    if count <= 3 {
        return Ok(None);
    }
    let mut rec = at(r.u32(at(section, 0x3c))?, 0x60);
    let mut i = 3;
    loop {
        let t = r.u8(rec)?;
        if t == a {
            return Ok(Some((rec, false)));
        }
        if Some(t) == b {
            return Ok(Some((rec, true)));
        }
        i += 1;
        if i >= count {
            return Ok(Some((rec, false)));
        }
        rec = at(rec, 0x20);
    }
}

/// The triangle's three vertex indices.
fn corners<S: Source>(r: &Run<S>, section: u32, tri: u32) -> Result<[u32; 3], GuestFault> {
    let p = at(r.u32(at(section, 0x24))?, tri.wrapping_mul(6));
    Ok([u32::from(r.u16(p)?), u32::from(r.u16(at(p, 2))?), u32::from(r.u16(at(p, 4))?)])
}

/// Two coordinates at each corner: f32 pairs (stride 8) or packed i16
/// pairs (stride 4), blended.
fn pair<S: Source>(r: &Run<S>, data: u32, idx: [u32; 3], floats: bool, u: f32, v: f32) -> Result<(f32, f32), GuestFault> {
    let mut c = [(0.0, 0.0); 3];
    for (k, i) in idx.into_iter().enumerate() {
        c[k] = if floats {
            let p = at(data, i.wrapping_mul(8));
            (r.f32(p)?, r.f32(at(p, 4))?)
        } else {
            let p = at(data, i.wrapping_mul(4));
            (packed(r.i16(p)?), packed(r.i16(at(p, 2))?))
        };
    }
    Ok((blend(c[0].0, c[1].0, c[2].0, u, v), blend(c[0].1, c[1].1, c[2].1, u, v)))
}

/// `0x000d2bf0`: the lightmap UV, from the first stream `0x1e` (f32) or
/// `0x1f` (packed) at or after record 3. `None` (nothing stored) when the
/// section has no fourth record.
pub(super) fn lightmap_uv<S: Source>(r: &Run<S>, section: u32, tri: u32, u: f32, v: f32) -> Result<Option<(f32, f32)>, GuestFault> {
    let Some((rec, packed)) = find(r, section, 0x1e, Some(0x1f))? else {
        return Ok(None);
    };
    let idx = corners(r, section, tri)?;
    let data = r.u32(at(rec, 0xc))?;
    pair(r, data, idx, !packed, u, v).map(Some)
}

/// `0x000d2dc0`: the texcoord from stream record 1 (`+0x8` is an offset
/// into its data), each clamped to [-1000, 1000] (`0x000d2f34`).
pub(super) fn texcoord<S: Source>(r: &Run<S>, section: u32, tri: u32, u: f32, v: f32) -> Result<(u32, u32), GuestFault> {
    let rec = at(r.u32(at(section, 0x3c))?, 0x20);
    let idx = corners(r, section, tri)?;
    let data = at(r.u32(at(rec, 0xc))?, r.u32(at(rec, 8))?);
    let floats = r.u8(rec)? == 0x18;
    let (s, t) = pair(r, data, idx, floats, u, v)?;
    let c = |x: f32| super::clamp(x.to_bits(), TEXCOORD_LO, TEXCOORD_HI);
    Ok((c(s), c(t)))
}

/// `0x000d31c0`: the normal from stream `0x30`: x and y the word's low
/// two 11-bit fields, z its top 10, each `(2n + 1) / (2^bits - 1)`.
pub(super) fn normal<S: Source>(r: &Run<S>, section: u32, tri: u32, u: f32, v: f32) -> Result<Option<[u32; 3]>, GuestFault> {
    let Some((rec, _)) = find(r, section, 0x30, None)? else {
        return Ok(None);
    };
    let data = r.u32(at(rec, 0xc))?;
    let idx = corners(r, section, tri)?;
    let mut n = [[0.0f32; 3]; 3];
    for (k, i) in idx.into_iter().enumerate() {
        let w = r.u32(at(data, i.wrapping_mul(4)))?;
        let x = (w << 21) as i32;
        let y = ((w >> 11) << 21) as i32;
        let z = ((w >> 22) << 22) as i32;
        n[k] = [
            (x as f32 * f(N11_SCALE) + ONE) * f(N11_RANGE),
            (y as f32 * f(N11_SCALE) + ONE) * f(N11_RANGE),
            (z as f32 * f(N10_SCALE) + ONE) * f(N10_RANGE),
        ];
    }
    Ok(Some(blend3(n, u, v)))
}

fn blend3(c: [[f32; 3]; 3], u: f32, v: f32) -> [u32; 3] {
    [0, 1, 2].map(|i| blend(c[0][i], c[1][i], c[2][i], u, v).to_bits())
}

/// A corner's colour: an A8R8G8B8 word through `0x00131bb0`, or three
/// bytes each times 1/255 (SSE).
fn colours<S: Source>(r: &Run<S>, data: u32, idx: [u32; 3], words: bool) -> Result<[[f32; 3]; 3], GuestFault> {
    let mut c = [[0.0f32; 3]; 3];
    for (k, i) in idx.into_iter().enumerate() {
        c[k] = if words {
            unpack(r.u32(at(data, i.wrapping_mul(4)))?)?.map(f)
        } else {
            let p = at(data, i.wrapping_mul(3));
            [r.u8(p)?, r.u8(at(p, 1))?, r.u8(at(p, 2))?].map(|b| f32::from(b) * f(INV_255))
        };
    }
    Ok(c)
}

/// `0x000d33a0`: the vertex colour from stream `0x2e` (words) or `0x2f`
/// (bytes); `None` when the section has no fourth record.
pub(super) fn vertex_colour<S: Source>(r: &Run<S>, section: u32, tri: u32, u: f32, v: f32) -> Result<Option<[u32; 3]>, GuestFault> {
    let Some((rec, bytes)) = find(r, section, 0x2e, Some(0x2f))? else {
        return Ok(None);
    };
    let idx = corners(r, section, tri)?;
    let data = r.u32(at(rec, 0xc))?;
    Ok(Some(blend3(colours(r, data, idx, !bytes)?, u, v)))
}

/// `0x000d35a0` (`dl` 1, the stack argument unread): the stream record
/// of the instance's vertex buffer, with its `+4` and `+8` set to the
/// instance's first vertex as a byte offset (4 or 3 bytes a vertex); 0
/// when the buffer is not flagged (`+0` bit 1) or not resident.
pub(super) fn instance_stream<S: Source>(r: &mut Run<S>, group: u32, instance: u32) -> Result<u32, GuestFault> {
    let entry = at(r.u32(at(group, 0x54))?, instance.wrapping_mul(12));
    let first = r.u32(at(entry, 8))?;
    let buffer = at(r.u32(at(group, 0x44))?, u32::from(r.u16(at(entry, 2))?).wrapping_mul(0x38));
    if r.u8(buffer)? & 2 == 0 {
        return Ok(0);
    }
    // 0x000d35e3: 0x0012de70 (flags 3) on the buffer's block, then its
    // runtime section [buffer+0x34].
    let Some(section) = r.src.section(at(buffer, 0xc))? else {
        return Ok(0);
    };
    let rec = at(r.u32(at(section, 4))?, 0x20);
    let words = r.u8(rec)? == 0x2e;
    let first = u32::from(r.u16(first)?);
    let offset = if words { first << 2 } else { first.wrapping_mul(3) };
    r.store(at(rec, 4), offset);
    r.store(at(rec, 8), offset);
    Ok(rec)
}

/// `0x000d3630`: the colour of the instance's vertex stream `rec`
/// (`0x2e` words, else bytes) from `[rec+0xc] + [rec+4]`, indexed by the
/// hit section's triangle.
pub(super) fn instance_colour<S: Source>(
    r: &Run<S>,
    rec: u32,
    tri: u32,
    section: u32,
    u: f32,
    v: f32,
    out: &mut [u32; 3],
) -> Result<(), GuestFault> {
    let idx = corners(r, section, tri)?;
    let data = at(r.u32(at(rec, 0xc))?, r.u32(at(rec, 4))?);
    let words = r.u8(rec)? == 0x2e;
    *out = blend3(colours(r, data, idx, words)?, u, v);
    Ok(())
}
