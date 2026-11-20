//! Model decorators as the guest builds and draws them
//! (docs/graphics/native-decorators.md). Research points (Code, `h2
//! disasm`):
//!
//! - The list builder `0x00041640` adds, per visible cluster (count
//!   `[0x00509438]`, records of `0x10` bytes at [`VISIBLE_CLUSTERS`], the
//!   cluster's origin at `+4`), each of the cluster's placements that pass
//!   its culling as one item of list 0 (`0x00040e30`): `+0` the build and
//!   `+4` the draw picked by the decorator class's type (class `+4`; type 0,
//!   models, build [`DECORATOR_BUILD`] and draw [`DECORATOR_DRAW`]), `+8`
//!   the permutation's shader, `+0xc` the argument (placement index `<< 8`,
//!   the cluster's slot in the low byte, the permutation's detail level,
//!   byte `+9`, in bits 30 and 31), the layer mask from the shader
//!   (`0x0001bf50`).
//! - The build `0x0004de20` (stdcall, seven arguments as layer-draw.md
//!   gives them) takes nothing in layer 1; it asks the geometry cache
//!   (`0x0012de70`, flags 3) for the placement's geometry record and the
//!   set's geometry, and declines either one not given; it fills the entry
//!   with the implementation the shader's template gives for the layer
//!   (vertex type 1).
//! - The draw `0x0004e260`, once per placement: `TwoSidedLighting` on,
//!   `c[53]` = (`[0x0052598c + 4·lod]`, `[0x00525998 + 4·lod]`, 0, 1), the
//!   set's vertex stream (set `+0x34`) and the shader's pass; then for each
//!   of the placement's instances (`placement +8` first, `+0xa` count,
//!   in the geometry record's instance block `[record +0x28]`: `+0`
//!   count, `+4` base, `0x18` bytes each) that names a model with indices,
//!   `SET_VERTEX_DATA4F` of `v11..v15` ([`registers`]) and one
//!   `DrawIndexedVertices(TRIANGLELIST, count, [set +0x2c] + 2·first)` of
//!   the model's index range (set `+0x1c`, `8` bytes each: `+4` first,
//!   `+6` count); then `TwoSidedLighting` off.
//!
//! The arithmetic is the draw's, in its order and precision: SSE single
//! steps in `f32`, x87 steps in `f64` rounded where the guest stores. Its
//! constants are read from guest memory ([`Constants::read`]).

use frame_scene::decorator::{DecoratorDraw, DecoratorInstance, REGISTERS};
use frame_scene::Row;

use crate::GuestRead;

/// The model decorators' build and draw (an item's `+0` and `+4`).
pub const DECORATOR_BUILD: u32 = 0x0004_de20;
pub const DECORATOR_DRAW: u32 = 0x0004_e260;
/// The other decorator types' builds and draws (decals `0x0004dfa0` with
/// `0x0004ebd0`; quads `0x0004e0d0` with `0x0004ede0` or `0x0004efa0`):
/// transparent layers, not typed here.
pub const OTHER_DECORATOR_BUILDS: [u32; 2] = [0x0004_dfa0, 0x0004_e0d0];
/// The list builder that adds the placements.
pub const LIST_BUILDER: u32 = 0x0004_1640;

/// `[0x004e0348]`: the active structure BSP; its decorator block at `+0x238`.
pub const ACTIVE_BSP: u32 = 0x004e_0348;
pub const BSP_DECORATORS: u32 = 0x238;
/// The block's geometry records (`0x2c` bytes; the geometry block info
/// first, the instance block pointer at `+0x28`) and placements (`0x18`).
pub const BLOCK_GEOMETRY: u32 = 0x14;
pub const BLOCK_PLACEMENTS: u32 = 0x1c;
pub const GEOMETRY_SIZE: u32 = 0x2c;
pub const GEOMETRY_INSTANCES: u32 = 0x28;
pub const PLACEMENT_SIZE: u32 = 0x18;
/// Placement: `+0` palette index (i8), `+6` geometry record (i16), `+8`
/// first instance (i16), `+0xa` instances (i16).
pub const PLACEMENT_GEOMETRY: u32 = 6;
pub const PLACEMENT_FIRST: u32 = 8;
pub const PLACEMENT_COUNT: u32 = 0xa;
/// A geometry block info's resident byte: set, the cache gives it at once
/// (`0x0012de70`'s first test).
pub const RESIDENT: u32 = 0x1e;

