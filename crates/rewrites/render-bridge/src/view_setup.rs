//! The view setup `0x000132d0` and the view end `0x00013c20` in Rust
//! (docs/graphics/retire-d3d-display.md, "The view setup and view end").
//! Every frame producer reaches guest D3D through this pair: the game
//! views, the views outside a game view (`0x0002c8a0`, `0x0002ca00`) and
//! the view end's return to the view below.
//!
//! The setup is planned from guest memory ([`plan`]), with nothing written,
//! then made ([`apply`]): the game-side words, then the device's calls in
//! the body's order through [`native_targets::Host`], so it makes no guest
//! D3D entry. The bind and the clear are [`native_targets::cut_select`]'s
//! and [`native_targets::cut_clear`]'s, on the targets the frame graph
//! declares (`native_render::frame_graph::targets`).
//!
//! No guest body runs. The fog's effect `0x00027aa0` (the view's
//! `+0x1b8` not 0 or -1) is game code with no D3D call: the setup calls it
//! as the body does, last. A setup the cut cannot make panics, named
//! `view_setup: <cause>` (`host`, `unread`, `unwritable`, `device`: a
//! refused call); the end likewise, `view_end: <cause>`. Counted
//! `view_setup_calls` and `view_end_calls`.

use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};

use crate::native_targets::{self, unhandled, Arm, Host};

/// The render context the view is copied to, and the view's size.
pub const CONTEXT: u32 = 0x0048_5600;
pub const VIEW_SIZE: u32 = 0x298;
/// The view's target id (0, or `0x12` for view kind 1).
pub const TARGET: u32 = 0x0048_58b8;
/// The stencil helper `0x00015370`'s mirrors: the enable and the mode.
pub const STENCIL_ENABLE: u32 = 0x004b_843c;
pub const STENCIL_MODE: u32 = 0x0046_7010;
/// The two words the setup sets before the transform.
pub const SETUP_WORDS: [(u32, u32); 2] = [(0x0050_93b4, 0x901), (0x0050_93b8, 0x900)];
/// The guest's shadow of the constants, written unless bit 4 of
/// [`CONSTANT_FLAGS`] is set.
pub const TRANSFORM_SHADOW: u32 = 0x0040_7738;
pub const FOG_SHADOW: u32 = 0x0040_77f8;
pub const CONSTANT_FLAGS: u32 = 0x0040_4fe8;
pub const FILL_MODE: u32 = 0x004b_8428;
pub const DRAW_BEGIN: u32 = 0x0046_70bc;
/// The draw state `0x00016b10` resets.
pub const DRAW_STATE: u32 = 0x0048_5b48;
/// The fog's block (the view's `+0x14c`), its colour and the camera it
/// reads.
pub const FOG_BLOCK: u32 = 0x0048_574c;
pub const FOG_COLOUR: u32 = 0x004b_8424;
pub const CAMERA: u32 = 0x004b_9da0;
/// The view end's gate on the alpha clear, its stack of views and their
/// records.
pub const ALPHA_GATE: u32 = 0x004b_a04c;
pub const VIEW_STACK: u32 = 0x0046_701c;
pub const VIEW_RECORDS: u32 = 0x0048_50d0;
/// The view end's camera copies and calls.
pub const CAMERA_KIND: u32 = 0x004b_9ed4;
pub const CAMERA_FOG: u32 = 0x004b_9ef0;
pub const CAMERA_POINTS: u32 = 0x004b_9e14;
pub const CAMERA_UPDATE: u32 = 0x0002_f970;
pub const CAMERA_FINISH: u32 = 0x0002_fd90;

/// The target-0 surfaces the target switch `0x00014bc0` binds; target
/// `0x12` binds the cache's surface for it ([`cached`]) on the same depth.
pub const VIEW_COLOUR: u32 = 0x0050_935c;
pub const VIEW_DEPTH: u32 = 0x0050_9364;
/// View kind 1's target.
pub const CACHED_TARGET: u32 = 0x12;
/// The render-target cache `0x0001dcf0` reads: records of `0x98` bytes.
pub const TARGET_CACHE: u32 = 0x004b_4b58;
pub const TARGET_RECORD: u32 = 0x98;
/// The screen-target pick `0x00025960` reads: two records' live word and
/// their off byte.
pub const SCREEN_A: (u32, u32) = (0x004b_57c0, 0x004b_57cd);
pub const SCREEN_B: (u32, u32) = (0x004b_5690, 0x004b_569d);
/// The fog's effect, called last when the view's `+0x1b8` is not 0 or -1.
pub const FOG_EFFECT: u32 = 0x0002_7aa0;
/// The D3D render-state array, the deferred states' dirty word, and the
/// tables of the simple states' push headers and the deferred states'
/// dirty bits the restore `0x00018900` reads.
pub const RENDER_STATES: u32 = 0x0040_7498;
pub const DEFERRED_DIRTY: u32 = 0x0040_7490;
pub const SIMPLE_HEADERS: u32 = 0x0043_e6f0;
pub const DEFERRED_BITS: u32 = 0x0043_e4d0;
/// The restore's slot ranges: below, simple; below, deferred; the rest
/// through their setters.
pub const SIMPLE_SLOTS: u8 = 0x5c;
pub const DEFERRED_SLOTS: u8 = 0x88;
/// The device pointer, the push-buffer threshold `MakeRequestedSpace` is
/// asked for, and the device's clip range and its flags (`+0x1c18` bit 1:
/// the game owns it).
pub const DEVICE: u32 = native_targets::DEVICE;
pub const PUSH_THRESHOLD: u32 = 0x0040_8648;
pub const DEVICE_CLIP: u32 = 0x1c10;

