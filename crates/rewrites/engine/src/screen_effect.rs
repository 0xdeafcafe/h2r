//! What one screen effect draws: `0x0002d140` (docs/graphics/native-post-hud.md,
//! stage 5, "The screen effects") as a plan, a pure function over guest
//! memory, for the native renderer to draw it in the game's place.
//! [`crate::screen_effects::drawn`] says which effects draw and with what
//! values.
//!
//! (Code, `h2 disasm`.) `0x0002d140(values, columns, rows, corners)`
//! (`ret 0x10`, EAX the `egor` tag datum) copies the four values to
//! `0x004c19b0` and returns at once for a datum of -1, an empty window
//! (`0x004b9dd0`: top, left, bottom, right) or a target override
//! `[0x004858b8]`. The egor's shader (`+0x44`) gives through its template
//! the count of layer 15's implementations ([`implementations`]). Then
//! per entry of its pass block (`+0x88` count, `+0x8c` address,
//! [`PASS_SIZE`] bytes), [`plan`]:
//!
//! - the implementation (`+0x08`, -1 the pass's index), skipped outside
//!   the count, and the selector, one of the bytes `+0x0c..0x0f` by which
//!   of the first two values are not zero, skipped at -1;
//! - the target (`+0x60`): 0 the bound one, else target 1 at the window's
//!   size, a half or a quarter of it (`0x0001ddd0`, rounding toward zero);
//! - `0x00016610(shader, 15, implementation, selector, target != 0, 0)`,
//!   the shader's template pass, and the pass skipped when it fails (the
//!   plan assumes it draws: the device's swallow catches the other case);
//! - per row of corners a triangle strip pushed by hand (`SET_BEGIN_END`
//!   6): per column the corner in this row and the one below, each with
//!   four texture coordinates (`SET_VERTEX_DATA4F_M` attributes 1..4, the
//!   kinds `+0x50`, offsets and a scale flag from the block at `+0x5c`
//!   when `+0x58` is above 0) and its position by the target kind
//!   (`0x101a00`, z and w `[0x0045dd40]`);
//! - per entry of its convolution block (`+0xa4` count, `+0xa8` address,
//!   [`CONVOLUTION_SIZE`] bytes): kept (the result the pass's target)
//!   when its flag bit 0 or 1 is set and that value is 0; else sized from
//!   the target's (`0x0001dd60`) times `+0x58` (at least 64; past
//!   `0x12c00` texels both scaled by an x87 square root, at least 64
//!   each) into targets `0x19`/`0x1a` (the first) or `0x1b` and the one
//!   the first did not end in, and drawn by `0x00021360` (flag bit 2,
//!   [`crate::glow::wide`]) or the blur `0x000211a0`
//!   ([`crate::glow::blur`]); the result in
//!   `[0x00485a78 + 4 n]`.
//!
//! The arithmetic is the original's (SSE under the caller's MXCSR, x87
//! under its control word), checked against the lifted original by
//! `engine/tests/screen_effects_differential.rs`.

use crate::glow::{self, Blur, BlurArgs, Wide, WideArgs};
use crate::light_constants::{Error, Sse, X87};
use guest_layout::memory::GuestMemory;

/// The tag table: per datum index `0x10` bytes, `+0` the group, `+8` the
/// definition.
pub const TAG_TABLE: u32 = 0x004e_3b44;
/// The window (top, left, bottom, right shorts).
pub const WINDOW: u32 = 0x004b_9dd0;
/// A target override that, set, skips every screen effect.
pub const TARGET_OVERRIDE: u32 = 0x0048_58b8;
/// The render targets: [`TARGET_STRIDE`] bytes each, `+0x80`/`+0x84` the
/// size, `+0x88` the surface, `+0x95` a byte that, set, ignores it.
pub const TARGETS: u32 = 0x004b_4b58;
pub const TARGET_STRIDE: u32 = 0x98;
/// Where the convolutions' results go.
pub const RESULTS: u32 = 0x0048_5a78;
pub const PASS_SIZE: u32 = 0xac;
pub const CONVOLUTION_SIZE: u32 = 0x5c;
/// Image constants.
pub mod constant {
    /// 0.0, the values are compared with.
    pub const ZERO: u32 = 0x0045_dbd8;
    pub const ONE: u32 = 0x0045_dbc0;
    /// The smallest convolution side.
    pub const MIN_SIDE: u32 = 0x0045_dc90;
    /// The texel count a convolution is scaled to past `0x12c00`.
    pub const AREA: u32 = 0x0045_e4f8;
    /// The half and quarter targets' position scales.
    pub const HALF: u32 = 0x0045_dbbc;
    pub const QUARTER: u32 = 0x0045_dc20;
    /// The positions' z and w.
    pub const DEPTH: u32 = 0x0045_dd40;
    /// The offsets' scale (with the flag).
    pub const OFFSET_SCALE: u32 = 0x004b_9de4;
    /// Texture-coordinate kind 4's vector (a pointer to four words).
    pub const VECTOR: u32 = 0x0046_87c0;
}
/// The texel count past which a convolution is scaled down.
pub const AREA_CAP: i32 = 0x12c00;

