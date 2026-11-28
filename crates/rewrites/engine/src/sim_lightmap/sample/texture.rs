//! The sample's texel reads: the lightmap page `0x000d2f90` and the base
//! map `0x000d30d0`, and under both `0x001362b0` (point sample),
//! `0x00136140` (address), `0x000358d0` (swizzle), `0x00135ca0` (decode)
//! and the DXT blocks (`0x00223ed0`, `0x00224040`, `0x00224080`, with
//! `0x00223e70`).
//!
//! Both callers ask the texture cache for the image, then either lock
//! its texture (`0x0001d2f0`) or read the image's own `+0x54`. The store
//! answers the cache ([`resolve`]):
//!
//! | [`Source::pixels`] | Guest path | `0x000d2f90` returns |
//! |---|---|---|
//! | `Some(token)` | `0x000d15e0` non-null, `0x000d1680` null, the lock succeeds (`0x000d303f`..`0x000d3064`, `0x000d315c`..`0x000d3175`) | 3 |
//! | `None`, `[image+0x54]` set | `0x000d15e0` null, `0x000d2fff`/`0x000d3130`: the image itself | 0 |
//! | `None` | not resident: zero colour (`0x000d3015`, `0x000d3141`) | 1 |
//!
//! The guest's other resident path, `0x000d1680` non-null after
//! `0x000d15e0`, reads the image's own `+0x54`, which no tag image sets
//! (the cache does not write it); its sample is `0x001362b0`'s -1 and
//! takes no pixels, so no store answer stands for it.
//!
//! The lock builds an image record from the texture's surface
//! ([`locked`]): its level-0 size, the bitmap format `0x0001d4b0` maps the
//! surface format back to, flags 1 (swizzled) | 2 (DXT) | 4 (P8), mip
//! count 1, the locked bits at `+0x54` and `0x00136490`'s size at
//! `+0x34`. Flag 8 is never set, so `0x00136140` reads a locked
//! uncompressed texture's bytes row by row, swizzled as they are.
use super::{LIGHTMAP, Run, Source, at, clamp, f, pack3, pack4, round, unpack, unported};
use runtime::GuestFault;

const NONE: u32 = u32::MAX;
const ONE: u32 = 0x3f80_0000;
/// `[0x0045dbbc]`.
const HALF: f32 = 0.5;
/// `[0x00445420]`, the float formats' scale before their clamp.
const HUNDRED: u32 = 0x42c8_0000;
/// Bits per pixel, i16 by format (`0x00453550`).
const BITS: u32 = 0x0045_3550;
/// The once-only flag the DXT read sets when its block is out of the
/// image (`0x001361d4`).
const DXT_OUTSIDE: u32 = 0x0055_e72c;
/// The swizzle's bit-spread table, u16 (`0x00450840`).
const SPREAD: u32 = 0x0045_0840;
/// The P8 bump format's colour table (`0x00468848`).
const BUMP: u32 = 0x0046_8848;
/// `0x00015cd0`'s colour for a palette it does not hold.
const NO_PALETTE: u32 = 0x00ff_4060;
/// The palette table holds 32 (`0x00484dd8`..`0x00484f58`, 12 bytes each).
const PALETTES: u32 = 32;
/// The scenario tag and the current structure BSP index, which
/// `0x00015b70` reads for the lightmap whose palettes it loads.
const SCENARIO: u32 = 0x004e_0350;
const BSP_INDEX: u32 = 0x0046_86c4;
const P8: u16 = 0x12;

/// The fields of an image record the texel read takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Image {
    /// `+4`, `+6`.
    pub width: u16,
    pub height: u16,
    /// `+0xc`.
    pub format: u16,
    /// `+0xe`.
    pub flags: u16,
    /// `+0x54`, the level-0 bytes.
    pub pixels: u32,
    /// `+0x34`, the bytes behind `pixels`: the DXT read's bound.
    pub size: u32,
}

impl Image {
    fn read<S: Source>(r: &Run<S>, image: u32) -> Result<Self, GuestFault> {
        Ok(Image {
            width: r.u16(at(image, 4))?,
            height: r.u16(at(image, 6))?,
            format: r.u16(at(image, 0xc))?,
            flags: r.u16(at(image, 0xe))?,
            pixels: r.u32(at(image, 0x54))?,
            size: r.u32(at(image, 0x34))?,
        })
    }
}