/// Guest render-state slots (`d3d::abi::RENDER_STATE_SETTERS`).
pub const SLOT_FOG_COLOUR: u32 = 138;
pub const SLOT_FILL_MODE: u32 = 139;
pub const SLOT_STENCIL_ENABLE: u32 = 144;
/// `D3DFILL_SOLID`.
pub const SOLID: u32 = 0x1b02;

/// The constants the setup reads.
const ONE: u32 = 0x0045_dbc0;
const HALF: u32 = 0x0045_dbbc;
const MINUS_HALF: u32 = 0x0045_dbc4;
const SCALE: u32 = 0x0046_70c8;
const SCALE_MIN: u32 = 0x0045_df78;
const SCALE_OFF: u32 = 0x0050_93fc;
const Z_OFFSET: u32 = 0x0045_de50;
const KIND2_EXTENT: u32 = 0x0045_dd60;
const ROW_SCALE: u32 = 0x0045_dd40;
const TRANSFORM_27: u32 = 0x0045_dc0c;
const TRANSFORM_31: u32 = 0x0045_e460;
const PUSH_W: u32 = 0x0045_e3b0;
const CLIP_NEAR: u32 = 0x0048_5adc;
const CLIP_FAR: u32 = 0x0048_5ae0;
const CLEAR_PLAIN: u32 = 0x0046_70cd;
const COLOUR_SCALE: u32 = 0x0045_dc38;
const RESET_SCALAR: u32 = 0x0045_dbd4;
const FOG_FLOOR: u32 = 0x0045_dbdc;

/// The words of guest stack below the call the cut stages the device's
/// arguments in, as the body's own frame would hold them.
pub const SCRATCH: u32 = 0x200;

pub static VIEW_SETUP: Arm = Arm::new("view_setup");
pub static VIEW_END: Arm = Arm::new("view_end");

type Cause = &'static str;

fn unread(_: MemFault) -> Cause {
    "unread"
}

fn unwritable(_: MemFault) -> Cause {
    "unwritable"
}

fn word(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    mem.u32(at).map_err(unread)
}

fn float(mem: &dyn GuestMemory, at: u32) -> Result<f32, Cause> {
    mem.f32(at).map_err(unread)
}

fn short(mem: &dyn GuestMemory, at: u32) -> Result<i32, Cause> {
    mem.u16(at).map(|v| i32::from(v as i16)).map_err(unread)
}

fn byte(mem: &dyn GuestMemory, at: u32) -> Result<u8, Cause> {
    mem.u8(at).map_err(unread)
}

/// One game-side write the body makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub at: u32,
    pub bytes: Vec<u8>,
}

fn words_le(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The fog `0x00025a10` sets: the constants from register 12 - 96 and the
/// colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Fog {
    pub block: [u32; 24],
    pub colour: u32,
}

/// What one setup makes, planned with nothing written.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    pub view: u32,
    /// The game-side words, in the body's order.
    pub writes: Vec<Write>,
    /// The device's clip range and its flags (`+0x1c10..+0x1c18`).
    pub clip: [u32; 3],
    /// `c[-96..-84]`: the view-projection rows and the view's constants.
    pub transform: [u32; 48],
    pub colour: u32,
    pub depth: u32,
    /// The target select's flag byte: 1 for target 0, 0 for `0x12`.
    pub flag: u8,
    /// `x, y, width, height, min_z, max_z`.
    pub viewport: [u32; 6],
    /// The clear's game flags and colour.
    pub clear: Option<(u32, u32)>,
    /// The raw push: method `0x1af0`, four words (0, `[0x0045dbbc]`,
    /// `[0x0045dbc0]`, `[0x0045e3b0]`).
    pub push: [u32; 5],
    /// The draw state's restore list, in its order.
    pub restore: Vec<Restore>,
    pub fog: Option<Fog>,
    /// Whether the fog's effect [`FOG_EFFECT`] is called last.
    pub fog_effect: bool,
}

