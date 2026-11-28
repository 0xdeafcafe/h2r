//! The second fog's screen fans (`0x00025dc0`,
//! docs/graphics/retire-d3d-fog-fans.md): which fans it draws, as a pure
//! function of guest memory and its two flag bytes.
//!
//! (Code, `h2 disasm 0x25dc0 40`, `0x26880 240`, `0x26e50 410`,
//! `0x276d0 160`, `0x34420 215`, `0x3a5c0 1200`.)
//!
//! `0x00025dc0(cl, dl)` draws only when the fog's tag datum
//! `[0x004857b8]` is neither -1 nor 0 and `[0x004857d8]` is above
//! `[0x0045dbd8]` (`comiss; jbe`). Then it stores `cl` and `dl` at
//! `0x005093fd` and `0x005093fe` and draws through the fan `0x00034420`:
//!
//! | Flags | Fans (setup, attribute callback) |
//! |---|---|
//! | `cl` set | [`Fans::Layers`]: `0x00026880`, `0x00026ca0`, into target `0x13` ([`layers`]); then `0x00026e50`, `0x00027520` (the branch byte clear, the weather plate [`weather_plate_placement`] and [`plate_fan`] when it returns true; else [`layers_composite`]) |
//! | `cl` clear, `dl` set | [`Fans::Composite`]: `0x000276d0`, `0x00027960`, target 3 into the view's ([`composite`]) |
//! | neither | none |
//!
//! Every path ends with the depth clip control `0x00041d78` set to 0 and
//! the words `0x004b8344` and `0x004075e0` zeroed.
//!
//! The composite's fan ([`composite`], `h2 disasm 0x276d0 160`,
//! `0x27960 35`): the setup writes the clip far `[0x00485ae0]` to the
//! depth word `0x00509400`, selects the view's target `[0x004858b8]`
//! (`0x00014bc0`, AL 0), binds target 3 at stage 0 (`0x00014f60`; clamp,
//! point, no mip), sets blend on, `GL_CONSTANT_COLOR` over
//! `GL_ONE_MINUS_SRC_ALPHA`, add, constant `0x00ffffff`, every channel
//! written, alpha test, cull, stencil and depth test off, then the block
//! [`composite_combiner`] and the pass-through vertex program, AL 1. The
//! callback gives attribute 0 the rectangle's lerp at the depth word
//! (`0x000350e0`) and attribute 1 the lerp itself, z 0, w 1.

use crate::light_constants::{Error, Sse, X87};
use crate::memory::{GuestMemory, MemFault};
use crate::screen_grid;

/// `0x00025dc0`.
pub const FOG_FANS: u32 = 0x0002_5dc0;
/// The fan every draw goes through.
pub const FAN: u32 = 0x0003_4420;

/// The fans' setups (handed to the fan in EAX) and attribute callbacks.
pub mod setup {
    /// The fog's four layers into target `0x13`.
    pub const LAYERS: u32 = 0x0002_6880;
    pub const LAYERS_CORNERS: u32 = 0x0002_6ca0;
    /// Target `0x13` over the view, after [`LAYERS`].
    pub const LAYERS_COMPOSITE: u32 = 0x0002_6e50;
    pub const LAYERS_COMPOSITE_CORNERS: u32 = 0x0002_7520;
    /// Target 3 over the view (`dl` alone).
    pub const COMPOSITE: u32 = 0x0002_76d0;
    pub const COMPOSITE_CORNERS: u32 = 0x0002_7960;
}

/// Guest globals and image constants `0x00025dc0` reads.
pub mod global {
    /// The fog's tag datum: -1 or 0, no fans.
    pub const DATUM: u32 = 0x0048_57b8;
    /// The fog's amount (`f32`), drawn only above [`ZERO`].
    pub const AMOUNT: u32 = 0x0048_57d8;
    /// 0.0.
    pub const ZERO: u32 = 0x0045_dbd8;
    /// Where the flags are kept for the setups (`cl`, `dl`).
    pub const FLAG_CL: u32 = 0x0050_93fd;
    pub const FLAG_DL: u32 = 0x0050_93fe;
}

/// The fan's frame as a setup sees it (`0x00034420` calls the setup
/// through EDI at `0x0003452c`): the setup's return, then `SETUP_FRAME`
/// bytes up the fan's return, the rectangle's address (the fan's first
/// argument, which it points at its own rectangle) and the callback.
pub const SETUP_RETURN: u32 = 0x0003_452e;
pub const SETUP_FRAME: u32 = 0x64;
/// `0x00025dc0`'s returns from its two fan calls.
pub const LAYERS_RETURN: u32 = 0x0002_5e04;
pub const SECOND_RETURN: u32 = 0x0002_5e2d;

/// The composite's globals and image constants.
pub mod composite_global {
    /// The view's target (`i16`).
    pub const VIEW_TARGET: u32 = 0x0048_58b8;
    /// The clip far the setup writes to [`DEPTH`].
    pub const CLIP_FAR: u32 = 0x0048_5ae0;
    /// The depth word the callback reads.
    pub const DEPTH: u32 = 0x0050_9400;
    /// The fog's colour (three `f32`), packed by `0x00131ed0`.
    pub const COLOUR: u32 = 0x0048_57bc;
    /// `0x00131ed0`'s scale (255.0).
    pub const SCALE: u32 = 0x0045_dc38;
}

/// The target the composite samples at stage 0.
pub const COMPOSITE_SOURCE: u8 = 3;

/// Words of a combiner block.
pub const WORDS: usize = 60;

/// The composite's block (`0x000278ce`..`0x00027917`): stage 0's
/// texture times the colour, the colour the final combiner's constant 0.
pub fn composite_combiner(colour: u32) -> [u32; WORDS] {
    let mut w = [0; WORDS];
    w[0] = 0x0820_0000; // alpha inputs, stage 0
    w[8] = 0x1c01_0000; // final combiner ABCD
    w[9] = 0x1c00; // final combiner EFG
    w[26] = 0xc0; // alpha outputs, stage 0
    w[43] = colour; // final combiner constant 0
    w[53] = 0x0001_1001; // combiner count
    w[54] = 1; // texture modes: stage 0 2D
    w
}

/// The composite's fan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Composite {
    /// The view's target the fan draws into (`[0x004858b8]`).
    pub target: i16,
    /// `0x00131ed0` of the fog's colour.
    pub colour: u32,
    pub combiner: [u32; WORDS],
    /// The fan's rectangle: left, right, top, bottom (f32 bits).
    pub rect: [u32; 4],
    /// Each vertex's attribute 0 and attribute 1 (x, y, z, w, f32 bits),
    /// in push order.
    pub positions: [[u32; 4]; 4],
    pub coordinates: [[u32; 4]; 4],
}

/// The fan's corners in the order it pushes them.
pub const CORNER_UV: [[bool; 2]; 4] = [[false, false], [false, true], [true, true], [true, false]];

/// The rectangle's address when the setup whose stack is at `esp` is the
/// fan's of `0x00025dc0` returning to `fan_return` with `callback`.
pub fn fan_call(mem: &dyn GuestMemory, esp: u32, fan_return: u32, callback: u32) -> Option<u32> {
    let fan = esp.wrapping_add(SETUP_FRAME);
    let word = |a: u32| mem.u32(a).ok();
    (word(esp)? == SETUP_RETURN && word(fan)? == fan_return && word(fan.wrapping_add(8))? == callback)
        .then(|| word(fan.wrapping_add(4)))
        .flatten()
}