/// What the cache's answer leaves to sample, and `0x000d2f90`'s EDI for it.
enum Resolved {
    Texel(Image, u32),
    Nothing,
}

/// The surface format a texture of bitmap format `format` is made with,
/// mapped back through `0x0001d4b0`: `Some` the format the lock reports,
/// `None` a surface `0x0001d4b0` has no entry for (the lock fails).
///
/// Stand-in (the texture's creation is behind the cache): a swizzled
/// image is made with its format's swizzled D3D twin (as
/// `assets::bitmap::FORMAT_TABLE` pairs them, P8 as `D3DFMT_P8`) and a
/// linear one with the `LIN_` twin. `0x0001d4b0` maps the swizzled twins
/// of A8, Y8, AY8, A8Y8, R5G6B5, A1R5G5B5, A4R4G4B4, X8R8G8B8, A8R8G8B8,
/// DXT1, DXT3, DXT5 and P8 back to themselves; every `LIN_` format, and
/// V8U8's and G8B8's twins (`0x28`, `0x29`), to -1. P8 bump and the float
/// formats have no established twin: the port stops on them.
fn lock_format(image: &Image) -> Result<Option<u16>, GuestFault> {
    if image.flags & 0x10 != 0 {
        return Ok(None);
    }
    match image.format {
        0 | 1 | 2 | 3 | 6 | 8 | 9 | 10 | 11 | 14 | 15 | 16 | P8 => Ok(Some(image.format)),
        22 | 23 => Ok(None),
        _ => Err(unported(0x0001_d4b0)),
    }
}

/// `0x0001d2f0`'s record for the locked texture of `image`.
fn locked<S: Source>(r: &Run<S>, image: &Image, token: u32) -> Result<Option<Image>, GuestFault> {
    let Some(format) = lock_format(image)? else {
        return Ok(None);
    };
    // 0x0001d369..0x0001d395: not linear (every format 0x0001d4b0
    // accepts), DXT, P8.
    let mut flags = 1;
    if (0xe..=0x10).contains(&format) {
        flags |= 2;
    }
    if format == P8 {
        flags |= 4;
    }
    Ok(Some(Image {
        width: image.width,
        height: image.height,
        format,
        flags,
        pixels: token,
        size: locked_size(r, image.width, image.height, format, flags)?,
    }))
}

/// `0x001364e0` on the lock's record (depth 1, 2D): the texels of level
/// `l`, each side shifted (1 at least) and rounded up to 4 for DXT.
fn level_texels(width: u16, height: u16, flags: u16, l: u32) -> i32 {
    let side = |d: u16| -> u32 {
        let shifted = (d as i16) >> (l & 15);
        let mut n = if shifted <= 1 { 1 } else { u32::from(shifted as u16) };
        if flags & 2 != 0 {
            n = n.wrapping_add(u32::from((n as u8).wrapping_neg() & 3));
        }
        n
    };
    let (w, h) = (side(width), side(height));
    (w as u16 as i16 as i32).wrapping_mul(h as u16 as i16 as i32)
}

/// `0x00136490` as `0x0001d3cd` calls it on the lock's record: levels 0
/// and 1 (its mip count is 1), times the format's bits, over 8.
fn locked_size<S: Source>(r: &Run<S>, width: u16, height: u16, format: u16, flags: u16) -> Result<u32, GuestFault> {
    let texels = level_texels(width, height, flags, 0).wrapping_add(level_texels(width, height, flags, 1));
    Ok((bits(r, format)?.wrapping_mul(texels) / 8) as u32)
}

/// Bits per pixel of `format`, from the i16 table `0x00453550` (0 for
/// `0xffff`).
fn bits<S: Source>(r: &Run<S>, format: u16) -> Result<i32, GuestFault> {
    if format == 0xffff {
        return Ok(0);
    }
    r.i16(at(BITS, u32::from(format).wrapping_mul(2)))
}