/// `[0x004e0350]`: the scenario; its decorator palette at `+0x37c` (8
/// bytes each, the set's tag datum at `+4`).
pub const SCENARIO: u32 = 0x004e_0350;
pub const SCENARIO_PALETTE: u32 = 0x37c;
/// `[0x004e3b44]`: the tag table, `0x10` bytes per tag, its definition at `+8`.
pub const TAG_TABLE: u32 = 0x004e_3b44;
/// The visible clusters' records (`0x10` bytes, the origin at `+4`).
pub const VISIBLE_CLUSTERS: u32 = 0x004c_5700;

/// Set definition: `+8` and `+0xc` the fade (base, scale), `+0x14` the
/// classes (`0x14` bytes, permutations at `+0x10`), `+0x18` the models'
/// count and `+0x1c` the models, `+0x2c` the index buffer, `+0x38` the
/// geometry block info.
pub const SET_FADE: u32 = 8;
pub const SET_CLASSES: u32 = 0x14;
pub const CLASS_SIZE: u32 = 0x14;
pub const CLASS_PERMUTATIONS: u32 = 0x10;
pub const SET_MODEL_COUNT: u32 = 0x18;
pub const SET_MODELS: u32 = 0x1c;
pub const SET_GEOMETRY: u32 = 0x38;
/// Permutation (`0x28` bytes): `+8` flags (bit 0, the instance's up axis),
/// `+0xa` model (u8), `+0xc`, `+0x10` scale, `+0x14`, `+0x18` tints,
/// `+0x1c`, `+0x20` the colours' weights.
pub const PERMUTATION_SIZE: u32 = 0x28;
/// Instance (`0x18` bytes): `+0` the packed word, `+4` the position, `+8`
/// and `+0xc` colours, `+0x10` the normal, `+0x14` the fade input.
pub const INSTANCE_SIZE: u32 = 0x18;

/// `c[53]`'s two tables, by detail level.
pub const LOD_FACTORS: [u32; 2] = [0x0052_598c, 0x0052_5998];

/// Where the draw's constants are (the executable's read-only data).
pub mod constant {
    pub const ONE: u32 = 0x0045_dbc0;
    pub const ORIGIN_BIAS: u32 = 0x0045_dc10;
    pub const POSITION: u32 = 0x0045_dc18;
    pub const POSITION_XY: u32 = 0x0045_dd38;
    pub const POSITION_Z: u32 = 0x0045_dd44;
    pub const NIBBLE: u32 = 0x0045_dd34;
    pub const ELEVEN_BITS: u32 = 0x0045_dd3c;
    pub const BYTE: u32 = 0x0045_dbb4;
    pub const COLOUR_FLOOR: u32 = 0x0045_dbbc;
    pub const ZERO_LENGTH: u32 = 0x0045_dbd8;
    pub const ANGLE_STEP: u32 = 0x0052_59b4;
    pub const TINT_STEP: u32 = 0x0043_fde4;
}

/// The draw's constants, read once per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constants {
    pub one: f32,
    pub origin_bias: f32,
    pub position: f32,
    pub position_xy: f32,
    pub position_z: f32,
    pub nibble: f32,
    pub eleven_bits: f32,
    pub byte: f32,
    pub colour_floor: f32,
    pub zero_length: f32,
    pub angle_step: f32,
    pub tint_step: f32,
}

impl Constants {
    pub fn read(mem: &dyn GuestRead) -> Option<Self> {
        use constant::*;
        let f = |a| mem.read_f32(a);
        Some(Self {
            one: f(ONE)?,
            origin_bias: f(ORIGIN_BIAS)?,
            position: f(POSITION)?,
            position_xy: f(POSITION_XY)?,
            position_z: f(POSITION_Z)?,
            nibble: f(NIBBLE)?,
            eleven_bits: f(ELEVEN_BITS)?,
            byte: f(BYTE)?,
            colour_floor: f(COLOUR_FLOOR)?,
            zero_length: f(ZERO_LENGTH)?,
            angle_step: f(ANGLE_STEP)?,
            tint_step: f(TINT_STEP)?,
        })
    }
}