/// What the composite's setup writes that a later read sees: the clip far
/// to the depth word, which the callback reads.
pub fn composite_writes(mem: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let far = mem.u32(composite_global::CLIP_FAR)?;
    mem.set_u32(composite_global::DEPTH, far)
}

/// The composite's fan whose rectangle is at `rect_at`, under the call's
/// MXCSR and x87 control word. The setup has just written the clip far to
/// the depth word, so the callback's depth is the clip far.
pub fn composite(mem: &dyn GuestMemory, mxcsr: u32, x87_control: u16, rect_at: u32) -> Result<Composite, Error> {
    use composite_global as g;
    let target = mem.u16(g::VIEW_TARGET)? as i16;
    let lane = |i: u32| mem.u32(g::COLOUR + 4 * i);
    let colour = crate::glow::pack_rgb([lane(0)?, lane(1)?, lane(2)?], mem.u32(g::SCALE)?, x87_control)?;
    let depth = mem.u32(g::CLIP_FAR)?;
    let mut rect = [0; 4];
    for (i, r) in rect.iter_mut().enumerate() {
        *r = mem.u32(rect_at.wrapping_add(4 * i as u32))?;
    }
    let one = mem.u32(screen_grid::constant::ONE)?;
    let mut sse = Sse { mxcsr };
    let (mut positions, mut coordinates) = ([[0; 4]; 4], [[0; 4]; 4]);
    for (n, [u, v]) in CORNER_UV.into_iter().enumerate() {
        let uv = [if u { one } else { 0 }, if v { one } else { 0 }];
        positions[n] = screen_grid::position(mem, &mut sse, &rect, uv, depth)?;
        let [x, y] = lerp(&mut sse, &rect, uv)?;
        coordinates[n] = [x, y, 0, one];
    }
    Ok(Composite { target, colour, combiner: composite_combiner(colour), rect, positions, coordinates })
}

/// The layers' globals and image constants (`h2 disasm 0x26880 240`,
/// `0x26ca0 50`, `0x26e50 410`, `0x27520 40`).
pub mod layers_global {
    /// The tag table: entry `datum & 0xffff` (16 bytes) holds the tag's
    /// data address at `+8`; the fog patch's bitmap datum is at `+0x18`.
    pub const TAG_TABLE: u32 = 0x004e_3b44;
    pub const BITMAP_DATUM: u32 = 0x18;
    /// Layer `n` at `LAYERS + 0x38 n`: its image word at `+0`, amount
    /// `+0x14`, matrix `+0x18`, `+0x1c`, `+0x28`, `+0x2c`, offsets
    /// `+0x24`, `+0x34`.
    pub const LAYERS: u32 = 0x004b_9cac;
    pub const LAYER_STRIDE: u32 = 0x38;
    /// The layers' colour (three `f32`) and the three layer colours.
    pub const COLOUR: u32 = 0x004b_9cb4;
    pub const LAYER_COLOURS: [u32; 3] = [0x004b_9cec, 0x004b_9d24, 0x004b_9d5c];
    /// The branch byte: set, the full combiner; clear, the weather plate
    /// first (`0x0003a5c0`).
    pub const BRANCH: u32 = 0x004b_9d9c;
    /// The coordinates' aspect and the composite's stage-3 scale.
    pub const ASPECT: u32 = 0x004b_9d94;
    pub const TARGET_SCALE: u32 = 0x004b_9d90;
    /// 2.0, the coordinates' scale.
    pub const TWO: u32 = 0x0045_dc0c;
    /// The composite's dot vector: `[0x0048565c]` over the depth span
    /// `[0x004857e0] - [0x004857dc]`, held to at least `[0x0045dbdc]`.
    pub const DOT_X: u32 = 0x0048_565c;
    pub const SPAN_NEAR: u32 = 0x0048_57dc;
    pub const SPAN_FAR: u32 = 0x0048_57e0;
    pub const SPAN_MIN: u32 = 0x0045_dbdc;
    /// The composite's fog amount besides [`super::global::AMOUNT`].
    pub const DENSITY: u32 = 0x0048_57d4;
}

/// The target the layers draw into and their composite samples at stage 3.
pub const LAYERS_TARGET: u8 = 0x13;

/// The layers' fan into target `0x13` (`0x00026880`, `0x00026ca0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayersFan {
    /// The fog patch's bitmap datum, and the image each stage binds.
    pub bitmap: u32,
    pub images: [u16; 4],
    pub combiner: [u32; WORDS],
    /// Each vertex's position and stage coordinates (f32 bits), in push
    /// order.
    pub positions: [[u32; 4]; 4],
    pub coordinates: [[[u32; 4]; 4]; 4],
}

/// Where the layers' composite draws (`0x00026f81`): the view's target
/// when `dl` is set, else target 3, which it also samples at stage 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayersDest {
    View(i16),
    Target3,
}

/// The layers' composite fan, the set branch (`0x00026f81`..`0x0002751b`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayersComposite {
    pub dest: LayersDest,
    pub combiner: [u32; WORDS],
    pub positions: [[u32; 4]; 4],
    pub coordinates: [[[u32; 4]; 4]; 4],
}

impl LayersComposite {
    /// Into target 3 the setup writes blue alone with blending off
    /// (`0x000274a0`: states 0x43 = 1, 0x3b = 0); else every channel,
    /// blended as the composite.
    pub fn blue_only(&self) -> bool {
        self.dest == LayersDest::Target3
    }
}

/// The callbacks' depth word: written with the clip far when not above 0
/// (`comiss 0, [0x00509400]; jb`, so a NaN is kept).
pub fn callback_depth(mem: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let d = mem.u32(composite_global::DEPTH)?;
    if f32::from_bits(d) <= 0.0 {
        let far = mem.u32(composite_global::CLIP_FAR)?;
        mem.set_u32(composite_global::DEPTH, far)?;
        return Ok(far);
    }
    Ok(d)
}

fn read_rect(mem: &dyn GuestMemory, at: u32) -> Result<[u32; 4], MemFault> {
    let mut rect = [0; 4];
    for (i, r) in rect.iter_mut().enumerate() {
        *r = mem.u32(at.wrapping_add(4 * i as u32))?;
    }
    Ok(rect)
}

fn lerp(sse: &mut Sse, rect: &[u32; 4], uv: [u32; 2]) -> Result<[u32; 2], Error> {
    let [left, right, top, bottom] = *rect;
    let x = sse.sub(right, left)?;
    let x = sse.mul(x, uv[0])?;
    let x = sse.add(x, left)?;
    let y = sse.sub(bottom, top)?;
    let y = sse.mul(y, uv[1])?;
    let y = sse.add(y, top)?;
    Ok([x, y])
}

fn corner(one: u32, [u, v]: [bool; 2]) -> [u32; 2] {
    [if u { one } else { 0 }, if v { one } else { 0 }]
}