/// The groups whose shader goes through a virtual call (`0x00137bd0`): not
/// planned.
const VIRTUAL_GROUPS: [u32; 2] = [0x5052_544d, 0x7072_7433];

/// One vertex the strip pushes: four texture coordinates and the
/// position's x and y (f32 bits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub texcoords: [[u32; 4]; 4],
    pub position: [u32; 2],
}

/// What a convolution entry does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Convolution {
    /// Not drawn (its value is 0): the result is the pass's target.
    Kept { result: u32 },
    /// The blur `0x000211a0` into `args.dest` and `args.other`, sized
    /// `size` (width, height).
    Blur { size: (i32, i32), args: BlurArgs, blur: Blur },
    /// `0x00021360` ([`crate::glow::wide`]): EAX the source, ECX
    /// `cvttss2si(+0x44)`, XMM0 `+0x48`, then the words `+0x50`, `+0x54`,
    /// the targets, -1, 1, 0, 0.
    Wide { size: (i32, i32), args: WideArgs, wide: Wide },
}

/// One pass of the effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pass {
    /// Its index in the egor's block.
    pub index: u32,
    /// `0x00016610`'s third argument: which of layer 15's passes it draws
    /// (the count [`Effect::implementations`] bounds).
    pub implementation: u32,
    /// `0x00016610`'s fourth argument: that pass's implementation.
    pub selector: i32,
    /// `+0x60`: 0 the bound target, 1..3 target 1 at the window's size, a
    /// half, a quarter.
    pub kind: u16,
    /// Target 1's size (width, height) for kinds 1..3.
    pub size: Option<(i32, i32)>,
    /// The strips, one per row of corners, in push order.
    pub strips: Vec<Vec<Vertex>>,
    pub convolutions: Vec<Convolution>,
}

/// The plan of one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effect {
    /// The egor's shader (its tag datum).
    pub shader: u32,
    /// Layer 15's pass count (the high 7 bits of the template's word).
    pub implementations: u32,
    pub passes: Vec<Pass>,
}

impl Pass {
    /// The words the strips push (each strip: `SET_BEGIN_END` 6, then per
    /// vertex four texture coordinates and the position); `depth` the
    /// position's z and w.
    pub fn pushed(&self, depth: u32) -> Vec<Vec<u32>> {
        self.strips
            .iter()
            .map(|strip| {
                let mut w = vec![0x0004_17fc, 6];
                for v in strip {
                    for (s, t) in v.texcoords.iter().enumerate() {
                        w.push(0x0010_0000 + 0x1a10 + 0x10 * s as u32);
                        w.extend_from_slice(t);
                    }
                    w.extend_from_slice(&[0x0010_1a00, v.position[0], v.position[1], depth, depth]);
                }
                w
            })
            .collect()
    }
}

fn def(mem: &dyn GuestMemory, datum: u32) -> Result<u32, Error> {
    let table = mem.u32(TAG_TABLE)?;
    Ok(mem.u32(table.wrapping_add((datum & 0xffff) << 4).wrapping_add(8))?)
}

/// Layer 15's implementation count for `shader` (`0x0002d1dd..0x0002d267`:
/// the template datum `+0x24` points at, its definition's block `+0x5c`
/// (the first element), the word its `+4` points at (low 9 bits), the
/// word that many plus 15 into its `+0xc`, its high 7 bits). `Err(Data)`
/// for the groups that go through a virtual call.
pub fn implementations(mem: &dyn GuestMemory, shader: u32) -> Result<u32, Error> {
    let table = mem.u32(TAG_TABLE)?;
    let index = i32::from(shader as u16 as i16);
    let group = mem.u32(table.wrapping_add((index as u32) << 4))?;
    if VIRTUAL_GROUPS.contains(&group) {
        return Err(Error::Data);
    }
    let template = mem.u32(mem.u32(def(mem, shader)?.wrapping_add(0x24))?)?;
    let element = mem.u32(def(mem, template)?.wrapping_add(0x5c))?;
    let at = u32::from(mem.u16(mem.u32(element.wrapping_add(4))?)? & 0x1ff);
    let words = mem.u32(element.wrapping_add(0xc))?;
    Ok(u32::from(mem.u16(words.wrapping_add(at * 2 + 0x1e))? >> 9))
}

/// The template layer `0x0002d140` draws through `0x00016610`.
pub const TEMPLATE_LAYER: u32 = 15;