/// What the draw reads of a permutation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Permutation {
    pub flags: u16,
    pub model: u8,
    pub scale: [f32; 2],
    pub tints: [u32; 2],
    pub weights: [f32; 2],
}

/// What the draw reads of an instance.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct InstanceWords {
    pub packed: u32,
    pub position: u32,
    pub colours: [u32; 2],
    pub normal: u32,
    pub fade: f32,
}

impl InstanceWords {
    /// Its class (bits 0..2) and permutation (bits 3..8).
    pub fn class(&self) -> u32 {
        self.packed & 7
    }
    pub fn permutation(&self) -> u32 {
        (self.packed >> 3) & 0x3f
    }
}

fn round(x: f64) -> f32 {
    x as f32
}

/// `0x0003f220`: the packed position (x and y 10 bits, z 12) in the
/// cluster's frame.
pub fn position(k: &Constants, word: u32) -> [f32; 3] {
    let p = f64::from(k.position);
    [
        round(f64::from(word & 0x3ff) * p * f64::from(k.position_xy)),
        round(f64::from((word >> 10) & 0x3ff) * p * f64::from(k.position_xy)),
        round(f64::from(word >> 20) * p * f64::from(k.position_z)),
    ]
}

/// `2·(n·scale) − 1`, the third component stored once between
/// (`0x0004dcd0`, `0x0004dd70`).
fn signed3(k: &Constants, n: [u32; 3], scale: [f32; 3]) -> [f32; 3] {
    let one = f64::from(k.one);
    let s = |i: usize| f64::from(n[i]) * f64::from(scale[i]);
    let z = f64::from(round(s(2)));
    [round(s(0) + s(0) - one), round(s(1) + s(1) - one), round(z + z - one)]
}

/// `0x0004dd70`: the packed normal (11, 11 and 10 bits).
pub fn normal(k: &Constants, word: u32) -> [f32; 3] {
    signed3(k, [word & 0x7ff, (word >> 11) & 0x7ff, word >> 22], [k.eleven_bits, k.eleven_bits, k.position_xy])
}

/// `0x0004dcd0`: three nibbles (the up axis: the packed word's bits
/// 20..31).
pub fn nibbles(k: &Constants, word: u32) -> [f32; 3] {
    signed3(k, [word & 0xf, (word >> 4) & 0xf, (word >> 8) & 0xf], [k.nibble; 3])
}

/// `0x00131bb0`: an `x8r8g8b8` colour as three floats.
pub fn colour(k: &Constants, word: u32) -> [f32; 3] {
    [((word >> 16) & 0xff) as f32 * k.byte, ((word >> 8) & 0xff) as f32 * k.byte, (word & 0xff) as f32 * k.byte]
}

/// `x` held to `[0, one]` as the draw's compare pairs do (a NaN passes).
fn clamp(x: f32, one: f32) -> f32 {
    if 0.0 > x {
        0.0
    } else if x > one {
        one
    } else {
        x
    }
}

/// `0x0004ca00` and `0x0004ca40`: the instance's two colours, the second
/// tinted by the permutation's tints at the instance's step (bits 13..15),
/// each moved to its floor by the permutation's weight. `[A, B]`; `v15`
/// is their product.
pub fn colours(k: &Constants, p: &Permutation, i: &InstanceWords) -> [[f32; 3]; 2] {
    let t = ((i.packed >> 13) & 7) as f32 * k.tint_step;
    let mut a = colour(k, i.colours[1]);
    let mut b = colour(k, i.colours[0]);
    let (c1, c2) = (clamp(p.weights[0], k.one), clamp(p.weights[1], k.one));
    let (from, to) = (colour(k, p.tints[0]), colour(k, p.tints[1]));
    let u = k.one - t;
    for c in 0..3 {
        let tint = u * from[c] + to[c] * t;
        b[c] = (b[c] * c1 + (k.one - c1)) * tint;
    }
    let floor = (k.one - c2) * k.colour_floor;
    for x in &mut a {
        *x = *x * c2 + floor;
    }
    [a, b]
}

/// `0x0011d000`: an axis perpendicular to `v` (not unit).
pub fn perpendicular(v: [f32; 3]) -> [f32; 3] {
    let (x, y, z) = (v[0].abs(), v[1].abs(), v[2].abs());
    if y >= x && z >= x {
        [0.0, v[2], 0.0 - v[1]]
    } else if z >= y {
        [0.0 - v[2], 0.0, v[0]]
    } else {
        [v[1], 0.0 - v[0], 0.0]
    }
}