/// One entry of the draw state's restore list (`0x00018900`): a slot byte
/// and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restore {
    /// Below [`SIMPLE_SLOTS`]: `SetRenderState_Simple(header, value)`,
    /// then the slot's word.
    Simple { slot: u8, header: u32, value: u32 },
    /// Below [`DEFERRED_SLOTS`]: the dirty bits or'd in and the slot's word.
    Deferred { slot: u8, bits: u32, value: u32 },
    /// The rest: the slot's `SetRenderState_*` setter.
    Setter { slot: u8, value: u32 },
}

/// The scale `0x00012fa0` and `0x00014bc0` clamp: `[0x004670c8]` into
/// `[0x0045df78]..1.0` when the view's byte `+7` is set and `[0x005093fc]`
/// is not, else 1.0. As the guest compares, a NaN passes through.
fn view_scale(mem: &dyn GuestMemory, view: u32) -> Result<f32, Cause> {
    let one = float(mem, ONE)?;
    if byte(mem, view.wrapping_add(7))? == 0 || byte(mem, SCALE_OFF)? != 0 {
        return Ok(one);
    }
    let (lo, x) = (float(mem, SCALE_MIN)?, float(mem, SCALE)?);
    Ok(if lo > x {
        lo
    } else if x > one {
        one
    } else {
        x
    })
}

/// The view-projection rows `0x00012d50` builds with BL 0 for the view's
/// matrices at `m` (its `+0x8c`): per row `i`, the projection column at
/// `+0x78 + 4i` through the 3x4 matrix at `+4`, times `[0x0045dd40]` over
/// the context's `+0x5c` (here the view's, which the context copies).
/// The operations in the body's order (`0x00012de2..0x00012edc`).
pub fn rows(mem: &dyn GuestMemory, view: u32) -> Result<[f32; 16], Cause> {
    let m = view.wrapping_add(0x8c);
    let s = float(mem, ROW_SCALE)? / float(mem, view.wrapping_add(0x5c))?;
    let v = |k: u32| float(mem, m.wrapping_add(4 * k));
    let (v1, v2, v3, v4, v5, v6) = (v(1)?, v(2)?, v(3)?, v(4)?, v(5)?, v(6)?);
    let (v7, v8, v9, v10, v11, v12) = (v(7)?, v(8)?, v(9)?, v(10)?, v(11)?, v(12)?);
    let mut out = [0.0; 16];
    for i in 0..4u32 {
        let a = |k: u32| float(mem, m.wrapping_add(0x78 + 4 * i + 0x10 * k));
        let (a0, a1, a2, a3) = (a(0)?, a(1)?, a(2)?, a(3)?);
        let row = [
            (a2 * v3 + v1 * a0 + a1 * v2) * s,
            (a2 * v6 + a0 * v4 + a1 * v5) * s,
            (a2 * v9 + v8 * a1 + a0 * v7) * s,
            a3 * s + (a2 * v12 + v11 * a1 + a0 * v10) * s,
        ];
        out[4 * i as usize..4 * i as usize + 4].copy_from_slice(&row);
    }
    Ok(out)
}