/// Whether `0x00016610(shader, layer, implementation, selector, ..)`
/// draws, its gate as the original reads it (Code,
/// `0x00016610..0x000166ed`): the shader's template (the datum at its
/// definition's `+4`), its definition's `+0x5c` with a count of at least
/// one (as `dec`/`jns` tests it), `layer`'s bit in the 32-bit mask at `+2` of what its `+4` points
/// at, the word `layer` past that one's low 9 bits in the words at its
/// `+0xc` (its high 7 bits the passes, its low 9 the first), and of those
/// passes the one `implementation` names (-1 any): its record (ten bytes
/// each at `+0x14`) names a pass tag, whose definition's `+0x20` points
/// at a block whose `+4` holds the implementations (`0x132` bytes each),
/// and the one `selector` names draws unless its `+0x100` is -1 (no
/// vertex shader). `0x0002d140` skips a pass, strips and convolutions,
/// when it does not draw; [`plan`] assumes it does (the differential stubs
/// it drawing), so the caller checks each pass with this.
pub fn template_draws(mem: &dyn GuestMemory, shader: u32, layer: u32, implementation: i32, selector: i32) -> Result<bool, Error> {
    if shader == u32::MAX {
        return Ok(false);
    }
    let template = def(mem, mem.u32(def(mem, shader)?.wrapping_add(4))?)?;
    let element = mem.u32(template.wrapping_add(0x5c))?;
    // `dec ecx; jns`: the count less one not negative.
    if (mem.u32(element)?.wrapping_sub(1) as i32) < 0 {
        return Ok(false);
    }
    let layers = mem.u32(element.wrapping_add(4))?;
    if mem.u32(layers.wrapping_add(2))? & 1u32.wrapping_shl(layer) == 0 {
        return Ok(false);
    }
    let at = u32::from(mem.u16(layers)? & 0x1ff).wrapping_add(layer);
    let word = u32::from(mem.u16(mem.u32(element.wrapping_add(0xc))?.wrapping_add(at.wrapping_mul(2)))?);
    let records = mem.u32(element.wrapping_add(0x14))?;
    for pass in 0..(word >> 9) {
        if implementation != -1 && implementation as u32 != pass {
            continue;
        }
        let record = records.wrapping_add(((word & 0x1ff) + pass).wrapping_mul(10));
        let pass_def = def(mem, mem.u32(record.wrapping_add(4))?)?;
        let implementations = mem.u32(mem.u32(pass_def.wrapping_add(0x20))?.wrapping_add(4))?;
        let entry = implementations.wrapping_add((selector as u32).wrapping_mul(0x132));
        if mem.u32(entry.wrapping_add(0x100))? != u32::MAX {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The window as `0x0002d140` reads it: (top, left, bottom, right).
fn window(mem: &dyn GuestMemory) -> Result<[i32; 4], Error> {
    let s = |o: u32| -> Result<i32, Error> { Ok(i32::from(mem.u16(WINDOW + o)? as i16)) };
    Ok([s(0)?, s(2)?, s(4)?, s(6)?])
}

/// `0x0001dd60`: target `t`'s size, as the sizes set earlier in the call
/// (`set`) leave the table.
pub fn target_size(mem: &dyn GuestMemory, t: u32, set: &[(u32, (i32, i32))]) -> Result<(i32, i32), Error> {
    let e = TARGETS.wrapping_add(t.wrapping_mul(TARGET_STRIDE));
    if mem.u32(e + 0x88)? != 0 && mem.u8(e + 0x95)? == 0 {
        if let Some((_, s)) = set.iter().rev().find(|(n, _)| *n == t) {
            return Ok(*s);
        }
        return Ok((mem.u32(e + 0x80)? as i32, mem.u32(e + 0x84)? as i32));
    }
    Ok(if [0, 3, 0x18].contains(&t) { (0x280, 0x1e0) } else { (0, 0) })
}

/// The side `0x0002d9ab` takes: `scale * side`, at least the smallest.
fn side(sse: &mut Sse, scale: u32, side: u32, min: u32) -> Result<i32, Error> {
    let v = sse.mul(scale, side)?;
    let v = if sse.above(v, min)? { v } else { min };
    cvtt(v)
}

/// `cvttss2si`: the integer indefinite for NaN and out of range.
fn cvtt(bits: u32) -> Result<i32, Error> {
    let f = f32::from_bits(bits);
    Ok(if f.is_nan() || f >= 2_147_483_648.0 || f < -2_147_483_648.0 { i32::MIN } else { f as i32 })
}

/// `_ftol2` (`0x00322b0c`) of ST0, popped: toward zero.
fn ftol(x: &mut X87) -> Result<i32, Error> {
    let (code, store) = x87::abi::st_i32_trunc(&x.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x.0, store.status, true);
    Ok(store.value)
}

/// One convolution's size from its source's (`0x0002d984..0x0002da3c`).
fn convolution_size(
    mem: &dyn GuestMemory,
    sse: &mut Sse,
    x87_control: u16,
    scale: u32,
    (w, h): (i32, i32),
) -> Result<(i32, i32), Error> {
    let min = mem.u32(constant::MIN_SIDE)?;
    let (fw, fh) = (sse.int(w)?, sse.int(h)?);
    let mut width = side(sse, scale, fw, min)?;
    let mut height = side(sse, scale, fh, min)?;
    if height.wrapping_mul(width) > AREA_CAP {
        let mut x = X87::new(x87_control);
        x.fild(h.wrapping_mul(w))?;
        x.arith_m(x87::abi::arith_op::DIVR, mem.u32(constant::AREA)?)?;
        x.unary(x87::abi::unary_op::SQRT)?;
        x.fld(fw)?;
        x.arith_reg(x87::abi::arith_op::MUL, 0, 1, false)?;
        width = ftol(&mut x)?;
        x.fmul_m(fh)?;
        height = ftol(&mut x)?;
        if width < 64 || height < 64 {
            width = 64;
            height = 64;
        }
    }
    Ok((width, height))
}

/// A vertex at `corner` (x, y, u, v) for the pass at `p`.
fn vertex(
    mem: &dyn GuestMemory,
    sse: &mut Sse,
    p: u32,
    corner: [u32; 4],
    bounds: [u32; 4],
    kind: u16,
) -> Result<Vertex, Error> {
    let [left, right, top, bottom] = bounds;
    let one = mem.u32(constant::ONE)?;
    let offsets = if mem.u32(p + 0x58)? as i32 > 0 { mem.u32(p + 0x5c)? } else { 0 };
    let mut texcoords = [[0u32; 4]; 4];
    for (s, t) in texcoords.iter_mut().enumerate() {
        let s = s as u32;
        let (mut u, mut v): (u32, u32);
        let (mut z, mut w) = (0u32, one);
        match mem.u16(p + 0x50 + 2 * s)? {
            0 => (u, v) = (corner[2], corner[3]),
            1 => {
                let du = sse.sub(corner[0], left)?;
                let dw = sse.sub(right, left)?;
                u = sse.div(du, dw)?;
                let dv = sse.sub(corner[1], top)?;
                let dh = sse.sub(bottom, top)?;
                v = sse.div(dv, dh)?;
            }
            2 => (u, v) = (sse.sub(corner[0], left)?, sse.sub(corner[1], top)?),
            3 => (u, v) = (corner[0], corner[1]),
            4 => {
                let at = mem.u32(constant::VECTOR)?;
                (u, v, z, w) = (mem.u32(at)?, mem.u32(at + 4)?, mem.u32(at + 8)?, mem.u32(at + 12)?);
            }
            _ => return Err(Error::Data),
        }
        if offsets != 0 {
            let o = offsets + 8 + 0x10 * s;
            u = sse.add(mem.u32(o)?, u)?;
            v = sse.add(mem.u32(o + 4)?, v)?;
            z = sse.add(mem.u32(o + 8)?, z)?;
            w = sse.add(mem.u32(o + 12)?, w)?;
            if mem.u8(offsets + 2 * s)? & 1 != 0 {
                let k = mem.u32(constant::OFFSET_SCALE)?;
                u = sse.mul(u, k)?;
                v = sse.mul(v, k)?;
            }
        }
        *t = [u, v, z, w];
    }
    let (x, y) = match kind {
        0 => (corner[0], corner[1]),
        1 => (sse.sub(corner[0], left)?, sse.sub(corner[1], top)?),
        2 | 3 => {
            let k = mem.u32(if kind == 2 { constant::HALF } else { constant::QUARTER })?;
            let x = sse.sub(corner[0], left)?;
            let y = sse.sub(corner[1], top)?;
            (sse.mul(x, k)?, sse.mul(y, k)?)
        }
        _ => return Err(Error::Data),
    };
    Ok(Vertex { texcoords, position: [x, y] })
}

/// Build one strip from the screen effect's live pass and corner rows.
pub fn strip_vertices(
    mem: &dyn GuestMemory,
    pass: u32,
    bounds: [u32; 4],
    rows: &[[[u32; 4]; 2]],
    mxcsr: u32,
) -> Result<Vec<Vertex>, Error> {
    let kind = mem.u16(pass.checked_add(0x60).ok_or(Error::Data)?)?;
    let mut sse = Sse { mxcsr };
    rows.iter().flatten().map(|&corner| vertex(mem, &mut sse, pass, corner, bounds, kind)).collect()
}

/// The most passes and convolutions a plan takes.
pub const MAX_PASSES: u32 = 64;

/// `0x0002d140` for the egor `datum` with `values`, `corners` (`columns`
/// per row, `rows` rows; x, y, u, v each), under `mxcsr` and
/// `x87_control`. `None` when it returns at once. `Err(Data)` for what it
/// cannot plan (a shader through a virtual call, a kind past the jump
/// tables); `Err(Unbounded)` past [`MAX_PASSES`] passes or convolutions,
/// or a blur or `0x00021360` past its budget.
pub fn plan(
    mem: &dyn GuestMemory,
    datum: u32,
    values: [u32; 4],
    columns: u32,
    rows: u32,
    corners: &[[u32; 4]],
    mxcsr: u32,
    x87_control: u16,
) -> Result<Option<Effect>, Error> {
    plan_with(mem, datum, values, columns, rows, corners, mxcsr, x87_control, &|_| {})
}

/// [`plan`] with each blur's arguments given to `blur_args` before the
/// blur is planned, as a hook before `0x000211a0` called from
/// `0x0002da9b` would change its stack words (the converted Halo 1 scope,
/// `render_bridge::scope_zoom_hud::zoom_blur`). Its result feeds the next
/// convolution's buffers as the guest's EAX does.
pub fn plan_with(
    mem: &dyn GuestMemory,
    datum: u32,
    values: [u32; 4],
    columns: u32,
    rows: u32,
    corners: &[[u32; 4]],
    mxcsr: u32,
    x87_control: u16,
    blur_args: &dyn Fn(&mut BlurArgs),
) -> Result<Option<Effect>, Error> {
    if datum == u32::MAX {
        return Ok(None);
    }
    let [top, left, bottom, right] = window(mem)?;
    if left >= right || top >= bottom || mem.u32(TARGET_OVERRIDE)? != 0 {
        return Ok(None);
    }
    let mut sse = Sse { mxcsr };
    let egor = def(mem, datum)?;
    let shader = mem.u32(egor + 0x44)?;
    let implementations = implementations(mem, shader)?;
    let (width, height) = (right - left, bottom - top);
    let bounds = [sse.int(left)?, sse.int(right)?, sse.int(top)?, sse.int(bottom)?];
    let zero = mem.u32(constant::ZERO)?;
    let count = mem.u32(egor + 0x88)? as i32;
    if count > MAX_PASSES as i32 {
        return Err(Error::Unbounded);
    }
    let mut passes = Vec::new();
    let mut set: Vec<(u32, (i32, i32))> = Vec::new();
    for i in 0..count.max(0) as u32 {
        let p = mem.u32(egor + 0x8c)?.wrapping_add(i * PASS_SIZE);
        let own = mem.u16(p + 8)?;
        let implementation = if own == 0xffff { i as i32 } else { i32::from(own as i16) };
        let first = !sse.equal(values[0], 0)?;
        let second = !sse.equal(values[1], 0)?;
        let byte = match (first, second) {
            (false, false) => 0xc,
            (false, true) => 0xe,
            (true, false) => 0xd,
            (true, true) => 0xf,
        };
        let selector = i32::from(mem.u8(p + byte)? as i8);
        if implementation < 0 || implementation > implementations as i32 - 1 || selector == -1 {
            continue;
        }
        let kind = mem.u16(p + 0x60)?;
        let size = match kind {
            0 => None,
            1 => Some((width, height)),
            2 => Some((width / 2, height / 2)),
            3 => Some((width / 4, height / 4)),
            _ => return Err(Error::Data),
        };
        if let Some(s) = size {
            set.push((1, s));
        }
        let mut strips = Vec::new();
        for j in 0..rows.saturating_sub(1) {
            let mut strip = Vec::new();
            for k in 0..columns {
                for row in [j, j + 1] {
                    let c = corners.get((row * columns + k) as usize).ok_or(Error::Data)?;
                    strip.push(vertex(mem, &mut sse, p, *c, bounds, kind)?);
                }
            }
            strips.push(strip);
        }
        let target = u32::from(kind != 0);
        let mut convolutions = Vec::new();
        // `[0x00485a78]`: the first convolution's result.
        let mut first_result = 0;
        let n_conv = mem.u32(p + 0xa4)? as i32;
        if n_conv > MAX_PASSES as i32 {
            return Err(Error::Unbounded);
        }
        for n in 0..n_conv.max(0) as u32 {
            let c = mem.u32(p + 0xa8)?.wrapping_add(n * CONVOLUTION_SIZE);
            let (dest, other) = if n == 0 {
                (0x19, 0x1a)
            } else {
                (0x1b, 0x19 + u32::from(first_result == 0x19))
            };
            let flags = mem.u16(c)?;
            let kept = (flags & 1 != 0 && sse.equal(values[0], zero)?)
                || (flags & 2 != 0 && sse.equal(values[1], zero)?);
            let (conv, result) = if kept {
                (Convolution::Kept { result: target }, target)
            } else {
                let source_size = target_size(mem, target, &set)?;
                let size = convolution_size(mem, &mut sse, x87_control, mem.u32(c + 0x58)?, source_size)?;
                set.push((dest, size));
                set.push((other, size));
                if flags & 4 != 0 {
                    let args = WideArgs {
                        source: target,
                        taps: cvtt(mem.u32(c + 0x44)?)?,
                        scale: mem.u32(c + 0x48)?,
                        words: [mem.u32(c + 0x50)?, mem.u32(c + 0x54)?],
                        dest,
                        other,
                        last: u32::MAX,
                    };
                    let wide = glow::wide(mem, &args, mxcsr)?;
                    let result = wide.result;
                    (Convolution::Wide { size, args, wide }, result)
                } else {
                    let mut args = BlurArgs {
                        source: target,
                        steps: mem.u32(c + 0x44)?,
                        scale: mem.u32(c + 0x48)?,
                        growth: mem.u32(c + 0x4c)?,
                        dest,
                        other,
                        last: u32::MAX,
                    };
                    blur_args(&mut args);
                    let blur = glow::blur(mem, &args, mxcsr)?;
                    let result = blur.result;
                    (Convolution::Blur { size, args, blur }, result)
                }
            };
            if n == 0 {
                first_result = result;
            }
            convolutions.push(conv);
        }
        passes.push(Pass {
            index: i,
            implementation: implementation as u32,
            selector,
            kind,
            size,
            strips,
            convolutions,
        });
    }
    Ok(Some(Effect {
        shader,
        implementations,
        passes,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const TT: u32 = 0x0060_0000;
    const EGOR: u32 = 0x0061_0000;
    const SDEF: u32 = 0x0061_1000;
    const TDEF: u32 = 0x0061_2000;
    const X: u32 = 0x0061_3000;
    const PASSES: u32 = 0x0062_0000;
    const OFFSETS: u32 = 0x0063_0000;
    const CONVS: u32 = 0x0064_0000;
    const DATUM: u32 = 0xe000_0003;
    const SHADER: u32 = 0xe001_0005;

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    /// One egor with one pass into a half-size target: kinds 0..3, set
    /// 1's offsets scaled, one blur at half its source.
    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0030_0000);
        for (a, v) in [
            (constant::ONE, f(1.0)),
            (constant::MIN_SIDE, f(64.0)),
            (constant::HALF, f(0.5)),
            (constant::QUARTER, f(0.25)),
            (constant::OFFSET_SCALE, f(2.0)),
            (crate::glow::constant::SPREAD, f(2.0)),
        ] {
            m.set_u32(a, v).unwrap();
        }
        for (o, v) in [(0u32, 0u16), (2, 0), (4, 480), (6, 640)] {
            m.set_u16(WINDOW + o, v).unwrap();
        }
        m.set_u32(TAG_TABLE, TT).unwrap();
        m.set_u32(TT + 3 * 16 + 8, EGOR).unwrap();
        m.set_u32(TT + 5 * 16, 0x7264_6873).unwrap();
        m.set_u32(TT + 5 * 16 + 8, SDEF).unwrap();
        m.set_u32(SDEF + 0x24, SDEF + 0x100).unwrap();
        m.set_u32(SDEF + 0x100, 0xe002_0006).unwrap();
        m.set_u32(TT + 6 * 16 + 8, TDEF).unwrap();
        m.set_u32(TDEF + 0x5c, X).unwrap();
        m.set_u32(X + 4, X + 0x100).unwrap();
        m.set_u16(X + 0x100, 0xfe02).unwrap(); // index 2 (low 9 bits)
        m.set_u32(X + 0xc, X + 0x200).unwrap();
        m.set_u16(X + 0x200 + 2 * 2 + 0x1e, 3 << 9).unwrap();
        m.set_u32(EGOR + 0x44, SHADER).unwrap();
        m.set_u32(EGOR + 0x88, 1).unwrap();
        m.set_u32(EGOR + 0x8c, PASSES).unwrap();
        let p = PASSES;
        m.set_u16(p + 8, 0xffff).unwrap();
        m.write(p + 0xc, &[0xff, 7, 1, 2]).unwrap();
        for (s, k) in [0u16, 1, 2, 3].iter().enumerate() {
            m.set_u16(p + 0x50 + 2 * s as u32, *k).unwrap();
        }
        m.set_u32(p + 0x58, 1).unwrap();
        m.set_u32(p + 0x5c, OFFSETS).unwrap();
        m.set_u16(OFFSETS + 2, 1).unwrap(); // set 1 scaled
        m.set_u32(OFFSETS + 8 + 0x10, f(0.5)).unwrap(); // set 1's u offset
        m.set_u16(p + 0x60, 2).unwrap();
        m.set_u32(p + 0xa4, 1).unwrap();
        m.set_u32(p + 0xa8, CONVS).unwrap();
        m.set_u32(CONVS + 0x44, f(1.5)).unwrap();
        m.set_u32(CONVS + 0x48, f(1.0)).unwrap();
        m.set_u32(CONVS + 0x4c, f(0.25)).unwrap();
        m.set_u32(CONVS + 0x58, f(0.5)).unwrap();
        // Target 1 has a surface: its size is the pass's.
        m.set_u32(TARGETS + TARGET_STRIDE + 0x88, 1).unwrap();
        m
    }

    fn corners() -> Vec<[u32; 4]> {
        vec![
            [f(0.0), f(0.0), 0, 0],
            [f(640.0), f(0.0), f(1.0), 0],
            [f(0.0), f(480.0), 0, f(1.0)],
            [f(640.0), f(480.0), f(1.0), f(1.0)],
        ]
    }

    fn run(m: &FlatMemory, values: [u32; 4]) -> Option<Effect> {
        plan(m, DATUM, values, 2, 2, &corners(), 0x1f80, 0x027f).unwrap()
    }

    /// `0x00016610`'s gate over the test world's template: layer 15's
    /// word (the one [`implementations`] reads) gives three passes from
    /// record 0; pass 1's implementation 2 has a vertex shader.
    #[test]
    fn the_template_pass_draws_only_an_implementation_with_a_vertex_shader() {
        const PDEF: u32 = 0x0061_4000;
        const IMPLS: u32 = 0x0061_5000;
        let mut m = world();
        m.set_u32(SDEF + 4, 0xe002_0006).unwrap();
        m.set_u32(X, 1).unwrap();
        m.set_u32(X + 0x102, 1 << TEMPLATE_LAYER).unwrap();
        m.set_u32(X + 0x14, X + 0x400).unwrap();
        for pass in 0..3u32 {
            m.set_u32(X + 0x400 + 10 * pass + 4, 0xe003_0007).unwrap();
        }
        m.set_u32(TT + 7 * 16 + 8, PDEF).unwrap();
        m.set_u32(PDEF + 0x20, PDEF + 0x100).unwrap();
        m.set_u32(PDEF + 0x104, IMPLS).unwrap();
        for sel in 0..4u32 {
            m.set_u32(IMPLS + 0x132 * sel + 0x100, u32::MAX).unwrap();
        }
        m.set_u32(IMPLS + 0x132 * 2 + 0x100, 5).unwrap();
        let draws = |m: &FlatMemory, imp, sel| template_draws(m, SHADER, TEMPLATE_LAYER, imp, sel).unwrap();
        assert!(draws(&m, 1, 2), "a vertex shader");
        assert!(!draws(&m, 1, 1), "+0x100 -1: none, so 0x0002d140 skips the pass");
        assert!(draws(&m, -1, 2), "-1 takes any pass");
        assert!(!draws(&m, 3, 2), "past the layer's passes");
        assert!(!template_draws(&m, u32::MAX, TEMPLATE_LAYER, 1, 2).unwrap());
        let mut no_layer = m.clone();
        no_layer.set_u32(X + 0x102, 1 << 14).unwrap();
        assert!(!draws(&no_layer, 1, 2), "the template has no layer 15");
        let mut empty = m.clone();
        empty.set_u32(X, 0).unwrap();
        assert!(!draws(&empty, 1, 2), "a count below one");
        empty.set_u32(X, 0x8000_0000).unwrap();
        assert!(draws(&empty, 1, 2), "dec/jns: 0x80000000 less one is positive");
    }

    #[test]
    fn a_pass_into_a_half_target() {
        let m = world();
        let e = run(&m, [f(1.0), 0, 0, 0]).expect("draws");
        assert_eq!((e.shader, e.implementations), (SHADER, 3));
        let p = &e.passes[0];
        assert_eq!((p.implementation, p.selector, p.kind, p.size), (0, 7, 2, Some((320, 240))), "the first value only: byte +0x0d");
        let strip = &p.strips[0];
        assert_eq!(strip.len(), 4);
        // Top left, bottom left, top right, bottom right.
        assert_eq!(strip[1].position, [f(0.0), f(240.0)], "half the window");
        assert_eq!(strip[2].position, [f(320.0), f(0.0)]);
        let br = strip[3];
        assert_eq!(br.texcoords[0], [f(1.0), f(1.0), 0, f(1.0)], "the corner's own");
        assert_eq!(br.texcoords[1], [f(3.0), f(2.0), 0, f(1.0)], "the window's fraction, offset, then doubled");
        assert_eq!(br.texcoords[2], [f(640.0), f(480.0), 0, f(1.0)], "pixels from the window's corner");
        let pushed = p.pushed(f(16_777_215.0));
        assert_eq!(&pushed[0][..2], &[0x0004_17fc, 6]);
        assert_eq!(pushed[0].len(), 2 + 4 * 25);
        assert_eq!(pushed[0][2], 0x0010_1a10, "attribute 1 first");
        match &p.convolutions[0] {
            Convolution::Blur { size, args, blur } => {
                assert_eq!(*size, (160, 120), "half its source, the pass's target");
                assert_eq!((args.source, args.dest, args.other, args.last), (1, 0x19, 0x1a, u32::MAX));
                assert_eq!(blur.draws.len(), 2, "1.5 steps");
            }
            c => panic!("{c:?}"),
        }
    }

    #[test]
    fn a_blur_takes_its_changed_arguments() {
        let m = world();
        let with = |adjust: &dyn Fn(&mut BlurArgs)| {
            let e = plan_with(&m, DATUM, [f(1.0), 0, 0, 0], 2, 2, &corners(), 0x1f80, 0x027f, adjust).unwrap().unwrap();
            match e.passes[0].convolutions[0].clone() {
                Convolution::Blur { args, blur, .. } => (args, blur),
                c => panic!("{c:?}"),
            }
        };
        let (args, blur) = with(&|a| a.steps = 0);
        assert_eq!(args.steps, 0);
        assert!(blur.draws.is_empty(), "no steps: no draws");
        assert_eq!(blur.result, 1, "the source unchanged");
        let (args, blur) = with(&|a| a.scale = f(3.0));
        assert_eq!(args.scale, f(3.0));
        assert_eq!(blur.draws.len(), 2, "the tag's 1.5 steps");
        assert_eq!(blur.draws[0].spread, f(3.0), "the first draw at the new scale");
        assert_eq!(with(&|_| {}), {
            let e = run(&m, [f(1.0), 0, 0, 0]).unwrap();
            match e.passes[0].convolutions[0].clone() {
                Convolution::Blur { args, blur, .. } => (args, blur),
                c => panic!("{c:?}"),
            }
        }, "unchanged arguments: plan's");
    }

    #[test]
    fn the_selector_and_implementation_skip_passes() {
        let mut m = world();
        assert!(run(&m, [0, 0, 0, 0]).unwrap().passes.is_empty(), "byte +0x0c is -1");
        assert_eq!(run(&m, [0, f(1.0), 0, 0]).unwrap().passes[0].selector, 1);
        m.set_u16(PASSES + 8, 3).unwrap();
        assert!(run(&m, [f(1.0), 0, 0, 0]).unwrap().passes.is_empty(), "past the count");
        m.set_u16(PASSES + 8, 2).unwrap();
        assert_eq!(run(&m, [f(1.0), 0, 0, 0]).unwrap().passes[0].implementation, 2);
    }

    #[test]
    fn a_convolution_keeps_the_target_when_its_value_is_zero() {
        let mut m = world();
        m.set_u16(CONVS, 2).unwrap();
        let e = run(&m, [f(1.0), 0, 0, 0]).unwrap();
        assert_eq!(e.passes[0].convolutions[0], Convolution::Kept { result: 1 });
        m.set_u16(CONVS, 4).unwrap();
        assert!(matches!(run(&m, [f(1.0), 0, 0, 0]).unwrap().passes[0].convolutions[0], Convolution::Wide { .. }));
    }

    #[test]
    fn it_returns_at_once() {
        let mut m = world();
        assert_eq!(plan(&m, u32::MAX, [0; 4], 2, 2, &corners(), 0x1f80, 0x027f).unwrap(), None);
        m.set_u32(TARGET_OVERRIDE, 1).unwrap();
        assert_eq!(run(&m, [f(1.0), 0, 0, 0]), None);
        m.set_u32(TARGET_OVERRIDE, 0).unwrap();
        m.set_u16(WINDOW + 6, 0).unwrap();
        assert_eq!(run(&m, [f(1.0), 0, 0, 0]), None, "an empty window");
        let mut m = world();
        m.set_u32(TT + 5 * 16, 0x5052_544d).unwrap();
        assert_eq!(plan(&m, DATUM, [f(1.0), 0, 0, 0], 2, 2, &corners(), 0x1f80, 0x027f), Err(Error::Data));
    }

    #[test]
    fn a_large_convolution_is_scaled_to_its_area() {
        let m = world();
        let mut sse = Sse { mxcsr: 0x1f80 };
        assert_eq!(convolution_size(&m, &mut sse, 0x027f, f(0.25), (640, 480)).unwrap(), (160, 120));
        // 640 x 480 is past 0x12c00: both sides times sqrt(area / 307200),
        // here an area of 0, so the smallest.
        assert_eq!(convolution_size(&m, &mut sse, 0x027f, f(1.0), (640, 480)).unwrap(), (64, 64));
        let mut m = m;
        m.set_u32(constant::AREA, f(76_800.0)).unwrap();
        assert_eq!(convolution_size(&m, &mut sse, 0x027f, f(1.0), (640, 480)).unwrap(), (320, 240), "half each side");
    }
}
