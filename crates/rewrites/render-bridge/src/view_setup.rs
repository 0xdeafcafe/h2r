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
//! The arm: one call in [`native_targets::CHECK_EVERY`] runs the guest body
//! and compares what it left with the plan ([`check`]): every planned
//! word, the transform and fog constants, the three render states and the
//! bound target. A difference sends every later call to the guest body,
//! counted `scene_kept_view_setup_mismatch`. A setup the plan does not
//! cover runs the body, counted `scene_kept_view_setup_<cause>`:
//!
//! | Cause | When |
//! |---|---|
//! | `host` | no host installed |
//! | `target` | view kind 1: target `0x12`, the cache's (`0x0001dcf0`) |
//! | `scale` | the viewport's scale (`[0x004670c8]` clamped) is not 1.0 |
//! | `prelude` | a clear whose prelude `0x00014ac0` would run |
//! | `restore` | the draw state's restore list `0x00018900` is not empty |
//! | `fog_effect` | the fog's effect `0x00027aa0` would run (`+0x1b8` not 0 or -1) |
//! | `unread`, `unwritable` | a word that does not read or write |
//! | `device` | the device refused a call (the body repeats the prefix made) |
//!
//! The end has no arm of its own: its only D3D call is the alpha clear,
//! and the setup of the view left is the guest call `0x000132d0`, which is
//! this setup. Counted `view_end_calls`, `view_end_cut` and
//! `scene_kept_view_end_<cause>` (`host`, `device`, `unread`,
//! `unwritable`).

use std::sync::Arc;

use engine::memory::{GuestMemory, MemFault};

use crate::native_targets::{self, Arm, Host};

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

/// The target-0 surfaces the target switch `0x00014bc0` binds.
pub const VIEW_COLOUR: u32 = 0x0050_935c;
pub const VIEW_DEPTH: u32 = 0x0050_9364;
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
    /// `x, y, width, height, min_z, max_z`.
    pub viewport: [u32; 6],
    /// The clear's game flags and colour.
    pub clear: Option<(u32, u32)>,
    /// The raw push: method `0x1af0`, four words (0, `[0x0045dbbc]`,
    /// `[0x0045dbc0]`, `[0x0045e3b0]`).
    pub push: [u32; 5],
    pub fog: Option<Fog>,
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