/// The 48 constant words `0x00012fa0` loads at `c[-96]` (**Code**
/// `0x00012fa0..0x000132c8`): the rows, the view's matrix words from
/// `+0xc4`, its point at `+0x18`, and the screen mapping from the window
/// at `+0x48` (top, left, bottom, right).
pub fn transform(mem: &dyn GuestMemory, view: u32) -> Result<[u32; 48], Cause> {
    let at = |off: u32| view.wrapping_add(off);
    let (top, left) = (short(mem, at(0x48))?, short(mem, at(0x4a))?);
    let (bottom, right) = (short(mem, at(0x4c))?, short(mem, at(0x4e))?);
    let (one, half) = (float(mem, ONE)?, float(mem, HALF)?);
    let (mut width, mut height) = (right.wrapping_sub(left) as f32, bottom.wrapping_sub(top) as f32);
    let (mut x0, mut y0) = (left as f32, top as f32);
    let mut z = view_scale(mem, view)?;
    let mut z_offset = float(mem, Z_OFFSET)?;
    if short(mem, at(2))? == 2 {
        width = float(mem, KIND2_EXTENT)?;
        height = width;
        (x0, y0, z, z_offset) = (0.0, 0.0, one, 0.0);
    }
    let r = rows(mem, view)?;
    let m = |off: u32| word(mem, at(0x8c + off));
    let point = |off: u32| word(mem, at(0x18 + off));
    let mut out = [0u32; 48];
    for (o, v) in out.iter_mut().zip(r) {
        *o = v.to_bits();
    }
    let view_words = [
        m(0x38)?, m(0x3c)?, m(0x40)?, one.to_bits(),
        m(0x44)?, m(0x48)?, m(0x4c)?, half.to_bits(),
        m(0x50)?, m(0x54)?, m(0x58)?, word(mem, TRANSFORM_27)?,
        point(0)?, point(4)?, point(8)?, word(mem, TRANSFORM_31)?,
    ];
    out[16..32].copy_from_slice(&view_words);
    let sx = width * half * z;
    let sy = height * float(mem, MINUS_HALF)? * z;
    let screen = [
        sx, 0.0, 0.0, ((width + one) * half + x0) * z + z_offset,
        0.0, sy, 0.0, ((height + one) * half + y0) * z + z_offset,
        sx, sy, float(mem, ROW_SCALE)?, 0.0,
        (width * half + x0) * z + half, (height * half + y0) * z + half, 0.0, 0.0,
    ];
    for (o, v) in out[32..].iter_mut().zip(screen) {
        *o = v.to_bits();
    }
    Ok(out)
}

/// The screen target `0x00025960` picks: `0x14` while the first screen
/// record is live and not off, else `0x12` for the second, else -1.
pub fn screen_target(mem: &dyn GuestMemory) -> Result<i32, Cause> {
    for ((live, off), target) in [(SCREEN_A, 0x14), (SCREEN_B, 0x12)] {
        if word(mem, live)? != 0 && byte(mem, off)? == 0 {
            return Ok(target);
        }
    }
    Ok(-1)
}

/// The cache's surface for `target` (`0x0001dcf0`): the record's `+0x18`
/// when it has a texture (`+0x88`), is not off (`+0x95`) and has a size
/// (`+0x78` above 0), else null.
pub fn cached(mem: &dyn GuestMemory, target: u32) -> Result<u32, Cause> {
    let r = TARGET_CACHE.wrapping_add(target.wrapping_mul(TARGET_RECORD));
    let live = word(mem, r.wrapping_add(0x88))? != 0 && byte(mem, r.wrapping_add(0x95))? == 0 && mem.i32(r.wrapping_add(0x78)).map_err(unread)? > 0;
    Ok(if live { r.wrapping_add(0x18) } else { 0 })
}

/// `_ftol` (`0x00322b0c`) of `n * scale` as the x87 makes it: the product
/// is exact in 53 bits; truncated toward zero, out of range the indefinite.
fn scaled(n: i32, scale: f32) -> u32 {
    let p = (f64::from(n) * f64::from(scale)).trunc();
    if p.is_finite() && p >= f64::from(i32::MIN) && p <= f64::from(i32::MAX) { p as i32 as u32 } else { 0x8000_0000 }
}

/// The viewport `0x00014bc0` sets for `target` (0 or `0x12`, **Code**
/// `0x00014d47..0x00014ebe`): 128 square for view kind 2; the 640 x 480
/// screen for the screen target ([`screen_target`]) with the view's `+7`;
/// else the context's window times the scale, which is
/// [`view_scale`]'s off the screen target and 1.0 on it.
pub fn viewport(mem: &dyn GuestMemory, view: u32, target: u32) -> Result<[u32; 6], Cause> {
    let one = float(mem, ONE)?;
    let at = |off: u32| view.wrapping_add(off);
    if short(mem, at(2))? == 2 {
        return Ok([0, 0, 0x80, 0x80, 0, one.to_bits()]);
    }
    let on_screen = screen_target(mem)? == target as i32;
    let wide = byte(mem, at(7))? != 0;
    if on_screen && wide {
        return Ok([0, 0, 0x280, 0x1e0, 0, one.to_bits()]);
    }
    let scale = if on_screen { one } else { view_scale(mem, view)? };
    let (top, left) = (short(mem, at(0x48))?, short(mem, at(0x4a))?);
    let (bottom, right) = (short(mem, at(0x4c))?, short(mem, at(0x4e))?);
    Ok([
        scaled(left, scale),
        scaled(top, scale),
        scaled(right.wrapping_sub(left), scale),
        scaled(bottom.wrapping_sub(top), scale),
        0,
        one.to_bits(),
    ])
}