/// The cache's answer for `image`, as [`Source::pixels`] gives it (the
/// module's table).
fn resolve<S: Source>(r: &Run<S>, image: u32) -> Result<Resolved, GuestFault> {
    let record = Image::read(r, image)?;
    Ok(match r.src.pixels(image)? {
        Some(token) => match locked(r, &record, token)? {
            Some(l) => Resolved::Texel(l, 3),
            None => Resolved::Nothing,
        },
        None if record.pixels != 0 => Resolved::Texel(record, 0),
        None => Resolved::Nothing,
    })
}

/// `0x00137550(bitmap, index)`: the image record, 0 when the tag has none
/// at `index`.
fn image<S: Source>(r: &Run<S>, bitmap: u32, index: i32) -> Result<u32, GuestFault> {
    let tag = r.tag(bitmap)?;
    let index = index as i16 as i32;
    if tag == 0 || index < 0 || index >= r.u32(at(tag, 0x44))? as i32 {
        return Ok(0);
    }
    Ok(at(r.u32(at(tag, 0x48))?, (index as u32).wrapping_mul(0x74)))
}

/// `0x0013da30(material)`: the bitmap datum of the material's shader's
/// first template map, -1 for none.
pub(super) fn shader_bitmap<S: Source>(r: &Run<S>, material: u32) -> Result<u32, GuestFault> {
    if material == 0 {
        return Ok(NONE);
    }
    let shader = r.u32(at(material, 0xc))?;
    if shader == NONE {
        return Ok(NONE);
    }
    let t = r.u32(at(r.tag(shader)?, 0x24))?;
    let i = r.i16(r.u32(at(t, 0x60))?)?;
    if i == -1 {
        return Ok(NONE);
    }
    r.u32(at(r.u32(at(t, 8))?, (i as u32).wrapping_mul(12)))
}

fn zero(out: &mut [u32; 3]) {
    *out = [0; 3];
}

/// `0x000d2f90(flags, page, palette, u, v)`, EBX `out`: the lightmap's
/// colour at (u, v) on image `page` of the lightmap's bitmap, through
/// palette `palette` when the image is P8. Returns its EDI (the module's
/// table); 1 with `out` untouched when there is no lightmap.
pub(super) fn lightmap<S: Source>(r: &mut Run<S>, page: i32, palette: i32, u: f32, v: f32, out: &mut [u32; 3]) -> Result<u32, GuestFault> {
    let lm = r.u32(LIGHTMAP)?;
    if lm == 0 || r.u32(at(lm, 0x80))? as i32 <= 0 {
        return Ok(1);
    }
    let bitmap = r.u32(at(r.u32(at(lm, 0x84))?, 0x1c))?;
    if bitmap == NONE {
        return Ok(1);
    }
    let img = image(r, bitmap, page)?;
    if img == 0 {
        zero(out);
        return Ok(1);
    }
    let Resolved::Texel(s, edi) = resolve(r, img)? else {
        zero(out);
        return Ok(1);
    };
    let mut c = texel(r, &s, u, v)?;
    // 0x000d309a.
    if s.format == P8 {
        c = palette_colour(r, palette, c)?;
    }
    *out = unpack(c)?;
    Ok(edi)
}

/// `0x000d30d0(flags, u, v)`, EAX `bitmap`, EDI `out`: the colour of the
/// bitmap's first image at (u, v), no palette; false with `out` zeroed
/// when it cannot be read.
pub(super) fn sample<S: Source>(r: &mut Run<S>, bitmap: u32, u: u32, v: u32, out: &mut [u32; 3]) -> Result<bool, GuestFault> {
    let mut img = 0;
    if bitmap != NONE {
        let tag = r.tag(bitmap)?;
        if tag != 0 && r.u32(at(tag, 0x44))? as i32 > 0 {
            img = r.u32(at(tag, 0x48))?;
        }
    }
    if img == 0 {
        zero(out);
        return Ok(false);
    }
    let Resolved::Texel(s, _) = resolve(r, img)? else {
        zero(out);
        return Ok(false);
    };
    let c = texel(r, &s, f(u), f(v))?;
    *out = unpack(c)?;
    Ok(true)
}

/// `t mod n` into [0, n) as `0x001363c5`..`0x001363cd` take it: `idiv`,
/// add, `idiv` again.
fn wrap(t: i32, n: i32, pc: u32) -> Result<i32, GuestFault> {
    if n == 0 {
        return Err(unported(pc));
    }
    Ok(t.wrapping_rem(n).wrapping_add(n).wrapping_rem(n))
}