/// The layers' block (`0x00026a36`..`0x00026c56`) for the branch byte,
/// the four colours packed by `0x00131ed0`.
pub fn layers_combiner(full: bool, colours: [u32; 4]) -> [u32; WORDS] {
    let mut w = [0; WORDS];
    w[54] = 0x8421; // texture modes: stages 0..3 2D
    w[10] = colours[0]; // c0[0]
    w[18] = colours[1]; // c1[0]
    w[11] = colours[2]; // c0[1]
    w[19] = colours[3]; // c1[1]
    w[34] = 0x0801_0902; // rgb inputs, stage 0
    w[45] = 0x3089; // rgb outputs, stage 0
    w[35] = 0x0a01_0b02; // rgb inputs, stage 1
    w[46] = 0x30ab; // rgb outputs, stage 1
    if full {
        w[53] = 0x0001_1003;
        w[12] = 0x00ff_0000; // c0[2]
        w[20] = 0xff00; // c1[2]
        w[36] = 0x0901_0a02;
        w[47] = 0xc00;
        w[43] = 0xff; // final combiner constant 0
        w[8] = 0x0b01_000c;
        w[9] = 0x800;
    } else {
        w[53] = 0x0001_1004;
        w[36] = 0x2829_2a2b;
        w[47] = 0xcd;
        w[37] = 0x0c0d_0000;
        w[48] = 0xc0;
        w[8] = 0x2c;
        w[9] = 0x2c00;
    }
    w
}

/// The layers' fan whose rectangle is at `rect_at`, under the call's
/// MXCSR and x87 control word; `mem` takes the callback's depth write.
pub fn layers(mem: &mut dyn GuestMemory, mxcsr: u32, x87_control: u16, rect_at: u32) -> Result<LayersFan, Error> {
    use layers_global as g;
    let datum = mem.u32(global::DATUM)?;
    let tag = mem.u32(mem.u32(g::TAG_TABLE)?.wrapping_add((datum & 0xffff) << 4).wrapping_add(8))?;
    let bitmap = mem.u32(tag.wrapping_add(g::BITMAP_DATUM))?;
    let layer = |n: u32| g::LAYERS + g::LAYER_STRIDE * n;
    let mut images = [0; 4];
    for (n, i) in images.iter_mut().enumerate() {
        *i = mem.u16(layer(n as u32))?;
    }
    let scale = mem.u32(composite_global::SCALE)?;
    let pack = |at: u32| -> Result<u32, Error> {
        crate::glow::pack_rgb([mem.u32(at)?, mem.u32(at + 4)?, mem.u32(at + 8)?], scale, x87_control)
    };
    let colours = [pack(g::COLOUR)?, pack(g::LAYER_COLOURS[0])?, pack(g::LAYER_COLOURS[1])?, pack(g::LAYER_COLOURS[2])?];
    let combiner = layers_combiner(mem.u8(g::BRANCH)? != 0, colours);
    let depth = callback_depth(mem)?;
    let rect = read_rect(&*mem, rect_at)?;
    let (one, two, aspect) = (mem.u32(screen_grid::constant::ONE)?, mem.u32(g::TWO)?, mem.u32(g::ASPECT)?);
    let mut sse = Sse { mxcsr };
    let (mut positions, mut coordinates) = ([[0; 4]; 4], [[[0; 4]; 4]; 4]);
    for (v, c) in CORNER_UV.into_iter().enumerate() {
        let uv = corner(one, c);
        positions[v] = screen_grid::position(&*mem, &mut sse, &rect, uv, depth)?;
        for n in 0..4u32 {
            let l = layer(n);
            let mut axis = |m0: u32, m1: u32, o: u32| -> Result<u32, Error> {
                let x = sse.mul(uv[0], two)?;
                let y = sse.mul(uv[1], two)?;
                let x = sse.sub(x, one)?;
                let x = sse.mul(x, mem.u32(l + m0)?)?;
                let y = sse.sub(y, one)?;
                let y = sse.mul(y, mem.u32(l + m1)?)?;
                let y = sse.mul(y, aspect)?;
                let x = sse.sub(x, y)?;
                sse.add(x, mem.u32(l + o)?)
            };
            coordinates[v][n as usize] = [axis(0x18, 0x1c, 0x24)?, axis(0x28, 0x2c, 0x34)?, 0, one];
        }
    }
    Ok(LayersFan { bitmap, images, combiner, positions, coordinates })
}

/// Whether the layers' composite takes the weather plate's placement
/// `0x0003a5c0` first (the branch byte clear): [`weather_plate_placement`].
pub fn weather_plate(mem: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(mem.u8(layers_global::BRANCH)? == 0)
}

/// The weather plate's placement `0x0003a5c0` (`stdcall`, nine arguments,
/// `ret 24h`), the path the layers' composite reaches.
pub const WEATHER_PLATE: u32 = 0x0003_a5c0;

/// `0x0003a5c0`'s image constants and globals.
pub mod plate_global {
    /// 1.0.
    pub const ONE: u32 = 0x0045_dbc0;
    /// The scale the depths are divided by and the result multiplied by
    /// (the layers' [`super::layers_global::DOT_X`] word).
    pub const SCALE: u32 = 0x0048_565c;
    /// Kind 2's cap on `1 / (far - near)` (the jump table at `0x0003b88c`).
    pub const CAP: u32 = 0x0045_dd60;
    /// The rescale (256) and its inverse.
    pub const RESCALE: u32 = 0x0045_dd5c;
    pub const UNRESCALE: u32 = 0x0045_dfa0;
    /// The fixed point's scale (65536) and its inverse.
    pub const FIXED: u32 = 0x0045_dcb0;
    pub const UNFIXED: u32 = 0x0045_dcac;
    /// 0.5.
    pub const HALF: u32 = 0x0045_dbbc;
    /// The least depth the result takes.
    pub const LIMIT: u32 = 0x0048_5658;
}

/// The block the layers' composite builds on its stack for the plate
/// (`0x00026e73`..`0x00026eec`): the fog's density and amount, its colour,
/// and at `+0x14` whether `dl` is clear (the fan goes into target 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateBlock {
    pub density: u32,
    pub amount: u32,
    pub colour: [u32; 3],
    pub target3: bool,
}

/// `0x0003a5c0`'s arguments: the block, the near and far depths, the
/// kind (the switch at `0x0003b88c`), the four flag arguments 4..7 and
/// where the result depth is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlateArgs {
    pub block: PlateBlock,
    pub near: u32,
    pub far: u32,
    pub kind: u32,
    pub flags: [u32; 4],
    pub result: u32,
}

impl PlateArgs {
    /// The layers' composite's call (`0x00026e89`..`0x00026ef0`): kind 2,
    /// flags 0, 0, 1, 0, the result into the depth word.
    pub fn fog(mem: &dyn GuestMemory) -> Result<Self, MemFault> {
        let lane = |i: u32| mem.u32(composite_global::COLOUR + 4 * i);
        let block = PlateBlock {
            density: mem.u32(layers_global::DENSITY)?,
            amount: mem.u32(global::AMOUNT)?,
            colour: [lane(0)?, lane(1)?, lane(2)?],
            target3: mem.u8(global::FLAG_DL)? == 0,
        };
        Ok(Self {
            block,
            near: mem.u32(layers_global::SPAN_NEAR)?,
            far: mem.u32(layers_global::SPAN_FAR)?,
            kind: 2,
            flags: [0, 0, 1, 0],
            result: composite_global::DEPTH,
        })
    }