/// `0x00131ed0`: the red, green and blue floats at `at` times
/// `[0x0045dc38]`, each rounded to nearest by `fistp` and masked to a byte,
/// as `0x00RRGGBB`. **S**: the FPU's precision is wide enough that the
/// product of two floats is exact (it is in 53 or 64 bits); an integer
/// `fistp` cannot hold gives its indefinite, whose low byte is 0.
pub fn pack(mem: &dyn GuestMemory, at: u32) -> Result<u32, Cause> {
    let k = f64::from(float(mem, COLOUR_SCALE)?);
    let channel = |off: u32| -> Result<u32, Cause> {
        let p = f64::from(float(mem, at.wrapping_add(off))?) * k;
        let r = p.round_ties_even();
        Ok(if r.is_finite() && r >= f64::from(i32::MIN) && r <= f64::from(i32::MAX) { (r as i32 as u32) & 0xff } else { 0 })
    };
    Ok(channel(0)? << 16 | channel(4)? << 8 | channel(8)?)
}

/// The fog's constants `0x00025a10` builds from the view's fog block (its
/// `+0x14c`, as the context copy leaves it at [`FOG_BLOCK`]) and the
/// camera at [`CAMERA`] (**Code** `0x00025a10..0x00025c9a`).
pub fn fog(mem: &dyn GuestMemory, view: u32) -> Result<Fog, Cause> {
    let b = |off: u32| float(mem, view.wrapping_add(0x14c + off));
    let bw = |off: u32| word(mem, view.wrapping_add(0x14c + off));
    let a = |off: u32| float(mem, CAMERA + 0xc + off);
    let c = |off: u32| float(mem, CAMERA + off);
    let (a0, a4, a8) = (a(0)?, a(4)?, a(8)?);
    let d = c(8)? * a8 + c(4)? * a4 + c(0)? * a0;
    let (k, one) = (float(mem, FOG_FLOOR)?, float(mem, ONE)?);
    let floor = |x: f32| if k > x { k } else { x };
    let inv1 = one / floor(b(0x30)? - b(0x2c)?);
    let inv2 = one / floor(b(0x48)? - b(0x44)?);
    let inv3 = one / floor(b(0xb0)?);
    let inv4 = one / floor(b(0xb4)?);
    let t = 0.0 - b(0x108)? * inv4;
    let t = if 0.0 > t {
        0.0
    } else if t > one {
        one
    } else {
        t
    };
    let f = |x: f32| x.to_bits();
    let block = [
        f(a0 * inv1), f(inv1 * a4), f(a8 * inv1), f(0.0 - (b(0x2c)? + d) * inv1),
        f(0.0 - b(0xf4)? * inv4), f(0.0 - b(0xf8)? * inv4), f(0.0 - b(0xfc)? * inv4), f(b(0x100)? * inv4),
        f(a0 * inv3), f(inv3 * a4), f(a8 * inv3), f(0.0 - (b(0x110)? + d) * inv3),
        f(t), 0, bw(0x28)?, bw(0xac)?,
        f(a0 * inv2), f(inv2 * a4), f(a8 * inv2), f(0.0 - (d + b(0x44)?) * inv2),
        bw(0x40)?, 0, 0, 0,
    ];
    Ok(Fog { block, colour: pack(mem, view.wrapping_add(0x14c + 0x1c))? })
}

/// The restore list `0x00018900` (Code `0x00018900..0x00018996`) of the
/// draw state's implementation `implementation`: `[+0x11c]`'s low 9 bits
/// index the 5-byte entries at `[[DRAW_STATE + 0x20] + 0x14]`, its high
/// bits count them.
pub fn restore_list(mem: &dyn GuestMemory, implementation: u32) -> Result<Vec<Restore>, Cause> {
    let head = u32::from(mem.u16(implementation.wrapping_add(0x11c)).map_err(unread)?);
    let base = word(mem, word(mem, DRAW_STATE + 0x20)?.wrapping_add(0x14))?.wrapping_add((head & 0x1ff) * 5);
    (0..head >> 9)
        .map(|i| {
            let e = base.wrapping_add(5 * i);
            let raw = mem.bytes(e, 5).map_err(unread)?;
            let (slot, value) = (raw[0], u32::from_le_bytes([raw[1], raw[2], raw[3], raw[4]]));
            let table = |t: u32| word(mem, t.wrapping_add(4 * u32::from(slot)));
            Ok(if slot < SIMPLE_SLOTS {
                Restore::Simple { slot, header: table(SIMPLE_HEADERS)?, value }
            } else if slot < DEFERRED_SLOTS {
                Restore::Deferred { slot, bits: table(DEFERRED_BITS)?, value }
            } else {
                Restore::Setter { slot, value }
            })
        })
        .collect()
}