/// `0x001362b0` with XMM1 1.0, as both callers pass it (`0x000d3071`,
/// `0x000d3182`): level 0, so `0x00135a30`, `0x00135af0` and `0x00135c00`
/// all return `+0x54` and the image's kind does not matter. The nearest
/// texel to (u, v) - 0.5 texel, wrapped (masked for a power of two).
fn texel<S: Source>(r: &mut Run<S>, s: &Image, u: f32, v: f32) -> Result<u32, GuestFault> {
    if s.pixels == 0 {
        return Ok(NONE);
    }
    let dxt = s.flags & 2 != 0;
    let size = |d: u16| -> u32 {
        let mut n = if (d as i16) <= 1 { 1 } else { u32::from(d) };
        if dxt {
            n = n.wrapping_add(u32::from((n as u8).wrapping_neg() & 3));
        }
        n
    };
    let (w, h) = (size(s.width), size(s.height));
    let axis = |n: u32, c: f32, pc: u32| -> Result<i32, GuestFault> {
        let ni = n as u16 as i16 as i32;
        let t = round((ni as f32 * c - HALF).to_bits(), pc)?;
        if ni & ni.wrapping_sub(1) == 0 {
            Ok((n.wrapping_sub(1) & t as u32) as i32)
        } else {
            wrap(t, ni, pc)
        }
    };
    let x = axis(w, u, 0x0013_63c6)?;
    let y = axis(h, v, 0x0013_6410)?;
    fetch(r, s, x, y, w, h)
}

/// `0x00136140`: the texel at (x, y) of the level at `s.pixels`.
fn fetch<S: Source>(r: &mut Run<S>, s: &Image, x: i32, y: i32, w: u32, h: u32) -> Result<u32, GuestFault> {
    let flags = s.flags as u8;
    if flags & 2 != 0 {
        return dxt(r, s, x, y, w);
    }
    let index = if flags & 8 != 0 {
        swizzle(r, w as u16, y as u16, x as u16, h as u16)?
    } else {
        let row = (y as i16 as i32).wrapping_mul(w as u16 as i16 as i32);
        row.wrapping_add(x as i16 as i32) as u32
    };
    decode(r, index, s.pixels, s.format)
}

/// `n`'s log2 as `0x000358e3`..`0x000358f6` count it (shifts of the
/// sign-extended word until 1; 0 for 0 and 1).
fn log2(n: u16) -> u32 {
    let mut v = n as i16 as i32 as u32;
    let mut c = 0;
    if v == 0 || v == 1 {
        return 0;
    }
    while v != 1 {
        v >>= 1;
        c += 1;
    }
    c
}

/// `0x000358d0`: the swizzled index of (x, y) in a `w` by `h` image: the
/// low `m = min(log2 w, log2 h)` bits of each coordinate interleaved
/// through the table `0x00450840` (x even bits, y odd), the longer side's
/// rest above them.
fn swizzle<S: Source>(r: &Run<S>, w: u16, y: u16, x: u16, h: u16) -> Result<u32, GuestFault> {
    let lw = log2(w);
    let lh = log2(h);
    let m = if (lw as i16) > (lh as i16) { lh } else { lw };
    let mask = (1u32 << (m & 31)).wrapping_sub(1) as u16 as i16 as i32;
    let xs = x as i16 as i32;
    let ys = y as i16 as i32;
    let spread = |i: i32| -> Result<u32, GuestFault> { Ok(u32::from(r.u16(at(SPREAD, (i as u32).wrapping_mul(2)))?)) };
    let (mut ex, mut ey) = if mask as i16 <= 0x3f {
        (spread(xs & mask)?, spread(ys & mask)?)
    } else {
        let hi = mask >> 6;
        (
            spread((xs >> 6) & hi)? << 12 | spread(xs & 0x3f)?,
            spread((ys >> 6) & hi)? << 12 | spread(ys & 0x3f)?,
        )
    };
    ey <<= 1;
    let mm = m as i16;
    if (lw as i16) > mm {
        ex |= ((xs >> (m & 31)) as u32) << ((2 * m) & 31);
    } else if (lh as i16) > mm {
        ey |= ((ys >> (m & 31)) as u32) << ((2 * m) & 31);
    }
    Ok(ex | ey)
}