    /// Why these arguments are not the path ported, `None` for the
    /// composite's. Kinds 0, 1 and 3, the other flags (argument 7 set
    /// skips the device setup, `0x0003a7c9`) and no result are other
    /// callers'.
    pub fn unported(&self) -> Option<String> {
        if self.kind != 2 {
            return Some(format!("kind {}", self.kind));
        }
        if self.flags != [0, 0, 1, 0] {
            return Some(format!("flags {:?}", self.flags));
        }
        if self.result == 0 {
            return Some("no result".into());
        }
        None
    }
}

/// What `0x0003a5c0` leaves when it returns true: where the fan draws
/// (the view's target, or target 3 when the block's byte is set), the
/// 60-word block it builds at `0x00484f68` and the depth it wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeatherPlate {
    pub dest: LayersDest,
    pub combiner: [u32; WORDS],
    pub depth: u32,
}

/// `fld dword a; fmul dword b` (or `a` alone), `fstp qword`, `floor`
/// (`0x00321099`), `_ftol2` (`0x00322b0c`). The floor of a finite double
/// is exact; its NaN and infinity path (`0x00324995`) is not modelled.
fn floor_int(x87_control: u16, a: u32, b: Option<u32>) -> Result<i32, Error> {
    let mut x = X87::new(x87_control);
    x.fld(a)?;
    if let Some(b) = b {
        x.fmul_m(b)?;
    }
    let (code, store) = x87::abi::st_f64(&x.0, 0);
    X87::ok(code)?;
    let d = f64::from_bits(store.value);
    if !d.is_finite() {
        return Err(Error::X87(x87::abi::status::UNMODELLED));
    }
    let mut y = X87::new(x87_control);
    X87::ok(x87::abi::ld_f64(&mut y.0, d.floor().to_bits()))?;
    let (code, store) = x87::abi::st_i32_trunc(&y.0, 0);
    X87::ok(code)?;
    Ok(store.value)
}

/// `0x0002b480` then `_ftol2`, held to a byte (`0x0003b435`..`0x0003b46b`).
fn floor_byte(x87_control: u16, v: u32) -> Result<u32, Error> {
    Ok(floor_int(x87_control, v, None)?.clamp(0, 0xff) as u32)
}

/// `0x00131fc0`: `v` times 255 (`fistp`, the control word's rounding) in
/// the top byte.
fn alpha_byte(v: u32, scale: u32, x87_control: u16) -> Result<u32, Error> {
    let mut x = X87::new(x87_control);
    x.fld(v)?;
    x.fld(scale)?;
    x.arith_reg(x87::abi::arith_op::MUL, 1, 0, true)?;
    Ok((x.fistp_m32()? as u32) << 24)
}

/// `fld dword v; fmul dword k; fidiv dword d; fstp dword`.
fn scaled_over(x87_control: u16, v: u32, k: u32, d: i32) -> Result<u32, Error> {
    let mut x = X87::new(x87_control);
    x.fld(v)?;
    x.fmul_m(k)?;
    X87::ok(x87::abi::arith(&mut x.0, x87::abi::arith_op::DIV, 0, x87::abi::src_kind::I32, d as u32 as u64))?;
    x.fstp_m32()
}

/// `0x0003a5c0` for `args` (the composite's: [`PlateArgs::unported`] is
/// `None`) under the call's MXCSR and x87 control word. `None` is its
/// false (`0x0003b880`), which writes nothing; true writes the result
/// depth through `args.result`.
///
/// The placement (`0x0003a5c0`..`0x0003a7c2`): `near` and `far` over the
/// scale, both times 256 when `far` so is not above 1, each floored to
/// sixteen-bit fractions. False when `far` is not above `near`; when
/// `1 / (far - near)` is above the cap, false if rescaled, else (argument
/// 6) the cap is the slope; false when `near` is below 0. Then the device
/// setup (`0x0003a7cf`..`0x0003a984`: the view's target, target 3 at
/// stage 0, clamp and point, blend and alpha test off, states `0x39`,
/// `0x52`, vertex slot `0xc`, program `0x0051f0f0`), the block from the
/// fixed-point near end, the slope's power of two (`0x0003aad4`) and kind
/// 2's words (`0x0003b29f`..`0x0003b545`: blend as the composite, or into
/// target 3 blue alone), and the result: the scale times `near` (over 256
/// when rescaled), at least the limit (`0x0003b821`..`0x0003b871`).
pub fn weather_plate_placement(mem: &mut dyn GuestMemory, mxcsr: u32, x87_control: u16, args: &PlateArgs) -> Result<Option<WeatherPlate>, Error> {
    use plate_global as g;
    let mut sse = Sse { mxcsr };
    let one = mem.u32(g::ONE)?;
    let scale = mem.u32(g::SCALE)?;
    let cap = mem.u32(g::CAP)?;
    let rescale = mem.u32(g::RESCALE)?;
    let (fixed, unfixed) = (mem.u32(g::FIXED)?, mem.u32(g::UNFIXED)?);
    let inv = sse.div(one, scale)?;
    let mut near = sse.mul(inv, args.near)?;
    let mut far = sse.mul(inv, args.far)?;
    let big = sse.mul(far, rescale)?;
    // Kind 2: `al` 1, so a small range is rescaled.
    let rescaled = !sse.below(one, big)?;
    if rescaled {
        near = sse.mul(near, rescale)?;
        far = big;
    }
    let near_i = floor_int(x87_control, near, Some(fixed))?;
    let far_i = floor_int(x87_control, far, Some(fixed))?;
    let near = sse.int(near_i)?;
    let near = sse.mul(near, unfixed)?;
    let far = sse.int(far_i)?;
    let far = sse.mul(far, unfixed)?;
    if !sse.above(far, near)? {
        return Ok(None);
    }
    let span = sse.sub(far, near)?;
    let mut slope = sse.div(one, span)?;
    if sse.above(slope, cap)? {
        // `0x0003a709`: `1 / cap`, read again from the slot after.
        sse.div(one, cap)?;
        slope = cap;
        if rescaled {
            return Ok(None);
        }
    }
    // Kind 2's `bl` is clear and argument 5 is 0: a negative near end fails.
    if sse.above(0, near)? {
        return Ok(None);
    }
    let target = mem.u16(composite_global::VIEW_TARGET)? as i16;

    // 0x0003a989: the fixed-point near end in bytes.
    let v = floor_int(x87_control, near, Some(fixed))?;
    let (mut low, mut high) = ((v & 0xff) as u32, v >> 8);
    if high > 0xff {
        (low, high) = (0xff, 0xff);
    }
    let lead = u32::from(rescaled) + 4;
    let tail: u32 = if rescaled { 4 } else { 0x18 };
    let mut n = 0u32;
    if sse.above(slope, one)? {
        loop {
            n += 1;
            if n > 31 {
                return Err(Error::Unbounded);
            }
            let p = sse.int(1 << n)?;
            if !sse.above(slope, p)? {
                break;
            }
        }
    }
    let byte_scale = mem.u32(composite_global::SCALE)?;
    let density = alpha_byte(args.block.density, byte_scale, x87_control)?;
    let amount = alpha_byte(args.block.amount, byte_scale, x87_control)?;
    let hi = floor_byte(x87_control, scaled_over(x87_control, slope, rescale, 1 << n)?)?;
    let half = {
        let mut x = X87::new(x87_control);
        x.fld(slope)?;
        x.fmul_m(mem.u32(g::HALF)?)?;
        x.fstp_m32()?
    };
    let lo = floor_byte(x87_control, half)?;
    let level = |over: u32, under: u32| -> u32 {
        let x = if n > over { 0x20 } else if n <= under { 0 } else { 0x10 };
        x << 12 | 0xd00
    };
    let mut w = [0u32; WORDS];
    for (i, x) in [
        (11, low << 24),
        (35, lead << 24 | 0x20_1140),
        (1, lead << 24 | 0x20_9140),
        (53, 0x1_1108),
        (10, 0xff_0000),
        (18, 0xff00),
        (34, 0x0801_0802),
        (45, 0x3045),
        (46, 0xc00),
        (27, 0xc00),
        (12, (high as u32) << 24),
        (36, 0x2020),
        (47, 0x4d00),
        (2, tail << 24 | 0x20_1140),
        (28, 0xd00),
        (13, 0x0100_0000),
        (37, 0xcc20_2d20),
        (48, 0xc00),
        (3, 0xdd20_2df1),
        (29, 0xd00),
        // Kind 2 (0x0003b331..0x0003b4f2).
        (54, 0x21), // texture modes: stages 0 and 1 2D
        (14, if rescaled { 0x7f00_00ff } else { 0 }),
        (38, (if n > 3 { 0x20 } else { 0 }) | 0x1d20_1d00),
        (49, level(2, 1)),
        (4, 0x1801_1120),
        (30, 0xc00),
        (15, density),
        (23, amount),
        (39, 0x3911_1912),
        (50, 0xd00),
        (5, (if n > 6 { 0x20 } else { 0 }) | 0x0d20_0d00),
        (31, level(5, 4)),
        (24, hi << 24 | lo),
        (6, 0x1d12_0c02),
        (7, 0x1d0d_0d20),
        (33, 0x4d00),
        (32, (if n > 0 { 0x10 } else { 0 }) << 12 | 0xd00),
    ] {
        w[i] = x;
    }
    let dest = if args.block.target3 {
        // 0x0003b506: target 3, blue alone, blend off; no final constant.
        w[8] = 0x1d;
        w[9] = 0;
        LayersDest::Target3
    } else {
        w[43] = crate::glow::pack_rgb(args.block.colour, byte_scale, x87_control)?;
        w[8] = 0x1d01_0000;
        w[9] = 0x1d00;
        LayersDest::View(target)
    };
    let mut depth = sse.mul(scale, near)?;
    if rescaled {
        depth = sse.mul(depth, mem.u32(g::UNRESCALE)?)?;
    }
    let limit = mem.u32(g::LIMIT)?;
    if !sse.below(limit, depth)? {
        depth = limit;
    }
    mem.set_u32(args.result, depth)?;
    Ok(Some(WeatherPlate { dest, combiner: w, depth }))
}