/// The draw state's reset `0x00016b10` (ESI [`DRAW_STATE`]): an
/// implementation's restore list, then the reset's words.
fn draw_state(mem: &dyn GuestMemory, writes: &mut Vec<Write>) -> Result<Vec<Restore>, Cause> {
    let implementation = word(mem, DRAW_STATE + 0x24)?;
    let restore = if implementation != 0 { restore_list(mem, implementation)? } else { Vec::new() };
    let scalar = word(mem, RESET_SCALAR)?;
    let mut put = |at: u32, v: u32| writes.push(Write { at, bytes: v.to_le_bytes().to_vec() });
    for off in (0xc..=0x24).step_by(4) {
        put(DRAW_STATE + off, 0);
    }
    for off in [0, 0x40, 0x1320, 0x1360, 0x13a0, 0x13e0] {
        put(DRAW_STATE + off, u32::MAX);
    }
    for off in [0x1330, 0x1370, 0x13b0, 0x13f0] {
        put(DRAW_STATE + off, scalar);
    }
    Ok(restore)
}

/// The setup `0x000132d0(view)` as a plan (**Code**
/// `0x000132d0..0x0001344d`; the stencil helper `0x00015370` is called
/// with BX 0, its case 0 only). View kind 1 draws to target `0x12`, any
/// other to target 0. `Err` names a word that does not read.
pub fn plan(mem: &dyn GuestMemory, view: u32) -> Result<Setup, Cause> {
    let at = |off: u32| view.wrapping_add(off);
    let target = if short(mem, at(2))? == 1 { CACHED_TARGET } else { 0 };
    let mut writes = vec![Write { at: CONTEXT, bytes: mem.bytes(view, VIEW_SIZE as usize).map_err(unread)? }];
    let put = |writes: &mut Vec<Write>, at: u32, bytes: &[u8]| writes.push(Write { at, bytes: bytes.to_vec() });
    put(&mut writes, TARGET, &target.to_le_bytes());
    put(&mut writes, STENCIL_ENABLE, &0u32.to_le_bytes());
    put(&mut writes, STENCIL_MODE, &0u16.to_le_bytes());
    for (a, v) in SETUP_WORDS {
        put(&mut writes, a, &v.to_le_bytes());
    }
    let device = word(mem, DEVICE)?;
    let near = float(mem, ONE)? * float(mem, CLIP_NEAR)?;
    let clip = [near.to_bits(), word(mem, CLIP_FAR)?, word(mem, device.wrapping_add(DEVICE_CLIP + 8))? | 2];
    let transform = transform(mem, view)?;
    let shadow = byte(mem, CONSTANT_FLAGS)? & 0x10 == 0;
    if shadow {
        put(&mut writes, TRANSFORM_SHADOW, &words_le(&transform));
    }
    // `0x00014bc0`'s cases 0 and `0x12` (its table at `0x00014ed0`).
    let depth = word(mem, VIEW_DEPTH)?;
    let (colour, flag) = if target == 0 { (word(mem, VIEW_COLOUR)?, 1) } else { (cached(mem, target)?, 0) };
    let viewport = viewport(mem, view, target)?;
    let clear = if short(mem, at(8))? != 0 {
        Some(if byte(mem, CLEAR_PLAIN)? != 0 { (1, 0) } else { (0x61, pack(mem, at(0xc))?) })
    } else {
        None
    };
    let push = [0x0010_1af0, 0, word(mem, HALF)?, word(mem, ONE)?, word(mem, PUSH_W)?];
    put(&mut writes, FILL_MODE, &SOLID.to_le_bytes());
    put(&mut writes, DRAW_BEGIN, &[1]);
    let restore = draw_state(mem, &mut writes)?;
    let (fog, fog_effect) = if byte(mem, at(0x26c))? != 0 {
        put(&mut writes, FOG_BLOCK, &mem.bytes(at(0x14c), 0x48 * 4).map_err(unread)?);
        let effect = word(mem, at(0x1b8))?;
        let fog = fog(mem, view)?;
        if shadow {
            put(&mut writes, FOG_SHADOW, &words_le(&fog.block));
        }
        put(&mut writes, FOG_COLOUR, &fog.colour.to_le_bytes());
        (Some(fog), effect != u32::MAX && effect != 0)
    } else {
        (None, false)
    };
    Ok(Setup { view, writes, clip, transform, colour, depth, flag, viewport, clear, push, restore, fog, fog_effect })
}

fn stage(mem: &mut dyn GuestMemory, at: u32, words: &[u32]) -> Result<(), Cause> {
    mem.write(at, &words_le(words)).map_err(unwritable)
}