/// The float formats' lane: times 100, clamped to [0, 1]
/// (`0x00135e45`..`0x00135e67`).
fn unit(bits: u32) -> u32 {
    clamp((f(bits) * f(HUNDRED)).to_bits(), 0, ONE)
}

/// `0x00135880`: a 16-bit float as this format packs it (exponent in the
/// low 5 bits, mantissa above, sign on top).
fn half(h: u32) -> u32 {
    let h = h & 0xffff;
    let m = ((h >> 5) & 0x3ff) | ((h >> 15) << 18);
    (m << 13) | (((h & 0x1f) + 0x70) << 23)
}

/// `0x00135ca0(index, base, format)`: texel `index` of the level at
/// `base` as an A8R8G8B8 word (a P8 image's palette index; 0 for a
/// format it does not decode).
fn decode<S: Source>(r: &Run<S>, index: u32, base: u32, format: u16) -> Result<u32, GuestFault> {
    let b = |k: u32| r.u8(at(base, index.wrapping_mul(k)));
    let w = |k: u32| r.u16(at(base, index.wrapping_mul(k))).map(u32::from);
    Ok(match format as i16 as i32 as u32 {
        // A8.
        0 => u32::from(b(1)?) << 24,
        // Y8.
        1 => {
            let c = u32::from(b(1)?);
            (((c | 0xffff_ff00) << 8 | c) << 8) | c
        }
        // AY8.
        2 => {
            let c = u32::from(b(1)?);
            ((c << 8 | c) << 8 | c) << 8 | c
        }
        // A8Y8.
        3 => {
            let c = w(2)?;
            let y = c & 0xff;
            (((c & 0xff00) | y) << 8 | y) << 8 | y
        }
        // R5G6B5 (0x00135cb6).
        6 => {
            let c = w(2)?;
            let mut a = ((c & 0xffff_f800) | 0xffff_0000) << 3;
            a |= c & 0x7e0;
            a <<= 2;
            a |= c & 0xffff_e01f;
            let d = ((c >> 1) & 0xe) | (c & 0x600);
            (a << 3) | (d >> 1)
        }
        // A1R5G5B5 (0x00135cfa).
        8 => {
            let c = w(2)?;
            let mut a = (c & 0x7c00) << 3;
            a |= c & 0x3e0;
            a <<= 2;
            a |= c & 0x7000;
            a <<= 1;
            a |= c & 0x1f;
            a <<= 2;
            a |= c & 0x380;
            a <<= 1;
            a |= (c >> 2) & 7;
            a | ((c >> 15).wrapping_neg() << 24)
        }
        // A4R4G4B4 (0x00135d4f).
        9 => {
            let c = w(2)?;
            let hi = c >> 8;
            let n = hi & 0xf;
            let mut a = (((hi & 0xffff_fff0) << 12) | c) & 0xffff_f000;
            a |= (n << 4 | n) << 4;
            let g = (c >> 4) & 0xf;
            a |= g;
            a = (a << 4) | g;
            let lo = c & 0xf;
            a = (a << 4) | lo;
            (a << 4) | lo
        }
        // X8R8G8B8, A8R8G8B8.
        10 | 11 => r.u32(at(base, index.wrapping_mul(4)))?,
        // P8 bump: its colour table.
        17 => r.u32(at(BUMP, u32::from(b(1)?) * 4))?,
        // P8: the index.
        18 => u32::from(b(1)?),
        // ARGBFP32: four lanes, alpha first (0x00135e16).
        19 => {
            let p = at(base, index << 4);
            let lanes = [r.u32(p)?, r.u32(at(p, 4))?, r.u32(at(p, 8))?, r.u32(at(p, 12))?];
            pack4(lanes.map(unit))?
        }
        // RGBFP32 (0x00135f02).
        20 => {
            let p = at(base, index.wrapping_mul(12));
            pack3([r.u32(p)?, r.u32(at(p, 4))?, r.u32(at(p, 8))?].map(unit))?
        }
        // RGBFP16 (0x00135fc0): alpha 1.
        21 => {
            let p = at(base, index.wrapping_mul(6));
            let lanes = [r.u16(p)?, r.u16(at(p, 2))?, r.u16(at(p, 4))?].map(|h| unit(half(u32::from(h))));
            pack4([ONE, lanes[0], lanes[1], lanes[2]])?
        }
        // V8U8 (0x001360ae).
        22 => {
            let c = w(2)?;
            ((c | 0xffff_ff00) << 16) | (c & 0xffff_ff00)
        }
        // G8B8 (0x001360c8).
        23 => w(2)? | 0xff00_0000,
        _ => 0,
    })
}