/// `0x0004efb0`: `v` unit, unless its squared length is the zero constant.
pub fn normalize(k: &Constants, v: [f32; 3]) -> [f32; 3] {
    let len2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if len2 == k.zero_length {
        return v;
    }
    let inv = 1.0 / f64::from(len2).sqrt();
    v.map(|c| round(inv * f64::from(c)))
}

/// `0x00141ce0`: the basis (forward, left, up) of yaw `a`, pitch `b` and
/// roll `c`, as it computes it.
pub fn euler(a: f32, b: f32, c: f32) -> [[f32; 3]; 3] {
    let (ca, sa) = (round(f64::from(a).cos()), round(f64::from(a).sin()));
    let (cb, sb) = (round(f64::from(b).cos()), round(f64::from(b).sin()));
    let (cc, sc) = (round(f64::from(c).cos()), round(f64::from(c).sin()));
    let forward = [ca * cb, sa * cc - sb * sc * ca, sb * cc * ca + sa * sc];
    let left = [-(sa * cb), sb * sc * sa + ca * cc, ca * sc - sb * cc * sa];
    let up = [0.0 - sb, 0.0 - cb * sc, cb * cc];
    [forward, left, up]
}

/// The instance's basis before its scale: with permutation flag bit 0, its
/// up axis (bits 20..31) made unit, turned about it by its step (bits
/// 9..12); else a yaw by that step.
pub fn basis(k: &Constants, p: &Permutation, packed: u32) -> [[f32; 3]; 3] {
    let angle = f64::from((packed >> 9) & 0xf) * f64::from(k.angle_step) * f64::from(k.nibble);
    if p.flags & 1 == 0 {
        return euler(round(angle), 0.0, 0.0);
    }
    let up = nibbles(k, packed >> 20);
    let f = perpendicular(up);
    let l = [up[1] * f[2] - up[2] * f[1], up[2] * f[0] - up[0] * f[2], up[0] * f[1] - up[1] * f[0]];
    let up = normalize(k, up);
    let (s, c) = (round(angle.sin()), round(angle.cos()));
    let forward = [f[0] * c + l[0] * s, f[1] * c + l[1] * s, f[2] * c + l[2] * s];
    let left = [l[0] * c - f[0] * s, l[1] * c - f[1] * s, l[2] * c - f[2] * s];
    [forward, left, up]
}

/// `v11..v15` of one instance: `(normal, fade)`, the three rows of its
/// scaled basis with its world position, `(A·B, 1)`. `fade` is the set's
/// (base, scale); `origin` the cluster's origin less the bias.
pub fn registers(k: &Constants, fade: [f32; 2], origin: [f32; 3], p: &Permutation, i: &InstanceWords) -> [Row; REGISTERS] {
    let n = normal(k, i.normal);
    let local = position(k, i.position);
    let pos = [local[0] + origin[0], local[1] + origin[1], local[2] + origin[2]];
    let t = round(f64::from((i.packed >> 16) & 0xf) * f64::from(k.nibble));
    let scale = round((f64::from(k.one) - f64::from(t)) * f64::from(p.scale[0]) + f64::from(t) * f64::from(p.scale[1]));
    let alpha = clamp((i.fade - fade[0]) * fade[1], k.one);
    let [a, b] = colours(k, p, i);
    let [f, l, u] = basis(k, p, i.packed);
    let row = |c: usize| [f[c] * scale, l[c] * scale, u[c] * scale, pos[c]];
    [[n[0], n[1], n[2], alpha], row(0), row(1), row(2), [a[0] * b[0], a[1] * b[1], a[2] * b[2], k.one]]
}