/// The layers' composite's corners (`0x00027520`) at the callback's
/// depth: attribute 1 the rectangle's lerp, 2 the dot vector with the
/// branch byte set and else the lerp times the target scale (the jump
/// table at `0x000276b0`, `0x000275bd`), 3 the dot vector, 4 the scaled
/// lerp. `mem` takes the callback's depth write.
fn layers_corners(mem: &mut dyn GuestMemory, sse: &mut Sse, rect_at: u32) -> Result<([[u32; 4]; 4], [[[u32; 4]; 4]; 4]), Error> {
    use layers_global as g;
    let depth = callback_depth(mem)?;
    let branch = mem.u8(g::BRANCH)? != 0;
    let rect = read_rect(&*mem, rect_at)?;
    let one = mem.u32(screen_grid::constant::ONE)?;
    let span = sse.sub(mem.u32(g::SPAN_FAR)?, mem.u32(g::SPAN_NEAR)?)?;
    let floor = mem.u32(g::SPAN_MIN)?;
    let span = if sse.above(floor, span)? { floor } else { span };
    let inv = sse.div(one, span)?;
    let x = sse.mul(mem.u32(g::DOT_X)?, inv)?;
    let z = sse.mul(mem.u32(g::SPAN_NEAR)?, inv)?;
    let z = sse.sub(0, z)?;
    let dot = [x, 0, z, 0];
    let k = mem.u32(g::TARGET_SCALE)?;
    let (mut positions, mut coordinates) = ([[0; 4]; 4], [[[0; 4]; 4]; 4]);
    for (n, c) in CORNER_UV.into_iter().enumerate() {
        let uv = corner(one, c);
        positions[n] = screen_grid::position(&*mem, sse, &rect, uv, depth)?;
        let [x, y] = lerp(sse, &rect, uv)?;
        let scaled = [sse.mul(x, k)?, sse.mul(y, k)?, 0, one];
        coordinates[n] = [[x, y, 0, one], if branch { dot } else { scaled }, dot, scaled];
    }
    Ok((positions, coordinates))
}

/// The layers' composite's fan when the weather plate returned true
/// (`0x00026efd`..`0x00026f7e`): the plate's block and destination,
/// target 3 at stage 0 and target `0x13` at stage 1, at the plate's depth.
pub fn plate_fan(mem: &mut dyn GuestMemory, mxcsr: u32, rect_at: u32, plate: WeatherPlate) -> Result<LayersComposite, Error> {
    let mut sse = Sse { mxcsr };
    let (positions, coordinates) = layers_corners(mem, &mut sse, rect_at)?;
    Ok(LayersComposite { dest: plate.dest, combiner: plate.combiner, positions, coordinates })
}

/// `[0, 1]` as `0x00027209`..`0x0002737b` clamp: below 0 to 0, above 1
/// to 1, a NaN kept.
fn clamp01(v: u32) -> u32 {
    let f = f32::from_bits(v);
    if 0.0 > f {
        0
    } else if f > 1.0 {
        1.0f32.to_bits()
    } else {
        v
    }
}

/// The layers' composite's block (`0x0002737b`..`0x000274ec`): `v` the
/// twelve clamped amounts, `colour` `0x00131ed0` of the fog's colour
/// (used only into the view).
pub fn layers_composite_combiner(v: [u32; 12], colour: u32, scale: u32, x87_control: u16, target3: bool) -> Result<[u32; WORDS], Error> {
    let p = |i: usize| crate::colour_pass::pack_colour([v[i], v[i + 1], v[i + 2], v[i + 3]], scale, x87_control);
    let mut w = [0; WORDS];
    w[54] = 0xa621; // texture modes: 2D, dot product, dot st, 2D
    w[55] = 0x44; // dot mapping
    w[53] = 0x0001_1006;
    w[10] = p(0)?;
    w[14] = p(4)?;
    w[22] = p(8)?;
    for (i, x) in [
        (0, 0x1a20_b120), (26, 0x0001_0c00), (34, 0x0a20_a120), (45, 0x0001_0c00), (35, 0x0b0c_1b1c), (46, 0xcd),
        (2, 0x2c2d_0000), (28, 0x40), (12, 0x00ff_0000), (20, 0xff00), (36, 0x010c_020c), (47, 0x3045),
        (3, 0x2425_1a1a), (29, 0x58), (4, 0x1415_0000), (30, 0xc0), (38, 0x0a01_0a02), (49, 0xcd),
        (39, 0x1c0c_3c0d), (50, 0xc00),
    ] {
        w[i] = x;
    }
    if target3 {
        w[8] = 0xc;
        w[9] = 0;
    } else {
        w[43] = colour;
        w[8] = 0x0c01_0000;
        w[9] = 0xc00;
    }
    Ok(w)
}

