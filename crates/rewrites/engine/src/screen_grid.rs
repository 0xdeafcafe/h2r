//! The grid `0x00034d90` draws for each step of `0x00021360`
//! ([`crate::glow::wide`]; docs/graphics/native-post-hud.md, stage 5, "The
//! screen effects"), as the exact vertices it pushes, for the native
//! renderer to draw them in the game's place.
//!
//! (Code, `h2 disasm`.) `0x00034d90` (EAX the target, EDI four floats,
//! nine stack words, `ret 0x24`) is `0x00034a90`'s sibling. It fills the
//! same parameter block (the rectangle, then from `+0x10`: a depth, a
//! spread, per-tap offsets and per-stage size ratios, four per-tap
//! floats, two radii, a position scale, the tap count), sets its render
//! states, and calls `0x00034770` with the attribute function `0x00034100`
//! on a 20 x 30 grid where `0x00034a90` emits one quad (`0x00034420`):
//!
//! - the rectangle: the target's (0, 0, width, height; `0x0001dd60`) when
//!   its eighth word's low byte is set, else the window `0x004b9dd0`;
//! - the depth `|word 4|` (1.0 from `0x00021360`), the spread 0, the
//!   offsets 0, the ratios each stage's bound size (`0x00485af4`,
//!   `0x00485b04`) over the target's, EDI's four floats, the radii words
//!   5 and 6, the position scale 1.0, four taps.
//!
//! `0x00034770`: per row `j` of 30 a strip (`SET_BEGIN_END` 6), per column
//! `i` of 0..=20 and `k` of 0, 1 the vertex at `u = i / 20`,
//! `v = (j + k) / 30`, its attributes 15 down to 0 each from the attribute
//! function into `(0, 0, 0, 1)` and pushed when it returns true
//! (`0x101a00 + 16 a`, the position `0x101518`), then `SET_BEGIN_END` 0.
//! `0x00034100` takes attributes 0..5 ([`vertex`]):
//!
//! - 5: `(u, v)`;
//! - 1..4, tap `t = a - 1`: the rectangle at `(u, v)`; with the tap's
//!   float `f` zero, moved by the spread times a sign (`[0x0045dbbc]` for
//!   `t & 1` in x, `t & 2` in y, else `[0x0045dbc4]`); otherwise pulled
//!   toward the rectangle's centre by `f` (`(1 - f) p + f c`, plus the
//!   spread term), `f` first weighted, when the radii differ, by where the
//!   vertex's distance from the centre (`sqrt` of `(2u - 1)^2 + (2v -
//!   1)^2`, clamped to 0..1, x87) falls between them (0 inside the first,
//!   1 past the second); then times the stage's ratio, plus its offset;
//! - 0: the rectangle at `(u, v)` times the position scale, z and w from
//!   the depth (`0x000350e0`: the view's near and far `0x00485ad4`/`d8`,
//!   clamped to `0..[0x0045dd40]`).
//!
//! So a step of `0x00021360` is a zoom: each tap samples the source
//! pulled toward the centre by its float (`s`, `s * k1`, `s * k2`,
//! `-s`), a radial blur, fading in between the radii.
//!
//! The arithmetic is the original's (SSE under the caller's MXCSR, the
//! distance's square root and test on the x87 under its control word),
//! checked against the lifted original by
//! `engine/tests/screen_grid_differential.rs`.

use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

/// Image and engine constants.
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const ZERO: u32 = 0x0045_dbd8;
    /// The centre's half, and the tap sign with its bit set.
    pub const HALF: u32 = 0x0045_dbbc;
    /// The tap sign with its bit clear.
    pub const SIGN_CLEAR: u32 = 0x0045_dbc4;
    /// The distance's scale (the blur's spread term).
    pub const DOUBLE: u32 = 0x0045_dc0c;
    /// z and w's top.
    pub const DEPTH: u32 = 0x0045_dd40;
    /// The view's near and far.
    pub const NEAR: u32 = 0x0048_5ad4;
    pub const FAR: u32 = 0x0048_5ad8;
    /// Each stage's bound width and height (`0x00014f60`).
    pub const BOUND_WIDTH: u32 = 0x0048_5af4;
    pub const BOUND_HEIGHT: u32 = 0x0048_5b04;
}
/// The window (top, left, bottom, right shorts).
pub const WINDOW: u32 = 0x004b_9dd0;
/// The grid `0x00034d90` asks for.
pub const COLUMNS: u32 = 20;
pub const ROWS: u32 = 30;
/// Its taps.
pub const TAPS: usize = 4;