/// The push buffer's put, made room for as the body's inline writers do:
/// at or past the limit `+4`, `MakeRequestedSpace([0x00408648] / 2,
/// [0x00408648])`.
fn reserve(mem: &mut dyn GuestMemory, host: &dyn Host, device: u32) -> Result<u32, Cause> {
    let put = word(mem, device)?;
    if put < word(mem, device.wrapping_add(4))? {
        return Ok(put);
    }
    let t = word(mem, PUSH_THRESHOLD)?;
    host.make_space(mem, t >> 1, t).ok_or("device")
}

/// Makes `s`: the game-side words, then the device's calls in the body's
/// order, staged in the guest stack at `scratch`. The fog's effect is the
/// caller's ([`Setup::fog_effect`]). `Err` names the call it could not
/// make.
pub fn apply(mem: &mut dyn GuestMemory, host: &dyn Host, s: &Setup, scratch: u32) -> Result<(), Cause> {
    for w in &s.writes {
        mem.write(w.at, &w.bytes).map_err(unwritable)?;
    }
    let refused = |made: bool| if made { Ok(()) } else { Err("device") };
    refused(host.render_state(mem, SLOT_STENCIL_ENABLE, 0))?;
    let device = word(mem, DEVICE)?;
    stage(mem, device.wrapping_add(DEVICE_CLIP), &s.clip)?;
    let put = reserve(mem, host, device)?;
    let put = host.push_viewport(mem, put, device).ok_or("device")?;
    mem.set_u32(device, put).map_err(unwritable)?;
    stage(mem, scratch, &s.transform)?;
    refused(host.constants(mem, 0, scratch, 48))?;
    native_targets::cut_select(mem, host, s.colour, s.depth, s.flag)?;
    stage(mem, scratch, &s.viewport)?;
    refused(host.viewport(mem, scratch))?;
    if let Some((flags, colour)) = s.clear {
        native_targets::cut_clear(mem, host, flags, colour, 1f32.to_bits(), 0, scratch)?;
    }
    let put = reserve(mem, host, device)?;
    stage(mem, put, &s.push)?;
    mem.set_u32(device, put.wrapping_add(0x14)).map_err(unwritable)?;
    refused(host.render_state(mem, SLOT_FILL_MODE, SOLID))?;
    for r in &s.restore {
        restore(mem, host, *r)?;
    }
    if let Some(fog) = &s.fog {
        stage(mem, scratch, &fog.block)?;
        refused(host.constants(mem, 0xc, scratch, 0x18))?;
        refused(host.render_state(mem, SLOT_FOG_COLOUR, fog.colour))?;
    }
    Ok(())
}

/// One restore entry as `0x00018900` makes it.
pub fn restore(mem: &mut dyn GuestMemory, host: &dyn Host, r: Restore) -> Result<(), Cause> {
    let slot_word = |slot: u8| RENDER_STATES.wrapping_add(4 * u32::from(slot));
    match r {
        Restore::Simple { slot, header, value } => {
            if !host.simple_state(mem, header, value) {
                return Err("device");
            }
            mem.set_u32(slot_word(slot), value).map_err(unwritable)
        }
        Restore::Deferred { slot, bits, value } => {
            let dirty = word(mem, DEFERRED_DIRTY)? | bits;
            mem.set_u32(DEFERRED_DIRTY, dirty).map_err(unwritable)?;
            mem.set_u32(slot_word(slot), value).map_err(unwritable)
        }
        Restore::Setter { slot, value } => {
            if host.render_state(mem, u32::from(slot), value) { Ok(()) } else { Err("device") }
        }
    }
}

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    VIEW_SETUP.counters().into_iter().chain(VIEW_END.counters()).collect()
}

/// `0x000132d0`'s entry: the plan made, then the fog's effect as the
/// body's last call when it has one; it returns to the caller with `ret 4`.
fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    VIEW_SETUP.call();
    let host = native_targets::installed("view_setup");
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let made = word(&*mem, cpu.esp.wrapping_add(4)).and_then(|view| plan(&*mem, view)).and_then(|s| {
        apply(&mut *mem, &*host, &s, cpu.esp.wrapping_sub(SCRATCH))?;
        Ok(s.fog_effect)
    });
    let effect = made.unwrap_or_else(|cause| unhandled("view_setup", cause));
    if !effect {
        drop(mem);
        return ctx.memory().ret(cpu, 4);
    }
    let (esp, ret) = (cpu.esp, mem.u32(cpu.esp)?);
    // The body's frame: its three saved registers and EBX below the
    // return address.
    cpu.esp = esp.wrapping_sub(0x10);
    ctx.call_guest_named(cpu, &mut mem, FOG_EFFECT, "render: the view setup's fog effect", move |cpu, _ctx| {
        cpu.esp = esp.wrapping_add(8);
        Ok(runtime::TransferAbi::ret(ret))
    })
}