/// The layers' composite's fan, the set branch, whose rectangle is at
/// `rect_at`; `mem` takes the setup's depth write.
pub fn layers_composite(mem: &mut dyn GuestMemory, mxcsr: u32, x87_control: u16, rect_at: u32) -> Result<LayersComposite, Error> {
    use layers_global as g;
    composite_writes(mem)?;
    let dest = match mem.u8(global::FLAG_DL)? {
        0 => LayersDest::Target3,
        _ => LayersDest::View(mem.u16(composite_global::VIEW_TARGET)? as i16),
    };
    let mut v = [0; 12];
    for n in 0..4u32 {
        v[n as usize] = clamp01(mem.u32(g::LAYERS + g::LAYER_STRIDE * n + 0x14)?);
    }
    let (density, amount) = (clamp01(mem.u32(g::DENSITY)?), clamp01(mem.u32(global::AMOUNT)?));
    v[4..8].fill(density);
    v[8..12].fill(amount);
    let scale = mem.u32(composite_global::SCALE)?;
    let target3 = dest == LayersDest::Target3;
    let colour = if target3 {
        0
    } else {
        let lane = |i: u32| mem.u32(composite_global::COLOUR + 4 * i);
        crate::glow::pack_rgb([lane(0)?, lane(1)?, lane(2)?], scale, x87_control)?
    };
    let combiner = layers_composite_combiner(v, colour, scale, x87_control, target3)?;
    let mut sse = Sse { mxcsr };
    let (positions, coordinates) = layers_corners(mem, &mut sse, rect_at)?;
    Ok(LayersComposite { dest, combiner, positions, coordinates })
}

/// What `0x00025dc0` draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fans {
    /// No fan: the gate stopped it, or neither flag is set.
    None,
    /// `cl`: the layers into target `0x13`, then their composite.
    Layers,
    /// `dl` alone: target 3's composite.
    Composite,
}

/// The gate: the datum neither -1 nor 0 and the amount above 0, under
/// the call's MXCSR.
pub fn gate(mem: &dyn GuestMemory, mxcsr: u32) -> Result<bool, Error> {
    let datum = mem.u32(global::DATUM)?;
    if datum == u32::MAX || datum == 0 {
        return Ok(false);
    }
    Sse { mxcsr }.above(mem.u32(global::AMOUNT)?, mem.u32(global::ZERO)?)
}