/// `0x00034d90`'s inputs, besides memory's constants and window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    /// The target's size (`0x0001dd60` of EAX, -1 the bound one).
    pub size: (i32, i32),
    /// Each stage's bound size (`0x00485af4`/`0x00485b04`).
    pub bound: [(i32, i32); 4],
    /// EDI's four floats.
    pub taps: [u32; TAPS],
    /// Stack word 4: the depth (f32 bits).
    pub depth: u32,
    /// Stack words 5 and 6: the radii (f32 bits).
    pub radii: [u32; 2],
    /// Stack word 7's low byte set: the target's rectangle, not the
    /// window.
    pub full: bool,
}

impl Grid {
    /// The bound sizes as memory holds them.
    pub fn bound(mem: &dyn GuestMemory) -> Result<[(i32, i32); 4], Error> {
        let mut b = [(0, 0); 4];
        for (s, v) in b.iter_mut().enumerate() {
            let s = s as u32;
            *v = (
                mem.u32(constant::BOUND_WIDTH + 4 * s)? as i32,
                mem.u32(constant::BOUND_HEIGHT + 4 * s)? as i32,
            );
        }
        Ok(b)
    }
}

/// One vertex: attribute 5 (`u`, `v`, 0 and the image's 1.0), the four
/// taps (attributes 1..4) and the position (f32 bits each).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridVertex {
    pub uv: [u32; 4],
    pub taps: [[u32; 4]; TAPS],
    pub position: [u32; 4],
}

impl GridVertex {
    /// The words it pushes: attributes 5 down to 0.
    pub fn pushed(&self) -> Vec<u32> {
        let mut w = vec![0x0010_1a50];
        w.extend_from_slice(&self.uv);
        for t in (0..TAPS).rev() {
            w.push(0x0010_1a10 + 0x10 * t as u32);
            w.extend_from_slice(&self.taps[t]);
        }
        w.push(0x0010_1518);
        w.extend_from_slice(&self.position);
        w
    }
}

/// The parameter block `0x00034d90` fills (from `+0x10`, the part the
/// attribute function reads).
struct Block {
    rect: [u32; 4],
    depth: u32,
    spread: u32,
    offsets: [[u32; 2]; TAPS],
    ratios: [[u32; 2]; TAPS],
    taps: [u32; TAPS],
    radii: [u32; 2],
    scale: u32,
}

fn block(mem: &dyn GuestMemory, sse: &mut Sse, g: &Grid) -> Result<Block, Error> {
    let one = mem.u32(constant::ONE)?;
    let (w, h) = g.size;
    let rect = if g.full {
        [0, sse.int(w)?, 0, sse.int(h)?]
    } else {
        let s = |o: u32| -> Result<i32, Error> { Ok(i32::from(mem.u16(WINDOW + o)? as i16)) };
        // left, right, top, bottom
        let (l, r, t, b) = (s(2)?, s(6)?, s(0)?, s(4)?);
        [sse.int(l)?, sse.int(r)?, sse.int(t)?, sse.int(b)?]
    };
    let fw = sse.int(w)?;
    let inv_w = sse.div(one, fw)?;
    let fh = sse.int(h)?;
    let inv_h = sse.div(one, fh)?;
    let depth = if sse.below(g.depth, 0)? { sse.sub(0, g.depth)? } else { g.depth };
    let mut ratios = [[0; 2]; TAPS];
    for (s, r) in ratios.iter_mut().enumerate() {
        let (bw, bh) = g.bound[s];
        let fw = sse.int(bw)?;
        let x = sse.mul(fw, inv_w)?;
        let fh = sse.int(bh)?;
        let y = sse.mul(fh, inv_h)?;
        *r = [x, y];
    }
    Ok(Block {
        rect,
        depth,
        spread: 0,
        offsets: [[0; 2]; TAPS],
        ratios,
        taps: g.taps,
        radii: g.radii,
        scale: one,
    })
}