/// What the end does before the setup of the view left: the alpha clear
/// when `[0x004ba04c]` is above 1, and the pop. `Ok(None)`: the stack is
/// empty, the end returns.
fn end_begin(mem: &mut dyn GuestMemory, host: &dyn Host) -> Result<Option<u32>, Cause> {
    if mem.i32(ALPHA_GATE).map_err(unread)? > 1 {
        if !host.clear(mem, 0x80, 0, 1f32.to_bits(), 0) {
            return Err("device");
        }
    }
    let depth = mem.i32(VIEW_STACK).map_err(unread)?.wrapping_sub(1);
    mem.set_i32(VIEW_STACK, depth).map_err(unwritable)?;
    Ok((depth >= 0).then(|| VIEW_RECORDS.wrapping_add(depth as u32 * VIEW_SIZE)))
}

/// The end's copies after the setup (`0x00013c65..0x00013c9f`): the view's
/// kind, its fog block and its camera.
fn end_copies(mem: &mut dyn GuestMemory) -> Result<(), Cause> {
    let view = VIEW_RECORDS.wrapping_add((mem.i32(VIEW_STACK).map_err(unread)? as u32).wrapping_mul(VIEW_SIZE));
    let kind = short(mem, view)?;
    mem.set_i32(CAMERA_KIND, kind).map_err(unwritable)?;
    let fog = mem.bytes(view.wrapping_add(0x14c), 0x48 * 4).map_err(unread)?;
    mem.write(CAMERA_FOG, &fog).map_err(unwritable)?;
    let camera = mem.bytes(view.wrapping_add(0x18), 0x1d * 4).map_err(unread)?;
    mem.write(CAMERA, &camera).map_err(unwritable)
}

/// `0x00013c20`'s entry: the clear and the pop in Rust, then the body's
/// calls as guest calls: the setup `0x000132d0(view)` (this module's), the
/// camera `0x0002f970` (EAX [`CAMERA`], ESI a 16-byte local) and
/// `0x0002fd90` (EDX the local, EBX [`CAMERA_POINTS`], ESI [`CAMERA`]),
/// neither of which calls D3D. It returns to the end's caller with EBX,
/// ESI, EDI and EBP as they came in.
fn end_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, _addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    VIEW_END.call();
    let host = native_targets::installed("view_end");
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let view = end_begin(&mut *mem, &*host).unwrap_or_else(|cause| unhandled("view_end", cause));
    let (esp, ret) = (cpu.esp, mem.u32(cpu.esp)?);
    let Some(view) = view else {
        drop(mem);
        return ctx.memory().ret(cpu, 0);
    };
    let saved = (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp);
    // The body's frame: its 16-byte local below the return address, then
    // its three saved registers; the view pushed for the setup's `ret 4`.
    let local = esp.wrapping_sub(0x10);
    cpu.esp = esp.wrapping_sub(0x1c).wrapping_sub(4);
    mem.set_u32(cpu.esp, view)?;
    ctx.call_guest_named(cpu, &mut mem, engine::points::RENDER_VIEW_SETUP.base, "render: the view end's setup", move |cpu, ctx| {
        let rt = Arc::clone(ctx.runtime());
        let mut mem = rt.memory();
        end_copies(&mut *mem).unwrap_or_else(|cause| unhandled("view_end", cause));
        cpu.eax = CAMERA;
        cpu.esi = local;
        ctx.call_guest_named(cpu, &mut mem, CAMERA_UPDATE, "render: the view end's camera", move |cpu, ctx| {
            let rt = Arc::clone(ctx.runtime());
            let mut mem = rt.memory();
            cpu.edx = local;
            cpu.ebx = CAMERA_POINTS;
            cpu.esi = CAMERA;
            ctx.call_guest_named(cpu, &mut mem, CAMERA_FINISH, "render: the view end's camera", move |cpu, _ctx| {
                (cpu.ebx, cpu.esi, cpu.edi, cpu.ebp) = saved;
                cpu.esp = esp.wrapping_add(4);
                Ok(runtime::TransferAbi::ret(ret))
            })
        })
    })
}

/// Places the setup's and the end's implementations (`native_targets`'
/// feature).
pub fn place(b: &mut hooks::Builder<'_>) {
    b.raw().implement(&engine::points::RENDER_VIEW_SETUP, "native_view_setup", setup_call).context_only();
    b.raw().implement(&engine::points::RENDER_VIEW_END, "native_view_end", end_call).context_only();
}

#[cfg(test)]
mod tests;