/// The viewport `0x00014bc0` sets for target 0 (**Code**
/// `0x00014d47..0x00014ebe`): 128 square for view kind 2, else the
/// context's window. A scale other than 1.0 is refused (`scale`): its
/// `fild`, `fmul` and `_ftol` are not made here.
pub fn viewport(mem: &dyn GuestMemory, view: u32) -> Result<[u32; 6], Cause> {
    let one = float(mem, ONE)?.to_bits();
    let at = |off: u32| view.wrapping_add(off);
    if short(mem, at(2))? == 2 {
        return Ok([0, 0, 0x80, 0x80, 0, one]);
    }
    if view_scale(mem, view)?.to_bits() != one {
        return Err("scale");
    }
    let (top, left) = (short(mem, at(0x48))?, short(mem, at(0x4a))?);
    let (bottom, right) = (short(mem, at(0x4c))?, short(mem, at(0x4e))?);
    Ok([left as u32, top as u32, right.wrapping_sub(left) as u32, bottom.wrapping_sub(top) as u32, 0, one])
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

/// The draw state's reset `0x00016b10` (ESI [`DRAW_STATE`]): an
/// implementation's restore `0x00018900` is taken only with an empty list
/// (`restore` otherwise), whose words the guest still reads.
fn draw_state(mem: &dyn GuestMemory, writes: &mut Vec<Write>) -> Result<(), Cause> {
    let implementation = word(mem, DRAW_STATE + 0x24)?;
    if implementation != 0 {
        if mem.u16(implementation.wrapping_add(0x11c)).map_err(unread)? >> 9 != 0 {
            return Err("restore");
        }
        word(mem, word(mem, DRAW_STATE + 0x20)?.wrapping_add(0x14))?;
    }
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
    Ok(())
}

/// The setup `0x000132d0(view)` as a plan (**Code**
/// `0x000132d0..0x0001344d`; the stencil helper `0x00015370` is called
/// with BX 0, its case 0 only). `Err` names why the body runs.
pub fn plan(mem: &dyn GuestMemory, view: u32) -> Result<Setup, Cause> {
    let at = |off: u32| view.wrapping_add(off);
    let kind = short(mem, at(2))?;
    if kind == 1 {
        return Err("target");
    }
    let mut writes = vec![Write { at: CONTEXT, bytes: mem.bytes(view, VIEW_SIZE as usize).map_err(unread)? }];
    let put = |writes: &mut Vec<Write>, at: u32, bytes: &[u8]| writes.push(Write { at, bytes: bytes.to_vec() });
    put(&mut writes, TARGET, &0u32.to_le_bytes());
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
    let (colour, depth) = (word(mem, VIEW_COLOUR)?, word(mem, VIEW_DEPTH)?);
    let viewport = viewport(mem, view)?;
    let clear = if short(mem, at(8))? != 0 {
        if byte(mem, at(7))? != 0 {
            // `0x00014b60`'s prelude `0x00014ac0` (the context's `+7`, target 0).
            return Err("prelude");
        }
        Some(if byte(mem, CLEAR_PLAIN)? != 0 { (1, 0) } else { (0x61, pack(mem, at(0xc))?) })
    } else {
        None
    };
    let push = [0x0010_1af0, 0, word(mem, HALF)?, word(mem, ONE)?, word(mem, PUSH_W)?];
    put(&mut writes, FILL_MODE, &SOLID.to_le_bytes());
    put(&mut writes, DRAW_BEGIN, &[1]);
    draw_state(mem, &mut writes)?;
    let fog = if byte(mem, at(0x26c))? != 0 {
        put(&mut writes, FOG_BLOCK, &mem.bytes(at(0x14c), 0x48 * 4).map_err(unread)?);
        let effect = word(mem, at(0x1b8))?;
        if effect != u32::MAX && effect != 0 {
            return Err("fog_effect");
        }
        let fog = fog(mem, view)?;
        if shadow {
            put(&mut writes, FOG_SHADOW, &words_le(&fog.block));
        }
        put(&mut writes, FOG_COLOUR, &fog.colour.to_le_bytes());
        Some(fog)
    } else {
        None
    };
    Ok(Setup { view, writes, clip, transform, colour, depth, viewport, clear, push, fog })
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
/// order, staged in the guest stack at `scratch`. `Err` names why the
/// guest body runs from where the cut stopped; each of its steps sets the
/// same state again.
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
    native_targets::cut_select(mem, host, s.colour, s.depth, 1)?;
    stage(mem, scratch, &s.viewport)?;
    refused(host.viewport(mem, scratch))?;
    if let Some((flags, colour)) = s.clear {
        native_targets::cut_clear(mem, host, flags, colour, 1f32.to_bits(), 0)?;
    }
    let put = reserve(mem, host, device)?;
    stage(mem, put, &s.push)?;
    mem.set_u32(device, put.wrapping_add(0x14)).map_err(unwritable)?;
    refused(host.render_state(mem, SLOT_FILL_MODE, SOLID))?;
    if let Some(fog) = &s.fog {
        stage(mem, scratch, &fog.block)?;
        refused(host.constants(mem, 0xc, scratch, 0x18))?;
        refused(host.render_state(mem, SLOT_FOG_COLOUR, fog.colour))?;
    }
    Ok(())
}

/// A check: what the guest body left against `s`. `bound` is the bound
/// target the cut would leave (the pair from the select, the planned
/// viewport); the device clips the viewport's far edges to the target, so
/// its width and height are compared as at most the planned ones.
pub fn check(mem: &dyn GuestMemory, host: &dyn Host, s: &Setup, bound: [u32; 8]) -> Result<(), String> {
    for w in &s.writes {
        if mem.bytes(w.at, w.bytes.len()).ok().as_deref() != Some(&w.bytes[..]) {
            return Err(format!("the words at {:#010x}", w.at));
        }
    }
    let device = mem.u32(DEVICE).map_err(|_| "the device".to_string())?;
    for (i, v) in s.clip.iter().enumerate() {
        if mem.u32(device.wrapping_add(DEVICE_CLIP + 4 * i as u32)).ok() != Some(*v) {
            return Err(format!("the clip word {i}"));
        }
    }
    let constants = |base: usize, words: &[u32]| -> Result<(), String> {
        for (r, row) in words.chunks(4).enumerate() {
            if host.constant(base + r).as_ref().map(|c| &c[..]) != Some(row) {
                return Err(format!("c[{}]", base + r));
            }
        }
        Ok(())
    };
    constants(0, &s.transform)?;
    let mut states = vec![(SLOT_STENCIL_ENABLE, 0), (SLOT_FILL_MODE, SOLID)];
    if let Some(fog) = &s.fog {
        constants(12, &fog.block)?;
        states.push((SLOT_FOG_COLOUR, fog.colour));
    }
    for (slot, v) in states {
        if host.stored_state(mem, slot) != Some(v) {
            return Err(format!("render state {slot}"));
        }
    }
    let Some(guest) = host.bound() else { return Err("no bound target".into()) };
    let same = [0, 1, 2, 3, 6, 7].iter().all(|&i| guest[i] == bound[i]) && guest[4] <= bound[4] && guest[5] <= bound[5];
    if !same {
        return Err(format!("the bound target (cut {bound:08x?}, guest {guest:08x?})"));
    }
    Ok(())
}

/// The bound target the cut leaves from `before`.
pub fn expected(before: [u32; 8], bind: native_targets::Bind, s: &Setup) -> [u32; 8] {
    let mut out = native_targets::expected(before, bind, s.colour, s.depth);
    out[2..].copy_from_slice(&s.viewport);
    out
}

/// The counters `render.scene` reports.
pub fn counters() -> Vec<(String, u64)> {
    VIEW_SETUP.counters().into_iter().chain(VIEW_END.counters()).collect()
}

fn original(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget { pc: cpu.eip, target: addr, kind: runtime::abi::JUMP, route: runtime::abi::ENTRY });
    Ok(runtime::TransferAbi::fault(event))
}