/// `0x000350e0`: the rectangle at `uv`, and z and w from `depth`. The
/// planar fog's corners are this call too (`crate::planar_fog`).
pub(crate) fn position(mem: &dyn GuestMemory, sse: &mut Sse, rect: &[u32; 4], uv: [u32; 2], depth: u32) -> Result<[u32; 4], Error> {
    let [left, right, top, bottom] = *rect;
    let x = sse.sub(right, left)?;
    let x = sse.mul(x, uv[0])?;
    let x = sse.add(x, left)?;
    let y = sse.sub(bottom, top)?;
    let y = sse.mul(y, uv[1])?;
    let y = sse.add(y, top)?;
    let far = mem.u32(constant::FAR)?;
    let near = mem.u32(constant::NEAR)?;
    let q = sse.sub(far, near)?;
    let q = sse.div(far, q)?;
    let a = sse.mul(q, depth)?;
    let b = sse.mul(near, q)?;
    let top_z = mem.u32(constant::DEPTH)?;
    let z = sse.sub(a, b)?;
    let z = sse.div(z, depth)?;
    let z = sse.mul(z, top_z)?;
    let clamp = |sse: &mut Sse, v: u32| -> Result<u32, Error> {
        Ok(if sse.above(0, v)? {
            0
        } else if sse.above(v, top_z)? {
            top_z
        } else {
            v
        })
    };
    let z = clamp(sse, z)?;
    let w = sse.div(depth, far)?;
    let w = sse.mul(w, top_z)?;
    let w = clamp(sse, w)?;
    Ok([x, y, z, w])
}

/// The tap weight with the radii apart (`0x000341fe..0x000342eb`): the
/// distance's place between them, clamped to 0..1.
fn radial(mem: &dyn GuestMemory, sse: &mut Sse, x87_control: u16, uv: [u32; 2], radii: [u32; 2]) -> Result<u32, Error> {
    let one = mem.u32(constant::ONE)?;
    let k = mem.u32(constant::DOUBLE)?;
    let [lo, hi] = radii;
    let distance2 = |sse: &mut Sse| -> Result<u32, Error> {
        let dy = sse.mul(uv[1], k)?;
        let dy = sse.sub(dy, one)?;
        let dx = sse.mul(uv[0], k)?;
        let dx = sse.sub(dx, one)?;
        let dx2 = sse.mul(dx, dx)?;
        let dy2 = sse.mul(dy, dy)?;
        sse.add(dy2, dx2)
    };
    let d2 = distance2(sse)?;
    let d2 = if sse.above(0, d2)? {
        0
    } else {
        let d2 = distance2(sse)?;
        if sse.above(d2, one)? { one } else { distance2(sse)? }
    };
    let mut x = X87::new(x87_control);
    x.fld(d2)?;
    x.unary(x87::abi::unary_op::SQRT)?;
    let r = x.fst_m32()?;
    x.arith_m(x87::abi::arith_op::SUB, lo)?;
    x.fld(hi)?;
    x.arith_m(x87::abi::arith_op::SUB, lo)?;
    x.arith_reg(x87::abi::arith_op::DIV, 1, 0, true)?;
    x.fld(mem.u32(constant::ZERO)?)?;
    let flags = x.fcomip(1)?;
    x.fstp_st(0)?;
    if flags & 0x41 == 0 {
        return Ok(0);
    }
    let n = sse.sub(r, lo)?;
    let d = sse.sub(hi, lo)?;
    let a = sse.div(n, d)?;
    if sse.above(a, one)? {
        return Ok(one);
    }
    let n = sse.sub(r, lo)?;
    let d = sse.sub(hi, lo)?;
    sse.div(n, d)
}