/// Field reads at `base + offset`, the address wrapping as the guest's
/// does (a pointer read from guest memory can be anything).
fn word(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Refusal> {
    mem.read_u32(base.wrapping_add(offset)).ok_or(Refusal::Unread)
}

fn float(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<f32, Refusal> {
    word(mem, base, offset).map(f32::from_bits)
}

fn byte(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u8, Refusal> {
    let addr = base.wrapping_add(offset);
    Ok((word(mem, addr & !3, 0)? >> (8 * (addr & 3))) as u8)
}

fn half(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u16, Refusal> {
    let addr = base.wrapping_add(offset);
    if addr & 3 == 3 {
        return Ok(u16::from(byte(mem, addr, 0)?) | u16::from(byte(mem, addr, 1)?) << 8);
    }
    Ok((word(mem, addr & !3, 0)? >> (8 * (addr & 3))) as u16)
}

/// Element `index` of `size` bytes from `base` (`index` signed as the
/// guest's `movsx` makes it).
fn element(base: u32, index: i32, size: u32) -> u32 {
    base.wrapping_add((index as u32).wrapping_mul(size))
}

/// Why a placement's draw is not in the scene (the guest keeps it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Memory that does not read.
    Unread,
    /// The placement's geometry record or the set's geometry is not
    /// resident: the guest's cache request decides.
    NotResident,
}

/// The tag definition of datum `datum`.
pub fn definition(mem: &dyn GuestRead, datum: u32) -> Option<u32> {
    let table = mem.read_u32(TAG_TABLE)?;
    word(mem, element(table, (datum & 0xffff) as i32, 0x10), 8).ok()
}

fn resident(mem: &dyn GuestRead, info: u32) -> Result<bool, Refusal> {
    Ok(byte(mem, info, RESIDENT)? != 0)
}

/// The draw of list item `item` (build [`DECORATOR_BUILD`]) at `slot`,
/// layer `layer`, with its instances: what `0x0004e260` would set and
/// draw. `Ok(None)` for layer 1, where the build takes nothing.
pub fn read(
    mem: &dyn GuestRead, k: &Constants, item: u32, slot: u32, layer: u8,
) -> Result<Option<(DecoratorDraw, Vec<DecoratorInstance>)>, Refusal> {
    if layer == 1 {
        return Ok(None);
    }
    let shader = word(mem, item, 8)?;
    let argument = word(mem, item, 0xc)?;
    let flags = word(mem, item, 0x14)?;
    let (pass, variant) = (flags as u8, (flags >> 8) as u8);
    let block = word(mem, word(mem, ACTIVE_BSP, 0)?, BSP_DECORATORS)?;
    let placement = element(word(mem, block, BLOCK_PLACEMENTS)?, ((argument >> 8) & 0x3f_ffff) as i32, PLACEMENT_SIZE);
    let palette = byte(mem, placement, 0)? as i8;
    let palette_entry = element(word(mem, word(mem, SCENARIO, 0)?, SCENARIO_PALETTE)?, i32::from(palette), 8);
    let set = word(mem, palette_entry, 4)?;
    let def = definition(mem, set).ok_or(Refusal::Unread)?;
    let geometry_index = half(mem, placement, PLACEMENT_GEOMETRY)? as i16;
    let geometry = element(word(mem, block, BLOCK_GEOMETRY)?, i32::from(geometry_index), GEOMETRY_SIZE);
    if !resident(mem, geometry)? || !resident(mem, def.wrapping_add(SET_GEOMETRY))? {
        return Err(Refusal::NotResident);
    }
    let lod = argument >> 30;
    let lod_constant = [float(mem, LOD_FACTORS[0], 4 * lod)?, float(mem, LOD_FACTORS[1], 4 * lod)?, 0.0, k.one];
    let cluster = VISIBLE_CLUSTERS + 0x10 * (argument & 0xff);
    let mut origin = [0.0; 3];
    for (c, o) in origin.iter_mut().enumerate() {
        *o = float(mem, cluster, 4 + 4 * c as u32)? - k.origin_bias;
    }
    let fade = [float(mem, def, SET_FADE)?, float(mem, def, SET_FADE + 4)?];
    let first = half(mem, placement, PLACEMENT_FIRST)? as i16;
    let count = half(mem, placement, PLACEMENT_COUNT)? as i16;
    let mut out = Vec::new();
    if count > 0 {
        let instances = word(mem, geometry, GEOMETRY_INSTANCES)?;
        let available = word(mem, instances, 0)? as i32;
        let base = word(mem, instances, 4)?;
        let classes = word(mem, def, SET_CLASSES)?;
        let models = word(mem, def, SET_MODELS)?;
        let model_count = word(mem, def, SET_MODEL_COUNT)? as i32;
        for n in 0..count as u16 {
            let index = i32::from(first) + i32::from(n);
            if index < 0 || index >= available {
                continue;
            }
            let inst = element(base, index, INSTANCE_SIZE);
            let words = InstanceWords {
                packed: word(mem, inst, 0)?,
                position: word(mem, inst, 4)?,
                colours: [word(mem, inst, 8)?, word(mem, inst, 0xc)?],
                normal: word(mem, inst, 0x10)?,
                fade: float(mem, inst, 0x14)?,
            };
            let class = element(classes, words.class() as i32, CLASS_SIZE);
            let perm = element(word(mem, class, CLASS_PERMUTATIONS)?, words.permutation() as i32, PERMUTATION_SIZE);
            let p = Permutation {
                flags: half(mem, perm, 8)?,
                model: byte(mem, perm, 0xa)?,
                scale: [float(mem, perm, 0xc)?, float(mem, perm, 0x10)?],
                tints: [word(mem, perm, 0x14)?, word(mem, perm, 0x18)?],
                weights: [float(mem, perm, 0x1c)?, float(mem, perm, 0x20)?],
            };
            if i32::from(p.model) >= model_count {
                continue;
            }
            let model = element(models, i32::from(p.model), 8);
            let index_count = half(mem, model, 6)?;
            if index_count == 0 {
                continue;
            }
            out.push(DecoratorInstance {
                index: n,
                model: u16::from(p.model),
                first_index: u32::from(half(mem, model, 4)?),
                index_count: u32::from(index_count),
                registers: registers(k, fade, origin, &p, &words),
            });
        }
    }
    let draw = DecoratorDraw { slot, layer, set, shader, variant, pass, argument, lod_constant, instances: 0..0 };
    Ok(Some((draw, out)))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::Mem;

    /// Made-up constants, powers of two where a test wants exact
    /// results: positions in 1/16 units, nibbles and the eleven-bit
    /// normal in steps that land on 1, bytes in 1/255, a nibble step of
    /// the angle an eighth turn.
    pub(crate) fn constants() -> Constants {
        Constants {
            one: 1.0,
            origin_bias: 0.5,
            position: 1.0,
            position_xy: 1.0 / 16.0,
            position_z: 1.0 / 16.0,
            nibble: 0.125,
            eleven_bits: 1.0 / 1024.0,
            byte: 1.0 / 255.0,
            colour_floor: 0.5,
            zero_length: 0.0,
            angle_step: std::f32::consts::PI,
            tint_step: 1.0 / 7.0,
        }
    }

    pub(crate) fn put_constants(m: &mut Mem, k: &Constants) {
        use constant::*;
        for (a, v) in [
            (ONE, k.one),
            (ORIGIN_BIAS, k.origin_bias),
            (POSITION, k.position),
            (POSITION_XY, k.position_xy),
            (POSITION_Z, k.position_z),
            (NIBBLE, k.nibble),
            (ELEVEN_BITS, k.eleven_bits),
            (BYTE, k.byte),
            (COLOUR_FLOOR, k.colour_floor),
            (ZERO_LENGTH, k.zero_length),
            (ANGLE_STEP, k.angle_step),
            (TINT_STEP, k.tint_step),
        ] {
            m.f(a, v);
        }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn the_constants_read_back_from_memory() {
        let mut m = Mem::default();
        assert_eq!(Constants::read(&m), None);
        put_constants(&mut m, &constants());
        assert_eq!(Constants::read(&m), Some(constants()));
    }

    #[test]
    fn packed_positions_normals_and_colours_unpack_as_the_helpers_do() {
        let k = constants();
        assert_eq!(position(&k, 16 | 32 << 10 | 4095 << 20), [1.0, 2.0, 4095.0 / 16.0]);
        // 2 · 1024/1024 − 1 = 1; 512 is 0; z 0 is −1.
        assert_eq!(normal(&k, 1024 | 512 << 11), [1.0, 0.0, -1.0]);
        assert_eq!(nibbles(&k, 8 | 0 << 4 | 8 << 8), [1.0, -1.0, 1.0]);
        assert_eq!(colour(&k, 0x00ff_0033), [255.0 * k.byte, 0.0, 51.0 * k.byte]);
    }

    #[test]
    fn the_perpendicular_avoids_the_largest_axis_and_euler_yaw_turns_about_z() {
        assert_eq!(perpendicular([0.0, 0.0, 1.0]), [0.0, 1.0, -0.0]);
        assert_eq!(perpendicular([1.0, 0.0, 0.2]), [-0.2, 0.0, 1.0], "y smallest");
        assert_eq!(perpendicular([1.0, 0.5, 0.0]), [0.5, -1.0, 0.0], "z smallest");
        let [f, l, u] = euler(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        assert!(close(f[0], 0.0) && close(f[1], 1.0) && f[2] == 0.0);
        assert!(close(l[0], -1.0) && close(l[1], 0.0) && l[2] == 0.0);
        assert_eq!(u, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn an_instances_registers_are_its_normal_fade_scaled_rows_position_and_colour() {
        let k = constants();
        let p = Permutation {
            flags: 0,
            model: 0,
            scale: [1.0, 3.0],
            tints: [0x00ff_ffff, 0x00ff_ffff],
            weights: [1.0, 1.0],
        };
        // Step 4 (a quarter turn), scale nibble 8 (t = 1: the maximum).
        let words = InstanceWords {
            packed: 4 << 9 | 8 << 16,
            position: 16 | 32 << 10 | 48 << 20,
            colours: [0x00ff_8000, 0x00ff_ffff],
            normal: 1023 | 1023 << 11 | 1023 << 22,
            fade: 3.0,
        };
        let r = registers(&k, [1.0, 0.25], [10.0, 20.0, 30.0], &p, &words);
        assert_eq!(r[0][3], 0.5, "(3 − 1) · 0.25");
        // Rows: forward (0, 1, 0), left (−1, 0, 0), up (0, 0, 1), all · 3.
        assert!(close(r[1][0], 0.0) && close(r[1][1], -3.0) && close(r[1][2], 0.0));
        assert!(close(r[2][0], 3.0) && close(r[2][1], 0.0));
        assert_eq!(r[3][2], 3.0);
        assert_eq!([r[1][3], r[2][3], r[3][3]], [11.0, 22.0, 33.0]);
        // A white, B (1, 128/255, 0) under a white tint: v15 is B.
        assert_eq!(r[4], [255.0 * k.byte, 128.0 * k.byte, 0.0, 1.0]);
        // The fade clamps to [0, 1].
        let far = registers(&k, [1.0, 0.25], [0.0; 3], &p, &InstanceWords { fade: 100.0, ..words });
        assert_eq!(far[0][3], 1.0);
        let near = registers(&k, [1.0, 0.25], [0.0; 3], &p, &InstanceWords { fade: 0.0, ..words });
        assert_eq!(near[0][3], 0.0);
    }

    #[test]
    fn weights_move_the_colours_to_their_floors_and_the_tint_steps_between_two() {
        let k = constants();
        let p = Permutation { tints: [0x00ff_0000, 0x0000_00ff], weights: [0.0, 0.0], ..Default::default() };
        // Weights 0: B is the tint alone, A the floor (0.5).
        let i = InstanceWords { packed: 7 << 13, colours: [0, 0], ..Default::default() };
        let [a, b] = colours(&k, &p, &i);
        assert_eq!(a, [0.5; 3]);
        assert_eq!(b, [0.0, 0.0, 1.0], "step 7 of 7: the second tint");
        let i = InstanceWords { packed: 0, ..i };
        assert_eq!(colours(&k, &p, &i)[1], [1.0, 0.0, 0.0], "step 0: the first");
    }

    #[test]
    fn an_up_axis_permutation_turns_about_the_instances_own_up() {
        let k = constants();
        let p = Permutation { flags: 1, scale: [2.0, 2.0], ..Default::default() };
        // Up nibbles (8, 8, 8): up (1, 1, 1), unit after the cross product.
        let packed = 0x888 << 20;
        let [f, l, u] = basis(&k, &p, packed);
        let s = 1.0 / 3f32.sqrt();
        assert!(u.iter().all(|&c| close(c, s)), "{u:?}");
        // Step 0: forward is the perpendicular (0, 1, −1), left up × it.
        assert_eq!(f, [0.0, 1.0, -1.0]);
        assert_eq!(l, [1.0 * -1.0 - 1.0 * 1.0, 1.0 * 0.0 - 1.0 * -1.0, 1.0 * 1.0 - 1.0 * 0.0]);
    }
}
