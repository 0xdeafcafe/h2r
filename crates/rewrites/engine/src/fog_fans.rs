//! The second fog's screen fans (`0x00025dc0`,
//! docs/graphics/retire-d3d-fog-fans.md): which fans it draws, as a pure
//! function of guest memory and its two flag bytes.
//!
//! (Code, `h2 disasm 0x25dc0 40`, `0x26880 240`, `0x26e50 410`,
//! `0x276d0 160`, `0x34420 215`.)
//!
//! `0x00025dc0(cl, dl)` draws only when the fog's tag datum
//! `[0x004857b8]` is neither -1 nor 0 and `[0x004857d8]` is above
//! `[0x0045dbd8]` (`comiss; jbe`). Then it stores `cl` and `dl` at
//! `0x005093fd` and `0x005093fe` and draws through the fan `0x00034420`:
//!
//! | Flags | Fans (setup, attribute callback) |
//! |---|---|
//! | `cl` set | [`Fans::Layers`]: `0x00026880`, `0x00026ca0`, into target `0x13` ([`layers`]); then `0x00026e50`, `0x00027520` ([`layers_composite`]) |
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

use crate::light_constants::{Error, Sse};
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
/// `0x0003a5c0` first (the branch byte clear), which has no native stage.
pub fn weather_plate(mem: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(mem.u8(layers_global::BRANCH)? == 0)
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
    let depth = mem.u32(composite_global::DEPTH)?;
    let rect = read_rect(&*mem, rect_at)?;
    let one = mem.u32(screen_grid::constant::ONE)?;
    let mut sse = Sse { mxcsr };
    // Attributes 2 and 3: the same dot vector at every corner.
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
        positions[n] = screen_grid::position(&*mem, &mut sse, &rect, uv, depth)?;
        let [x, y] = lerp(&mut sse, &rect, uv)?;
        let scaled = [sse.mul(x, k)?, sse.mul(y, k)?, 0, one];
        coordinates[n] = [[x, y, 0, one], dot, dot, scaled];
    }
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