/// `0x00034100` for taps 1..4 (`t` 0..3) at `uv`.
fn tap(
    mem: &dyn GuestMemory,
    sse: &mut Sse,
    x87_control: u16,
    b: &Block,
    t: usize,
    uv: [u32; 2],
) -> Result<[u32; 4], Error> {
    let one = mem.u32(constant::ONE)?;
    let half = mem.u32(constant::HALF)?;
    let clear = mem.u32(constant::SIGN_CLEAR)?;
    let [left, right, top, bottom] = b.rect;
    let cy = sse.add(top, bottom)?;
    let cy = sse.mul(cy, half)?;
    let cx = sse.add(left, right)?;
    let cx = sse.mul(cx, half)?;
    let x0 = sse.sub(right, left)?;
    let x0 = sse.mul(x0, uv[0])?;
    let x0 = sse.add(x0, left)?;
    let y0 = sse.sub(bottom, top)?;
    let y0 = sse.mul(y0, uv[1])?;
    let y0 = sse.add(y0, top)?;
    let sx = if t & 1 != 0 { half } else { clear };
    let sy = if t & 2 != 0 { half } else { clear };
    let [rx, ry] = b.ratios[t];
    let f = b.taps[t];
    let (x, y) = if sse.equal(f, 0)? {
        let x = sse.mul(b.spread, sx)?;
        let x = sse.add(x, x0)?;
        let x = sse.mul(x, rx)?;
        let y = sse.mul(b.spread, sy)?;
        let y = sse.add(y, y0)?;
        let y = sse.mul(y, ry)?;
        (x, y)
    } else {
        let f = if sse.equal(b.radii[0], b.radii[1])? {
            f
        } else {
            let w = radial(mem, sse, x87_control, uv, b.radii)?;
            sse.mul(w, f)?
        };
        let keep = sse.sub(one, f)?;
        let a = sse.mul(b.spread, sx)?;
        let c = sse.mul(f, cx)?;
        let x = sse.add(a, c)?;
        let k = sse.mul(keep, x0)?;
        let x = sse.add(x, k)?;
        let x = sse.mul(x, rx)?;
        let a = sse.mul(y0, keep)?;
        let c = sse.mul(b.spread, sy)?;
        let y = sse.add(a, c)?;
        let k = sse.mul(f, cy)?;
        let y = sse.add(y, k)?;
        let y = sse.mul(y, ry)?;
        (x, y)
    };
    let [ox, oy] = b.offsets[t];
    let x = sse.add(ox, x)?;
    let y = sse.add(oy, y)?;
    Ok([x, y, 0, one])
}

/// One vertex at grid point `(i, j)`.
pub fn vertex(mem: &dyn GuestMemory, g: &Grid, i: u32, j: u32, mxcsr: u32, x87_control: u16) -> Result<GridVertex, Error> {
    let mut sse = Sse { mxcsr };
    let b = block(mem, &mut sse, g)?;
    vertex_in(mem, &mut sse, x87_control, &b, i, j)
}

fn vertex_in(mem: &dyn GuestMemory, sse: &mut Sse, x87_control: u16, b: &Block, i: u32, j: u32) -> Result<GridVertex, Error> {
    let fi = sse.int(i as i32)?;
    let columns = sse.int(COLUMNS as i32)?;
    let u = sse.div(fi, columns)?;
    let fj = sse.int(j as i32)?;
    let rows = sse.int(ROWS as i32)?;
    let v = sse.div(fj, rows)?;
    let uv = [u, v];
    let mut taps = [[0; 4]; TAPS];
    for t in (0..TAPS).rev() {
        taps[t] = tap(mem, sse, x87_control, b, t, uv)?;
    }
    let p = position(mem, sse, &b.rect, uv, b.depth)?;
    let position = [sse.mul(p[0], b.scale)?, sse.mul(p[1], b.scale)?, p[2], p[3]];
    Ok(GridVertex {
        uv: [u, v, 0, mem.u32(constant::ONE)?],
        taps,
        position,
    })
}

/// The strips `0x00034d90` pushes: per row of [`ROWS`], per column
/// 0..=[`COLUMNS`] the vertex in this row and the one below.
pub fn strips(mem: &dyn GuestMemory, g: &Grid, mxcsr: u32, x87_control: u16) -> Result<Vec<Vec<GridVertex>>, Error> {
    let mut sse = Sse { mxcsr };
    let b = block(mem, &mut sse, g)?;
    let mut out = Vec::with_capacity(ROWS as usize);
    for j in 0..ROWS {
        let mut strip = Vec::with_capacity(2 * (COLUMNS as usize + 1));
        for i in 0..=COLUMNS {
            for k in 0..2 {
                strip.push(vertex_in(mem, &mut sse, x87_control, &b, i, j + k)?);
            }
        }
        out.push(strip);
    }
    Ok(out)
}