/// The fans `0x00025dc0` draws for the flag bytes `cl` and `dl`.
pub fn fans(mem: &dyn GuestMemory, mxcsr: u32, cl: u8, dl: u8) -> Result<Fans, Error> {
    if !gate(mem, mxcsr)? {
        return Ok(Fans::None);
    }
    Ok(match (cl, dl) {
        (0, 0) => Fans::None,
        (0, _) => Fans::Composite,
        _ => Fans::Layers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::FlatMemory;

    const MXCSR: u32 = 0x1f80;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x0012_0000);
        m.set_u32(global::DATUM, 0xe1a3_0012).unwrap();
        m.set_f32(global::AMOUNT, 0.5).unwrap();
        m.set_f32(global::ZERO, 0.0).unwrap();
        m
    }

    #[test]
    fn cl_draws_the_layers_and_dl_alone_the_composite() {
        let m = mem();
        assert_eq!(fans(&m, MXCSR, 1, 0).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR, 0, 1).unwrap(), Fans::Composite);
        assert_eq!(fans(&m, MXCSR, 0, 0).unwrap(), Fans::None);
    }

    #[test]
    fn no_datum_or_no_amount_draws_nothing() {
        for datum in [0, u32::MAX] {
            let mut m = mem();
            m.set_u32(global::DATUM, datum).unwrap();
            assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::None, "{datum:#x}");
        }
        for amount in [0.0f32, -1.0, f32::NAN] {
            let mut m = mem();
            m.set_f32(global::AMOUNT, amount).unwrap();
            assert_eq!(fans(&m, MXCSR, 1, 1).unwrap(), Fans::None, "{amount}");
        }
    }

    fn composite_mem() -> (FlatMemory, u32, u32) {
        let (esp, rect) = (0x0050_0000, 0x0050_1000);
        let mut m = mem();
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, screen_grid::constant::ONE, 1.0);
        f(&mut m, screen_grid::constant::DEPTH, 16_777_215.0);
        f(&mut m, screen_grid::constant::NEAR, 0.0625);
        f(&mut m, screen_grid::constant::FAR, 1024.0);
        f(&mut m, composite_global::SCALE, 255.0);
        f(&mut m, composite_global::CLIP_FAR, 1024.0);
        for (i, c) in [1.0f32, 0.5, 0.25].into_iter().enumerate() {
            f(&mut m, composite_global::COLOUR + 4 * i as u32, c);
        }
        for (i, v) in [0.0f32, 640.0, 0.0, 480.0].into_iter().enumerate() {
            f(&mut m, rect + 4 * i as u32, v);
        }
        m.set_u32(esp, SETUP_RETURN).unwrap();
        m.set_u32(esp + SETUP_FRAME, SECOND_RETURN).unwrap();
        m.set_u32(esp + SETUP_FRAME + 4, rect).unwrap();
        m.set_u32(esp + SETUP_FRAME + 8, setup::COMPOSITE_CORNERS).unwrap();
        (m, esp, rect)
    }

    #[test]
    fn the_composites_fan_is_found_by_its_frame() {
        let (mut m, esp, rect) = composite_mem();
        assert_eq!(fan_call(&m, esp, SECOND_RETURN, setup::COMPOSITE_CORNERS), Some(rect));
        assert_eq!(fan_call(&m, esp, SECOND_RETURN, setup::LAYERS_COMPOSITE_CORNERS), None);
        assert_eq!(fan_call(&m, esp, LAYERS_RETURN, setup::COMPOSITE_CORNERS), None);
        m.set_u32(esp, 0x0003_4530).unwrap();
        assert_eq!(fan_call(&m, esp, SECOND_RETURN, setup::COMPOSITE_CORNERS), None);
    }

    #[test]
    fn the_composite_is_the_rectangle_with_the_colours_block() {
        let (mut m, _, rect) = composite_mem();
        m.set_u16(composite_global::VIEW_TARGET, 0).unwrap();
        let c = composite(&m, MXCSR, 0x027f, rect).unwrap();
        assert_eq!(c.target, 0);
        // 255, 127.5 (to even: 128), 63.75 (64), red high.
        assert_eq!(c.colour, 0x00ff_8040);
        assert_eq!(c.combiner[43], c.colour);
        assert_eq!(c.combiner[53], 0x0001_1001);
        assert_eq!(c.combiner[54], 1);
        let f = |v: [u32; 4]| v.map(f32::from_bits);
        assert_eq!(f(c.coordinates[0]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[1]), [0.0, 480.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[2]), [640.0, 480.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[3]), [640.0, 0.0, 0.0, 1.0]);
        for n in 0..4 {
            assert_eq!(c.positions[n][..2], c.coordinates[n][..2], "{n}");
        }
        assert!(c.positions.iter().all(|p| p[2] == c.positions[0][2] && p[3] == c.positions[0][3]));
    }

    #[test]
    fn the_composites_writes_are_the_depth_word() {
        let (mut m, _, _) = composite_mem();
        composite_writes(&mut m).unwrap();
        assert_eq!(m.u32(composite_global::DEPTH).unwrap(), 1024.0f32.to_bits());
    }

    /// The layers' inputs over [`composite_mem`]: the tag table, the fog
    /// patch's bitmap, four layers mapping `(u, v)` to `(2u - 1, 1 - 2v)`.
    fn layers_mem() -> (FlatMemory, u32) {
        use layers_global as g;
        let (mut m, _, rect) = composite_mem();
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        m.set_u32(g::TAG_TABLE, 0x0050_3000).unwrap();
        m.set_u32(0x0050_3000 + (0x12 << 4) + 8, 0x0050_4000).unwrap();
        m.set_u32(0x0050_4000 + g::BITMAP_DATUM, 0xabcd_0001).unwrap();
        f(&mut m, g::TWO, 2.0);
        f(&mut m, g::ASPECT, 1.0);
        for n in 0..4u32 {
            let l = g::LAYERS + g::LAYER_STRIDE * n;
            m.set_u16(l, n as u16 + 1).unwrap();
            f(&mut m, l + 0x18, 1.0);
            f(&mut m, l + 0x2c, 1.0);
            f(&mut m, l + 0x14, [2.0, 0.5, -1.0, 0.25][n as usize]);
        }
        f(&mut m, g::COLOUR, 1.0);
        (m, rect)
    }

    #[test]
    fn the_layers_read_the_patchs_bitmap_and_map_each_corner() {
        let (mut m, rect) = layers_mem();
        m.set_f32(composite_global::DEPTH, 0.0).unwrap();
        let l = layers(&mut m, MXCSR, 0x027f, rect).unwrap();
        assert_eq!(l.bitmap, 0xabcd_0001);
        assert_eq!(l.images, [1, 2, 3, 4]);
        // The callback's write: a depth word not above 0 takes the clip far.
        assert_eq!(m.u32(composite_global::DEPTH).unwrap(), 1024.0f32.to_bits());
        assert_eq!(l.combiner[10], 0x00ff_0000);
        assert_eq!((l.combiner[53], l.combiner[54], l.combiner[8]), (0x0001_1004, 0x8421, 0x2c));
        let f = |v: [u32; 4]| v.map(f32::from_bits);
        assert_eq!(f(l.coordinates[0][0]), [-1.0, 1.0, 0.0, 1.0]);
        assert_eq!(f(l.coordinates[2][3]), [1.0, -1.0, 0.0, 1.0]);
        assert_eq!(f(l.positions[2])[..2], [640.0, 480.0]);
        m.set_u8(layers_global::BRANCH, 1).unwrap();
        let full = layers(&mut m, MXCSR, 0x027f, rect).unwrap();
        assert_eq!((full.combiner[53], full.combiner[43], full.combiner[8]), (0x0001_1003, 0xff, 0x0b01_000c));
    }

    #[test]
    fn the_layers_composite_clamps_its_amounts_and_reads_the_dot_vector() {
        use layers_global as g;
        let (mut m, rect) = layers_mem();
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        f(&mut m, g::DOT_X, 640.0);
        f(&mut m, g::SPAN_NEAR, 24.0);
        f(&mut m, g::SPAN_FAR, 1048.0);
        f(&mut m, g::SPAN_MIN, 1.0);
        f(&mut m, g::TARGET_SCALE, 0.5);
        f(&mut m, g::DENSITY, 1.0);
        m.set_u8(global::FLAG_DL, 1).unwrap();
        m.set_u8(g::BRANCH, 1).unwrap();
        m.set_u16(composite_global::VIEW_TARGET, 0).unwrap();
        let c = layers_composite(&mut m, MXCSR, 0x027f, rect).unwrap();
        assert_eq!(c.dest, LayersDest::View(0));
        assert!(!c.blue_only());
        // Layer amounts 2, 0.5, -1, 0.25 clamp to 1, 0.5, 0, 0.25.
        assert_eq!(c.combiner[10], 0xff80_0040);
        assert_eq!(c.combiner[14], 0xffff_ffff);
        assert_eq!(c.combiner[22], 0x8080_8080);
        assert_eq!(c.combiner[43], 0x00ff_8040);
        let f = |v: [u32; 4]| v.map(f32::from_bits);
        assert_eq!(f(c.coordinates[1][1]), [0.625, 0.0, -0.0234375, 0.0]);
        assert_eq!(c.coordinates[1][1], c.coordinates[1][2]);
        assert_eq!(f(c.coordinates[2][3]), [320.0, 240.0, 0.0, 1.0]);
        assert_eq!(f(c.coordinates[2][0]), [640.0, 480.0, 0.0, 1.0]);
        assert_eq!(m.u32(composite_global::DEPTH).unwrap(), 1024.0f32.to_bits());
        m.set_u8(global::FLAG_DL, 0).unwrap();
        let t3 = layers_composite(&mut m, MXCSR, 0x027f, rect).unwrap();
        assert_eq!(t3.dest, LayersDest::Target3);
        assert!(t3.blue_only());
        assert_eq!((t3.combiner[8], t3.combiner[9], t3.combiner[43]), (0xc, 0, 0));
        assert!(!weather_plate(&{ let mut w = mem(); w.set_u8(g::BRANCH, 1).unwrap(); w }).unwrap());
        assert!(weather_plate(&mem()).unwrap());
    }

    /// The plate's constants over [`layers_mem`], the scale `scale`, the
    /// fog's density 1, amount 0.5, colour (1, 0.5, 0.25) into the view.
    fn plate_mem(scale: f32, near: f32, far: f32) -> (FlatMemory, u32) {
        use plate_global as p;
        let (mut m, rect) = layers_mem();
        let f = |m: &mut FlatMemory, a: u32, v: f32| m.set_f32(a, v).unwrap();
        for (a, v) in [
            (p::ONE, 1.0),
            (p::SCALE, scale),
            (p::CAP, 128.0),
            (p::RESCALE, 256.0),
            (p::UNRESCALE, 1.0 / 256.0),
            (p::FIXED, 65536.0),
            (p::UNFIXED, 1.0 / 65536.0),
            (p::HALF, 0.5),
            (p::LIMIT, 10.0),
            (layers_global::SPAN_NEAR, near),
            (layers_global::SPAN_FAR, far),
            (layers_global::SPAN_MIN, 1.0),
            (layers_global::DENSITY, 1.0),
            (layers_global::TARGET_SCALE, 0.5),
        ] {
            f(&mut m, a, v);
        }
        m.set_u8(global::FLAG_DL, 1).unwrap();
        m.set_f32(composite_global::DEPTH, 0.0).unwrap();
        (m, rect)
    }

    fn place(m: &mut FlatMemory) -> Option<WeatherPlate> {
        let args = PlateArgs::fog(&*m).unwrap();
        assert_eq!(args.unported(), None);
        weather_plate_placement(m, MXCSR, 0x027f, &args).unwrap()
    }

    #[test]
    fn the_composites_plate_call_is_kind_2_and_other_arguments_are_named() {
        let (m, _) = plate_mem(1024.0, 24.0, 1048.0);
        let a = PlateArgs::fog(&m).unwrap();
        assert_eq!((a.kind, a.flags, a.result), (2, [0, 0, 1, 0], composite_global::DEPTH));
        assert_eq!((a.near, a.far), (24.0f32.to_bits(), 1048.0f32.to_bits()));
        assert_eq!(a.block, PlateBlock { density: 1.0f32.to_bits(), amount: 0.5f32.to_bits(), colour: [1.0f32.to_bits(), 0.5f32.to_bits(), 0.25f32.to_bits()], target3: false });
        assert_eq!(PlateArgs { kind: 3, ..a }.unported().as_deref(), Some("kind 3"));
        assert_eq!(PlateArgs { flags: [0, 0, 1, 1], ..a }.unported().as_deref(), Some("flags [0, 0, 1, 1]"));
        assert_eq!(PlateArgs { result: 0, ..a }.unported().as_deref(), Some("no result"));
    }

    #[test]
    fn a_wide_plate_is_placed_into_the_view_with_its_block_and_depth() {
        // 24 and 1048 over 1024: 0.0234375 and 1.0234375, slope 1, n 0.
        let (mut m, _) = plate_mem(1024.0, 24.0, 1048.0);
        let p = place(&mut m).unwrap();
        assert_eq!(p.dest, LayersDest::View(0));
        let w = p.combiner;
        // The near end 1536 / 65536: low 0, high 6.
        assert_eq!((w[11], w[12]), (0, 0x0600_0000));
        assert_eq!((w[35], w[1], w[2]), (0x0420_1140, 0x0420_9140, 0x1820_1140));
        assert_eq!((w[53], w[54], w[14]), (0x1_1108, 0x21, 0));
        assert_eq!((w[38], w[5], w[49], w[31], w[32]), (0x1d20_1d00, 0x0d20_0d00, 0xd00, 0xd00, 0xd00));
        // 1 * 256 held to 255; 0.5 floored to 0.
        assert_eq!(w[24], 0xff00_0000);
        // Density 1 and amount 0.5 (127.5 to even: 128) in the top byte.
        assert_eq!((w[15], w[23]), (0xff00_0000, 0x8000_0000));
        assert_eq!((w[43], w[8], w[9]), (0x00ff_8040, 0x1d01_0000, 0x1d00));
        // 1024 * 0.0234375 = 24, over the limit of 10.
        assert_eq!(p.depth, 24.0f32.to_bits());
        assert_eq!(m.u32(composite_global::DEPTH).unwrap(), p.depth);
    }

    #[test]
    fn a_narrow_plate_takes_the_cap_and_its_power_of_two() {
        // 10 and 10.001: slope 65536 / 65 over the cap, so 128 and n 7.
        let (mut m, _) = plate_mem(1.0, 10.0, 10.001);
        m.set_u8(global::FLAG_DL, 0).unwrap();
        let p = place(&mut m).unwrap();
        let w = p.combiner;
        // The near end 655360: high above 255, both bytes 255.
        assert_eq!((w[11], w[12]), (0xff00_0000, 0xff00_0000));
        assert_eq!((w[38], w[5], w[49], w[31], w[32]), (0x1d20_1d20, 0x0d20_0d20, 0x20d00, 0x20d00, 0x10d00));
        // 128 * 256 / 128 held to 255; 128 * 0.5 = 64.
        assert_eq!(w[24], 0xff00_0040);
        // Into target 3: no final constant.
        assert_eq!(p.dest, LayersDest::Target3);
        assert_eq!((w[43], w[8], w[9]), (0, 0x1d, 0));
        assert_eq!(p.depth, 10.0f32.to_bits());
    }

    #[test]
    fn a_small_range_is_rescaled_by_256() {
        // 0.001 and 0.003: far * 256 is not above 1.
        let (mut m, _) = plate_mem(1.0, 0.001, 0.003);
        let p = place(&mut m).unwrap();
        let w = p.combiner;
        assert_eq!(w[14], 0x7f00_00ff);
        assert_eq!((w[35], w[2]), (0x0520_1140, 0x0420_1140));
        // The slope 65536 / 33554 is under 2: n 1.
        assert_eq!((w[49], w[32]), (0xd00, 0x10d00));
        // The result is the near end over 256, held to the limit.
        assert_eq!(p.depth, 10.0f32.to_bits());
        m.set_f32(plate_global::LIMIT, 0.0).unwrap();
        let p = place(&mut m).unwrap();
        assert_eq!(f32::from_bits(p.depth), 16777.0 / 65536.0 / 256.0);
    }

    #[test]
    fn an_empty_negative_or_narrow_rescaled_plate_is_refused_and_writes_nothing() {
        for (near, far) in [(5.0f32, 5.0f32), (6.0, 5.0), (-2.0, 5.0), (0.001, 0.0010001)] {
            let (mut m, _) = plate_mem(1.0, near, far);
            assert_eq!(place(&mut m), None, "{near} {far}");
            assert_eq!(m.u32(composite_global::DEPTH).unwrap(), 0, "{near} {far}");
        }
    }

    #[test]
    fn the_plates_fan_samples_the_scaled_lerp_for_stage_1() {
        let (mut m, rect) = plate_mem(1024.0, 24.0, 1048.0);
        let p = place(&mut m).unwrap();
        let c = plate_fan(&mut m, MXCSR, rect, p.clone()).unwrap();
        assert_eq!((c.dest, c.combiner), (p.dest, p.combiner));
        let f = |v: [u32; 4]| v.map(f32::from_bits);
        assert_eq!(f(c.coordinates[2][0]), [640.0, 480.0, 0.0, 1.0]);
        // The branch byte clear: attribute 2 is attribute 4.
        assert_eq!(f(c.coordinates[2][1]), [320.0, 240.0, 0.0, 1.0]);
        assert_eq!(c.coordinates[2][1], c.coordinates[2][3]);
        assert_ne!(c.coordinates[2][2], c.coordinates[2][1]);
        // At the plate's depth (24), not the clip far.
        let mut s = Sse { mxcsr: MXCSR };
        let at = screen_grid::position(&m, &mut s, &read_rect(&m, rect).unwrap(), [0, 0], 24.0f32.to_bits()).unwrap();
        assert_eq!(c.positions[0], at);
    }

    #[test]
    fn a_denormal_amount_draws() {
        // The Pentium III has no DAZ (`simd`), so the smallest denormal is
        // above 0.
        let mut m = mem();
        m.set_u32(global::AMOUNT, 1).unwrap();
        assert_eq!(fans(&m, MXCSR, 1, 0).unwrap(), Fans::Layers);
        assert_eq!(fans(&m, MXCSR | 0x0040, 1, 0).unwrap(), Fans::Layers);
    }
}