fn setup_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let arm = &VIEW_SETUP;
    let check_call = arm.next_is_check();
    let Some(host) = native_targets::host() else {
        arm.keep("host");
        return original(cpu, ctx, addr);
    };
    if arm.fallen_back() {
        arm.keep("fallback");
        return original(cpu, ctx, addr);
    }
    let rt = Arc::clone(ctx.runtime());
    let planned = {
        let mem = rt.memory();
        let m: &dyn GuestMemory = &*mem;
        m.u32(cpu.esp.wrapping_add(4)).map_err(unread).and_then(|view| plan(m, view))
    };
    let s = match planned {
        Ok(s) => s,
        Err(cause) => {
            arm.keep(cause);
            return original(cpu, ctx, addr);
        }
    };
    if check_call {
        let before = {
            let mem = rt.memory();
            native_targets::select(&*mem, s.colour, s.depth).ok().zip(host.bound())
        };
        let out = original(cpu, ctx, addr)?;
        if let Some((bind, before)) = before {
            let mem = rt.memory();
            arm.prove(check(&*mem, &*host, &s, expected(before, bind, &s)));
        }
        return Ok(out);
    }
    let made = {
        let mut mem = rt.memory();
        apply(&mut *mem, &*host, &s, cpu.esp.wrapping_sub(SCRATCH))
    };
    match made {
        Ok(()) => {
            arm.made();
            ctx.memory().ret(cpu, 4)
        }
        Err(cause) => {
            arm.keep(cause);
            original(cpu, ctx, addr)
        }
    }
}

/// A copy the end cannot make after its setup ran: the guest's own fault
/// at the view records.
fn fault(_: Cause) -> runtime::GuestFault {
    runtime::GuestFault {
        addr: VIEW_RECORDS,
        len: VIEW_SIZE as usize,
        access: runtime::guest::Access::Read,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

/// What the end does before the setup of the view left: the alpha clear
/// when `[0x004ba04c]` is above 1, and the pop. `Ok(None)`: the stack is
/// empty, the end returns.
fn end_begin(mem: &mut dyn GuestMemory, host: Option<&dyn Host>) -> Result<Option<u32>, Cause> {
    if mem.i32(ALPHA_GATE).map_err(unread)? > 1 {
        let host = host.ok_or("host")?;
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
fn end_call(cpu: &mut runtime::CpuAbi, ctx: &mut runtime::ExecCtx, addr: u32) -> Result<runtime::TransferAbi, runtime::GuestFault> {
    let arm = &VIEW_END;
    arm.call();
    let host = native_targets::host();
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let view = match end_begin(&mut *mem, host.as_deref()) {
        Ok(v) => v,
        Err(cause) => {
            drop(mem);
            arm.keep(cause);
            return original(cpu, ctx, addr);
        }
    };
    arm.made();
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
        end_copies(&mut *mem).map_err(fault)?;
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