/// The words the strips push (each: `SET_BEGIN_END` 6, the vertices,
/// `SET_BEGIN_END` 0).
pub fn pushed(strips: &[Vec<GridVertex>]) -> Vec<u32> {
    let mut w = Vec::new();
    for s in strips {
        w.extend_from_slice(&[0x0004_17fc, 6]);
        for v in s {
            w.extend(v.pushed());
        }
        w.extend_from_slice(&[0x0004_17fc, 0]);
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    /// Constants chosen here, a 640 x 480 window and target, every stage
    /// bound to a half-size source.
    fn world() -> (FlatMemory, Grid) {
        let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
        for (a, v) in [
            (constant::ONE, f(1.0)),
            (constant::HALF, f(0.5)),
            (constant::SIGN_CLEAR, f(-0.5)),
            (constant::DOUBLE, f(2.0)),
            (constant::DEPTH, f(1.0)),
            (constant::NEAR, f(0.1)),
            (constant::FAR, f(100.0)),
        ] {
            m.set_u32(a, v).unwrap();
        }
        for (o, v) in [(0u32, 0u16), (2, 0), (4, 480), (6, 640)] {
            m.set_u16(WINDOW + o, v).unwrap();
        }
        let g = Grid {
            size: (640, 480),
            bound: [(320, 240); 4],
            taps: [0, f(0.5), f(1.0), f(-1.0)],
            depth: f(1.0),
            radii: [0, 0],
            full: false,
        };
        (m, g)
    }

    #[test]
    fn each_tap_is_pulled_toward_the_centre_by_its_float() {
        let (m, g) = world();
        // The top-left corner.
        let v = vertex(&m, &g, 0, 0, 0x1f80, 0x027f).unwrap();
        assert_eq!(v.uv, [0, 0, 0, f(1.0)]);
        assert_eq!(&v.position[..2], &[0, 0]);
        // Tap 0, float 0: the corner moved by the spread (0), times the
        // ratio 0.5.
        assert_eq!(v.taps[0], [0, 0, 0, f(1.0)]);
        // Tap 1, 0.5: halfway to the centre (320, 240), times 0.5.
        assert_eq!(v.taps[1], [f(80.0), f(60.0), 0, f(1.0)]);
        // Tap 2, 1.0: the centre.
        assert_eq!(v.taps[2], [f(160.0), f(120.0), 0, f(1.0)]);
        // Tap 3, -1.0: as far again the other way.
        assert_eq!(v.taps[3], [f(-160.0), f(-120.0), 0, f(1.0)]);
        // The bottom right: x and y at the window's far corner.
        let v = vertex(&m, &g, COLUMNS, ROWS, 0x1f80, 0x027f).unwrap();
        assert_eq!(&v.position[..2], &[f(640.0), f(480.0)]);
        assert_eq!(v.taps[2], [f(160.0), f(120.0), 0, f(1.0)]);
    }

    #[test]
    fn the_radii_fade_the_pull_from_the_centre_out() {
        let (m, mut g) = world();
        g.radii = [f(0.25), f(0.75)];
        // The centre (10, 15 of 20, 30): inside the first radius, no pull.
        let c = vertex(&m, &g, 10, 15, 0x1f80, 0x027f).unwrap();
        assert_eq!(c.taps[2], [f(160.0), f(120.0), 0, f(1.0)], "the centre is the centre either way");
        assert_eq!(c.taps[1], [f(160.0), f(120.0), 0, f(1.0)]);
        // Halfway out in x (u 0.75, v 0.5): distance 0.5, weight 0.5.
        let h = vertex(&m, &g, 15, 15, 0x1f80, 0x027f).unwrap();
        // Tap 2 (1.0) weighted 0.5: halfway from 480 to 320, times 0.5.
        assert_eq!(h.taps[2], [f(200.0), f(120.0), 0, f(1.0)]);
        // A corner, past the second radius: the full pull.
        let k = vertex(&m, &g, 0, 0, 0x1f80, 0x027f).unwrap();
        assert_eq!(k.taps[2], [f(160.0), f(120.0), 0, f(1.0)]);
        // Inside the first radius the pull is 0: tap 2 at its own place.
        let near = vertex(&m, &g, 11, 15, 0x1f80, 0x027f).unwrap();
        assert_eq!(near.taps[2][1], f(120.0));
        assert_eq!(near.taps[2][0], f(352.0 * 0.5), "u 0.55, distance 0.1: no pull");
    }

    #[test]
    fn the_strips_cover_the_grid_and_push_their_words() {
        let (m, g) = world();
        let s = strips(&m, &g, 0x1f80, 0x027f).unwrap();
        assert_eq!(s.len(), ROWS as usize);
        assert!(s.iter().all(|r| r.len() == 2 * (COLUMNS as usize + 1)));
        let w = pushed(&s);
        assert_eq!(&w[..3], &[0x0004_17fc, 6, 0x0010_1a50]);
        assert_eq!(w.len(), ROWS as usize * (4 + 2 * (COLUMNS as usize + 1) * 30));
        let full = Grid { full: true, size: (320, 240), ..g };
        let v = vertex(&m, &full, COLUMNS, ROWS, 0x1f80, 0x027f).unwrap();
        assert_eq!(&v.position[..2], &[f(320.0), f(240.0)], "the target's own rectangle");
    }
}