/// `0x00015cd0(palette, value)` over the palette table `0x00015b70`
/// builds: entry `k` is the `k`th palette (0x400 bytes) of the current
/// BSP's lightmap, its groups (`+0x84`, `+0x80` of them, 0x68 bytes) in
/// order, the first 32 held, each address stored without its top bit and
/// read with it (`0x00015c37`, `0x00015ceb`).
///
/// Stand-in: the render builds the table at a BSP load; this reads the
/// same tag fields when asked, so it matches the table while the BSP it
/// was built for is current.
pub(super) fn palette_colour<S: Source>(r: &Run<S>, palette: i32, value: u32) -> Result<u32, GuestFault> {
    if palette < 0 {
        return Ok(NO_PALETTE);
    }
    let Some(base) = palette_entry(r, palette as u32)? else {
        return Ok(NO_PALETTE);
    };
    r.u32(at(base | 0x8000_0000, (value & 0xff) * 4))
}

/// The table's entry `k` (without its top bit), `None` past its count.
fn palette_entry<S: Source>(r: &Run<S>, k: u32) -> Result<Option<u32>, GuestFault> {
    if k >= PALETTES {
        return Ok(None);
    }
    let scenario = r.u32(SCENARIO)?;
    let bsps = r.u32(at(scenario, 0x214))?;
    let index = r.i16(BSP_INDEX)? as u32;
    let lightmap = r.u32(at(at(bsps, index.wrapping_mul(0x44)), 0x1c))?;
    if lightmap == NONE {
        return Ok(None);
    }
    let tag = r.tag(lightmap)?;
    let mut n = 0;
    let mut g = 0i32;
    while g < r.u32(at(tag, 0x80))? as i32 {
        let group = at(r.u32(at(tag, 0x84))?, (g as u32).wrapping_mul(0x68));
        let mut j = 0i32;
        while j < r.u32(at(group, 8))? as i32 {
            if n == k {
                let p = at(r.u32(at(group, 0xc))?, (j as u32).wrapping_mul(0x400));
                return Ok(Some(p & 0x7fff_ffff));
            }
            n += 1;
            if n >= PALETTES {
                return Ok(None);
            }
            j += 1;
        }
        g += 1;
    }
    Ok(None)
}


/// `0x00136157`..`0x00136254`: the 4x4 block holding (x, y), bounded by
/// `+0x54`..`+0x54 + +0x34` (outside it: the first block's first texel,
/// and `[0x0055e72c]` set once), decoded by format.
fn dxt<S: Source>(r: &mut Run<S>, s: &Image, x: i32, y: i32, w: u32) -> Result<u32, GuestFault> {
    let bpp = bits(r, s.format)? as i16 as i32;
    let row = (((y as i16 as i32) / 4) as i16 as i32).wrapping_mul(w as u16 as i16 as i32) / 4;
    let column = ((x as i16 as i32) / 4) as i16 as i32;
    let block_bytes = ((bpp << 4) / 8) as i16 as i32;
    let mut block = (row.wrapping_add(column)).wrapping_mul(block_bytes) as u32;
    block = at(block, s.pixels);
    let (mut bx, mut by) = (x as u32 & 3, y as u32 & 3);
    if block < s.pixels || block >= at(s.pixels, s.size) {
        let flag = r.u32(DXT_OUTSIDE)?;
        if flag & 0xff == 0 {
            r.store(DXT_OUTSIDE, (flag & !0xff) | 1);
        }
        (bx, by, block) = (0, 0, s.pixels);
    }
    match s.format as i16 {
        0xe => dxt_colour(r, block, bx, by),
        // DXT3: four bits of alpha a texel.
        0xf => {
            let c = dxt_colour(r, at(block, 8), bx, by)?;
            let a = (r.u16(at(block, by * 2))? >> (bx * 4)) & 0xf;
            Ok((c & 0x00ff_ffff) | u32::from((a << 4 | a) as u8) << 24)
        }
        0x10 => dxt5(r, block, bx, by),
        _ => Ok(0),
    }
}

/// `0x00223e70`: an R5G6B5 word as `R << 16 | G << 8 | B`, each widened
/// by repeating its top bits.
fn expand(c: u16) -> u32 {
    let b = ((c as u8) << 3) | ((c as u8) << 3 >> 5);
    let g6 = ((c >> 5) as u8) << 2;
    let g = g6 | (g6 >> 6);
    let r5 = ((c >> 11) as u8) << 3;
    let r = r5 | (r5 >> 5);
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

/// `0x00223ed0`: texel (x, y) of the DXT colour block at `block`: two
/// R5G6B5 ends and 2-bit indices; four colours, a third and two thirds
/// between (rounded), when the first end is the greater word, else
/// three, the mean between and transparent black.
fn dxt_colour<S: Source>(r: &Run<S>, block: u32, x: u32, y: u32) -> Result<u32, GuestFault> {
    if block == 0 {
        // 0x00223edf: zeroes 16 words at its output, past the caller's
        // slot; never reached from a nonzero pixel base.
        return Err(unported(0x0022_3edf));
    }
    let (e0, e1) = (r.u16(block)?, r.u16(at(block, 2))?);
    let (c0, c1) = (expand(e0), expand(e1));
    let lane = |c: u32, k: u32| (c >> (8 * k)) & 0xff;
    let mut c2 = 0xff00_0000u32;
    let mut c3 = 0u32;
    for k in 0..3 {
        let (a, b) = (lane(c0, k), lane(c1, k));
        if e0 > e1 {
            c2 |= ((2 * a + b + 1) / 3) << (8 * k);
            c3 |= ((a + 2 * b + 1) / 3) << (8 * k);
        } else {
            c2 |= ((a + b) / 2) << (8 * k);
        }
    }
    if e0 > e1 {
        c3 |= 0xff00_0000;
    }
    let colours = [c0 | 0xff00_0000, c1 | 0xff00_0000, c2, c3];
    let index = (r.u32(at(block, 4))? as i32 >> ((x + y * 4) * 2)) & 3;
    Ok(colours[index as usize])
}

/// `0x00224080`: DXT5, the colour block at `+8` with its alpha from two
/// ends and 3-bit indices: eight levels (sevenths) when the first end is
/// the greater, else six (fifths) and 0 and 255.
fn dxt5<S: Source>(r: &Run<S>, block: u32, x: u32, y: u32) -> Result<u32, GuestFault> {
    let c = dxt_colour(r, at(block, 8), x, y)?;
    let (a0, a1) = (u32::from(r.u8(block)?), u32::from(r.u8(at(block, 1))?));
    let t = if a0 > a1 {
        [a0, a1, (6 * a0 + a1) / 7, (5 * a0 + 2 * a1) / 7, (4 * a0 + 3 * a1) / 7, (3 * a0 + 4 * a1) / 7, (2 * a0 + 5 * a1) / 7, (a0 + 6 * a1) / 7]
    } else {
        [a0, a1, (4 * a0 + a1) / 5, (3 * a0 + 2 * a1) / 5, (2 * a0 + 3 * a1) / 5, (a0 + 4 * a1) / 5, 0, 0xff]
    };
    let (bits, at_x) = if y < 2 {
        (u32::from(r.u8(at(block, 2))?) | u32::from(r.u8(at(block, 3))?) << 8 | u32::from(r.u8(at(block, 4))?) << 16, x + y * 4)
    } else {
        (u32::from(r.u8(at(block, 5))?) | u32::from(r.u8(at(block, 6))?) << 8 | u32::from(r.u8(at(block, 7))?) << 16, (x + y * 4).wrapping_sub(8))
    };
    let index = (bits as i32 >> ((at_x.wrapping_mul(3)) & 31)) & 7;
    Ok((c & 0x00ff_ffff) | (t[index as usize] & 0xff) << 24)
}
