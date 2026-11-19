//! Halo 1 particles and contrails as Halo 2 `prt3` and `cont`, written
//! from the definitions (`crates/tags/src/defs/prt3.txt`, `cont.txt`).
//!
//! **Contrails.** Halo 2's `cont` is Halo 1's with its two tag references
//! shortened (16 bytes to 8) and the padding between the root's halves
//! dropped: V, word for word on the contrails both games ship (the sniper
//! rifle's, the needler's, the plasma rifle's; `tests/particles_disc.rs`
//! `contrail_survey`). So a converted contrail is Halo 1's spans at Halo
//! 2's offsets ([`CONT_SPANS`], [`CONT_POINT_SPANS`]), its bitmap converted
//! (`h1bitmaps`). The points' physics (`pphy`) has no converter here: it is
//! null, as on Halo 2's plasma streaks (V).

use h1map::{H1Map, Tag, TagId as H1TagId};
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};
use tags::effects::function::{Function, FunctionType};
use tags::effects::h1_attachment::{
    DIRECTIONAL_BIRTH, POPULATION_CLOCK, RADIAL_BIRTH, population_word,
};

use crate::h1::{reference, tag_bytes};
use crate::{Error, Result};

/// Halo 1 `cont` root size (V: every one is `0x144` plus `0x68` per point,
/// its point block at `+0x138`).
pub const H1_CONT_SIZE: usize = 0x144;
/// Halo 1 contrail point size.
pub const H1_CONT_POINT_SIZE: usize = 0x68;
/// Halo 1 `cont +0x138`: the points.
pub const H1_CONT_POINTS: usize = 0x138;

/// (Halo 2 field, Halo 1 offset, length) of a contrail root (`cont.txt`).
pub const CONT_SPANS: &[(&str, usize, usize)] = &[
    ("unknown_4", 0x4, 0x2c),
    ("unknown_38", 0x40, 0x4),
    ("unknown_60", 0xa8, 0x28),
    ("unknown_90", 0xe0, 0x58),
];
/// The same for a point.
pub const CONT_POINT_SPANS: &[(&str, usize, usize)] = &[("unknown_0", 0x0, 0x10), ("unknown_18", 0x40, 0x28)];

/// A Halo 1 contrail: its root and points' bytes and its bitmap.
#[derive(Clone, Debug, PartialEq)]
pub struct H1Contrail {
    pub flags: u32,
    pub root: Vec<u8>,
    pub bitmap: Option<H1TagId>,
    pub points: Vec<Vec<u8>>,
}

impl H1Contrail {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"cont", H1_CONT_SIZE)?;
        let blk = h1map::Block::read(b, H1_CONT_POINTS).ok_or(Error::Truncated("cont points"))?;
        let points = map
            .block(blk, H1_CONT_POINT_SIZE)?
            .chunks_exact(H1_CONT_POINT_SIZE)
            .map(<[u8]>::to_vec)
            .collect();
        Ok(Self {
            flags: u32::from_le_bytes(b[0..4].try_into().expect("4")),
            root: b[..H1_CONT_SIZE].to_vec(),
            bitmap: reference(map, b, 0x30),
            points,
        })
    }
}

fn def(group: &[u8; 4]) -> Result<&'static StructDef> {
    Defs::builtin()
        .group(FourCC(*group))
        .ok_or_else(|| Error::Invalid(format!("no definition for {}", FourCC(*group))))
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v).map_err(|e| Error::Invalid(e.to_string()))
}

fn spans(r: &mut Record, from: &[u8], list: &[(&str, usize, usize)]) -> Result<()> {
    for (field, at, len) in list {
        let bytes = from
            .get(*at..at + len)
            .ok_or(Error::Truncated("contrail span"))?;
        set(r, field, Val::Bytes(bytes.to_vec()))?;
    }
    Ok(())
}

/// The `cont` root for `c`, its bitmap `bitmap` (converted, or null).
pub fn contrail(c: &H1Contrail, bitmap: Option<TagId>) -> Result<Block> {
    let d = def(b"cont")?;
    let mut r = defs::default_record(d);
    set(&mut r, "flags", Val::Int(c.flags.into()))?;
    spans(&mut r, &c.root, CONT_SPANS)?;
    set(&mut r, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap.unwrap_or(TagId::NULL)))?;
    let pd = d.child("points").expect("cont points");
    let points = c
        .points
        .iter()
        .map(|p| {
            let mut x = defs::default_record(pd);
            spans(&mut x, p, CONT_POINT_SPANS)?;
            Ok(x)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut r, "points", Val::Block(points))?;
    defs::to_built(d, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

// ------------------------------------------------------------ particle shader
//
// **The compiled particle shader.** A `prt3`'s `postprocess` (`+0x8c`) is
// what Halo 2's tag compiler makes of its shader template (`stem`) and its
// parameters; the draw path reads only this (`prt3.txt`). Retail particles
// with no animated parameter follow one rule from their stem (V, every one
// on the disc, `tests/particles_disc.rs`
// `retail_particle_shaders_compile_from_their_stems`):
//
// - one `bitmaps`, `colors` and `vectors` element per instance source the
//   stem's bindings name (the binding's `source` byte, through each
//   implementation's bitmap, pixel-constant and vertex-constant ranges);
// - `lods`, `layers` and `passes` mirror the stem's: a lod's layer range,
//   a layer's pass range and a pass's implementation range are the
//   stem's, the lod's layer mask (layers with animation data) zero;
// - one `implementations` element per stem implementation, its animation
//   ranges empty; no properties, property references or animations;
// - a bitmap's `lod_offset` is `log2(max(width, height))` of its first
//   image times `ln(2)^2`, its image 0;
// - `parameter_bitmaps` is five entries, the first naming bitmap 0, the
//   rest `-1`; `parameter_colors` two zero colours, `parameter_scalars`
//   six zeros.
//
// The texture transform is identity (unit scale, zero offset). The second
// vertex default is the homogeneous origin. The blend's pixel constants
// start at zero; the emitter supplies the particle colours.
/// One stem's compiled-shader role on a base: the stem, its compiled
/// postprocess and the colour and vector values its particles hold.
#[derive(Clone, Debug)]
pub struct ParticleShader {
    pub name: String,
    pub stem: TagId,
    pub template: tags::shader::TemplatePostprocess,
    pub colors: Vec<u32>,
    pub vectors: Vec<[f32; 4]>,
}

/// The instance elements the stem's bindings read: (bitmaps, colors,
/// vectors), each one past the highest `source` its ranges name.
pub fn source_counts(t: &tags::shader::TemplatePostprocess) -> (usize, usize, usize) {
    let mut n = [0usize; 3];
    for i in &t.implementations {
        for (k, r) in [i.bitmaps, i.pixel_constants, i.vertex_constants].into_iter().enumerate() {
            for b in r.indices() {
                if let Some(b) = t.bindings.get(b) {
                    n[k] = n[k].max(usize::from(b.source) + 1);
                }
            }
        }
    }
    (n[0], n[1], n[2])
}

/// A bitmap parameter's `lod_offset` for a bitmap whose first image is
/// `width` by `height`.
pub fn lod_offset(width: u16, height: u16) -> f32 {
    let ln2 = std::f32::consts::LN_2;
    f32::from(width.max(height).max(1)).log2() * ln2 * ln2
}

/// Read the compiled particle stems directly. Conversion only needs a
/// stem's bindings; a base can supply a stem without any particle on it.
pub fn particle_shaders(base: &crate::base::Base) -> Result<Vec<ParticleShader>> {
    let meta = tags::shader::Metadata::new(base.meta.base, base.meta.bytes.clone());
    let mut out = Vec::new();
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"stem")
        && t.2.rsplit('\\').next().is_some_and(|name| name.starts_with("particle_"))) {
        let template = tags::shader::ShaderTemplate::decode(&meta, t.3)
            .map_err(|e| Error::Invalid(format!("{}: {e}", t.2)))?
            .compiled()
            .map_err(|e| Error::Invalid(format!("{}: {e}", t.2)))?
            .clone();
        let (_, colors, count) = source_counts(&template);
        let mut vectors = vec![[0.0; 4]; count];
        if let Some(uv) = vectors.first_mut() {
            *uv = [1.0, 1.0, 0.0, 0.0];
        }
        if let Some(origin) = vectors.get_mut(1) {
            *origin = [0.0, 0.0, 0.0, 1.0];
        }
        out.push(ParticleShader { name: t.2.clone(), stem: t.0, template, colors: vec![0; colors], vectors });
    }
    Ok(out)
}

/// The compiled shader (`prt3` `postprocess` element) of a particle on
/// `s` whose one bitmap parameter is `bitmap` (its tag and first image's
/// size), compiled by the rule above.
pub fn particle_postprocess(s: &ParticleShader, bitmap: Option<(TagId, u16, u16)>) -> Result<Record> {
    let d = def(b"prt3")?.child("postprocess").expect("prt3 postprocess");
    let t = &s.template;
    let (bitmaps, colors, vectors) = source_counts(t);
    let mut r = defs::default_record(d);
    let datum = |id: Option<TagId>| Val::Datum(id.map_or(defs::Target::Null, defs::Target::Id));
    set(&mut r, "template", datum(Some(s.stem)))?;
    let el = |name: &str| d.child(name).expect("postprocess child");
    let mut list = Vec::new();
    for k in 0..bitmaps {
        let mut x = defs::default_record(el("bitmaps"));
        let b = bitmap.filter(|_| k == 0);
        set(&mut x, "bitmap", datum(b.map(|b| b.0)))?;
        set(&mut x, "image", Val::Int(0))?;
        set(&mut x, "lod_offset", Val::Float(b.map_or(0.0, |b| lod_offset(b.1, b.2))))?;
        list.push(x);
    }
    set(&mut r, "bitmaps", Val::Block(list))?;
    let mut list = Vec::new();
    for k in 0..colors {
        let mut x = defs::default_record(el("colors"));
        set(&mut x, "color", Val::Int(s.colors.get(k).copied().unwrap_or(0).into()))?;
        list.push(x);
    }
    set(&mut r, "colors", Val::Block(list))?;
    let mut list = Vec::new();
    for k in 0..vectors {
        let mut x = defs::default_record(el("vectors"));
        set(&mut x, "vector", Val::floats(&s.vectors.get(k).copied().unwrap_or([0.0; 4])))?;
        list.push(x);
    }
    set(&mut r, "vectors", Val::Block(list))?;
    let mut list = Vec::new();
    for l in &t.lods {
        let mut x = defs::default_record(el("lods"));
        set(&mut x, "layer_mask", Val::Int(0))?;
        set(&mut x, "layers", Val::Int(l.layers.encode().into()))?;
        list.push(x);
    }
    set(&mut r, "lods", Val::Block(list))?;
    let mut list = Vec::new();
    for l in &t.layers {
        let mut x = defs::default_record(el("layers"));
        set(&mut x, "passes", Val::Int(l.encode().into()))?;
        list.push(x);
    }
    set(&mut r, "layers", Val::Block(list))?;
    let mut list = Vec::new();
    for p in &t.passes {
        let mut x = defs::default_record(el("passes"));
        set(&mut x, "implementations", Val::Int(p.implementations.encode().into()))?;
        list.push(x);
    }
    set(&mut r, "passes", Val::Block(list))?;
    let mut list = Vec::new();
    for _ in &t.implementations {
        let mut x = defs::default_record(el("implementations"));
        set(&mut x, "stage_and_pixel_constant_animations", Val::Raw(defs::RawVal::Word(0)))?;
        list.push(x);
    }
    set(&mut r, "implementations", Val::Block(list))?;
    let mut list = Vec::new();
    for k in 0..5 {
        let mut x = defs::default_record(el("parameter_bitmaps"));
        let named = k == 0 && bitmaps > 0;
        set(&mut x, "bitmap_index", Val::Int(if named { 0 } else { -1 }))?;
        set(&mut x, "unknown_2", Val::Bytes(vec![0xff, 0xff]))?;
        list.push(x);
    }
    set(&mut r, "parameter_bitmaps", Val::Block(list))?;
    let list = (0..2).map(|_| defs::default_record(el("parameter_colors"))).collect();
    set(&mut r, "parameter_colors", Val::Block(list))?;
    let list = (0..6).map(|_| defs::default_record(el("parameter_scalars"))).collect();
    set(&mut r, "parameter_scalars", Val::Block(list))?;
    Ok(r)
}

// ------------------------------------------------------------- particles
//
// **Particles.** A Halo 1 `part` (root `0x164`; `tests/particles_disc.rs`
// `halo1_layout_survey`, `particle_root_survey`) becomes a `prt3` on the
// shader template its framebuffer blend function names: the `u16` at
// `+0xda` holds 0, 3, 6 or 7 (V), and the particles Halo 2 ships again
// pair them with Halo 2's stems (V: smoke, blood and debris are 0 and
// alpha blend in Halo 2; flashes and sparks 3 and additive; ice 7 and
// alpha-multiply-add, `particle_alpha_multiply_add` holding only
// `solids\ice`). Its bitmap (`+0x4`) is the stem's one bitmap parameter
// (`alpha_map_texture`, the name every particle stem's parameter has on
// the disc), its sprite sequences Halo 1's (`+0x98` first, `+0x9a`
// initial and `+0x9c` looping counts: one random sequence per particle
// among them, prt3 flag `0x10`), its collision and death effects
// (`+0x48`, `+0x58`) converted. The five properties are constants as the
// base's particle evaluator reads them: white, opaque,
// unit size (the emitter scales it), no rotation, the first sprite.
//
// **The frame property is never empty.** Many of the base's static
// particles leave `frame` (`+0x5c`) with no function data: they draw the
// static quad (`flags_2` bit 0, E `0x002246e0`), a path that never reads
// it. A converted particle draws its bitmap's sprites (`flags_2` 0), and
// that path (E `0x00224846`: a sprites bitmap with sprites in the chosen
// sequence) evaluates `frame` through the property wrapper `0x00246cd0`,
// which reads the function's header byte 1 (`test byte [edi+1], 0xf0`
// at `0x00246cfc`) with no check for a null pointer. An empty `frame`
// faults reading address 1 the moment such a particle is drawn (Blood
// Gulch's grenade effects, P: the fault's property was a converted prt3's
// `frame`, data count 0 and address 0). So an empty or unparseable frame
// is written as [`first_sprite_frame`]: a constant with range `[0, 0]`,
// the first sprite of the particle's sequence.

/// Halo 1 `part` root size (V: all 143 distinct tags).
pub const H1_PART_SIZE: usize = 0x164;

/// A Halo 1 particle, the fields converted.
#[derive(Clone, Debug, PartialEq)]
pub struct H1Particle {
    pub flags: u32,
    /// `+0x4` bitmap.
    pub bitmap: Option<H1TagId>,
    /// `+0x48` collision effect (`effe` or `snd!`), `+0x58` death effect.
    pub collision_effect: Option<H1TagId>,
    pub death_effect: Option<H1TagId>,
    /// `+0x98`, `+0x9a`, `+0x9c`, `+0x9e`: first sequence, then the
    /// initial, looping and final sequence counts.
    pub sequences: [u16; 4],
    /// `+0xda` framebuffer blend function.
    pub blend: u16,
    /// `+0x38` lifespan bounds, seconds.
    pub lifespan: [f32; 2],
}

impl H1Particle {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"part", H1_PART_SIZE)?;
        let h = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let f = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().expect("4"));
        Ok(Self {
            flags: u32::from_le_bytes(b[0..4].try_into().expect("4")),
            bitmap: reference(map, b, 0x4),
            collision_effect: reference(map, b, 0x48),
            death_effect: reference(map, b, 0x58),
            sequences: [h(0x98), h(0x9a), h(0x9c), h(0x9e)],
            blend: h(0xda),
            lifespan: [f(0x38), f(0x3c)],
        })
    }

    /// The sequences a particle picks from: (first, count).
    pub fn sequence_range(&self) -> (u16, u16) {
        (self.sequences[0], (self.sequences[1] + self.sequences[2]).max(1))
    }
}

/// The Halo 2 stem for a Halo 1 blend function, and whether the pairing
/// is proven (V for 0, 3, 7; 6 is drawn additive, unproven).
pub fn particle_stem(blend: u16) -> (&'static str, bool) {
    match blend {
        0 => ("shaders\\shader_templates\\transparent\\particle_alpha_blend", true),
        3 => ("shaders\\shader_templates\\transparent\\particle_additive", true),
        7 => ("shaders\\shader_templates\\transparent\\particle_alpha_multiply_add", true),
        _ => ("shaders\\shader_templates\\transparent\\particle_additive", false),
    }
}

/// The function blob of a frame property that always picks the first
/// sprite: a constant (type 1, unranged, so the evaluator gives 0) mapped
/// onto the range `[0, 0]`; one set of one word, `0x1c` bytes.
pub fn first_sprite_frame() -> Vec<u8> {
    use tags::effects::function::{Function, FunctionType};
    let kind = FunctionType::Constant;
    Function {
        kind,
        flags: 0,
        sub_types: [0, 0],
        range: [0.0, 0.0],
        unknown_0c: [0; 8],
        payload: vec![0; kind.blob_len().expect("constant has a length") - tags::effects::function::HEADER_LEN],
    }
    .to_bytes()
}

/// Writes [`first_sprite_frame`] into a `prt3` record's `frame` function
/// when it holds no function the engine can read (empty, or not one).
pub fn sprite_frame(r: &mut Record) -> Result<()> {
    let readable = matches!(r.at("frame.function"), Some(Val::Data(b))
        if tags::effects::function::Function::parse(b).is_ok());
    if !readable {
        set(r, "frame.function", Val::Data(first_sprite_frame()))?;
    }
    Ok(())
}

/// The `prt3` root of a Halo 1 particle on `shader`: its bitmap (converted
/// tag and first image's size), its collision and death effects
/// (converted, with their groups).
pub fn particle(
    p: &H1Particle,
    shader: &ParticleShader,
    bitmap: Option<(TagId, u16, u16)>,
    collision_effect: Option<(FourCC, TagId)>,
    death_effect: Option<(FourCC, TagId)>,
) -> Result<Block> {
    let d = def(b"prt3")?;
    let mut r = defs::default_record(d);
    let (first, count) = p.sequence_range();
    set(&mut r, "flags", Val::Int(0x10))?;
    set(&mut r, "flags_2", Val::Int(0))?;
    set(&mut r, "first_sequence", Val::Int(first.into()))?;
    set(&mut r, "sequence_count", Val::Int(count.into()))?;
    set(&mut r, "shader_template", Val::tag(FourCC(*b"stem"), shader.stem))?;
    let pd = d.child("parameters").expect("prt3 parameters");
    let mut x = defs::default_record(pd);
    set(&mut x, "name", Val::text("alpha_map_texture"))?;
    set(&mut x, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap.map_or(TagId::NULL, |b| b.0)))?;
    set(&mut r, "parameters", Val::Block(vec![x]))?;
    // 0x2245ad..0x224caf multiplies these three particle properties by
    // emitter tint, alpha and size. Their neutral values preserve the H1 emitter.
    for (field, scalar, colour) in [
        ("color", 1.0, Some(u32::MAX)), ("alpha", 1.0, None),
        ("size", 1.0, None), ("rotation", 0.0, None), ("frame", 0.0, None),
    ] {
        constant_property(&mut r, field, scalar, colour)?;
    }
    // Input index zero is used by each constant (0x173ba0 builds this mask).
    set(&mut r, "input_mask", Val::Int(1))?;
    set(&mut r, "quad", Val::floats(&[-0.5, -0.5, 0.5, 0.5]))?;
    set(&mut r, "uv_window", Val::floats(&[0.0, 1.0, 0.0, 1.0]))?;
    for (f, e) in [("collision_effect", collision_effect), ("death_effect", death_effect)] {
        let v = match e {
            Some((g, id)) => Val::tag(g, id),
            None => Val::tag(FourCC(*b"effe"), TagId::NULL),
        };
        set(&mut r, f, v)?;
    }
    set(&mut r, "postprocess", Val::Block(vec![particle_postprocess(shader, bitmap)?]))?;
    defs::to_built(d, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

// ------------------------------------------------------ effect particles
//
// **Effect particles as particle systems.** A Halo 1 effect event's
// particles (`+0x38`, `0xe8` each; `tests/particles_disc.rs`
// `halo1_effect_particle_survey`, V) become the event's Halo 2 particle
// systems, one emitter each, on the engine's emitter slots (E,
// docs/formats/effects.md "Emitter"):
//
// - `+0x8` spawn rate: the engine creates `rate x 30 x dt` particles per
//   step, `dt` the event's step in seconds (the game tick's length,
//   `[0x510c54]+4`, cut to what is left of the event), so an event of
//   duration `d` spawns `rate x 30 x d`. Halo 1's count (`+0x6c`, `+0x6e`
//   bounds) is created at the event's start; the rate is the count over
//   `30 x d`, the event's duration floored at one tick (`1/30`, retail's
//   most common duration; an event with converted particles and a shorter
//   duration is given one tick).
// - `+0x18` lifespan: the particle's (`part +0x38` bounds).
// - `+0x28` initial speed: Halo 1's velocity bounds (`+0x84`), world units
//   per second in both.
// - `+0x78` shape 0, its cone (`+0x8c`, degrees) Halo 1's velocity cone
//   (`+0x8c`, radians).
// - `+0x48` size: twice Halo 1's radius (`+0xa0` bounds), the sprite quad
//   of the static particles spanning one unit (unproven: the quad's corner
//   order is open).
// - `+0x58` tint and `+0x68` alpha: Halo 1's tint bounds (`+0xb0`, `+0xc0`,
//   alpha then RGB) as a packed colour and a scalar.
//
// Bounds become their midpoint. The constant property encoding is built
// directly. Optional system flags and distance controls keep the defaults.

/// Halo 1 effect particle size.
pub const H1_EFFECT_PARTICLE: usize = 0xe8;

/// A Halo 1 effect particle (`0xe8`), the fields converted.
#[derive(Clone, Debug, PartialEq)]
pub struct H1EffectParticle {
    /// `+0x8` index into the effect's locations.
    pub location: i16,
    /// `+0x54` the `part`.
    pub particle: Option<H1TagId>,
    /// `+0x6c`, `+0x6e` count bounds.
    pub count: [i16; 2],
    /// `+0x84` velocity bounds (world units per second), `+0x8c` cone
    /// angle (radians).
    pub velocity: [f32; 2],
    pub velocity_cone: f32,
    /// `+0xa0` radius bounds.
    pub radius: [f32; 2],
    /// `+0xb0`, `+0xc0` tint bounds, (alpha, red, green, blue).
    pub tint: [[f32; 4]; 2],
}

/// Per event of a Halo 1 effect, its particles, with the event's
/// duration bounds.
pub fn effect_particles(map: &H1Map, tag: &Tag) -> Result<Vec<([f32; 2], Vec<H1EffectParticle>)>> {
    let b = tag_bytes(map, tag, b"effe", crate::h1::EFFE_SIZE)?;
    let mut out = Vec::new();
    let Some(ev) = h1map::Block::read(b, 0x34) else { return Ok(out) };
    let evs = map.block(ev, crate::h1::EFFE_EVENT)?.to_vec();
    let f = |x: &[u8], o: usize| f32::from_le_bytes(x[o..o + 4].try_into().expect("4"));
    let i = |x: &[u8], o: usize| i16::from_le_bytes([x[o], x[o + 1]]);
    for e in evs.chunks_exact(crate::h1::EFFE_EVENT) {
        let mut list = Vec::new();
        if let Some(pb) = h1map::Block::read(e, 0x38) {
            for p in map.block(pb, H1_EFFECT_PARTICLE)?.chunks_exact(H1_EFFECT_PARTICLE) {
                let argb = |o: usize| [f(p, o), f(p, o + 4), f(p, o + 8), f(p, o + 0xc)];
                list.push(H1EffectParticle {
                    location: i(p, 0x8),
                    particle: reference(map, p, 0x54),
                    count: [i(p, 0x6c), i(p, 0x6e)],
                    velocity: [f(p, 0x84), f(p, 0x88)],
                    velocity_cone: f(p, 0x8c),
                    radius: [f(p, 0xa0), f(p, 0xa4)],
                    tint: [argb(0xb0), argb(0xc0)],
                });
            }
        }
        out.push(([f(e, 0x10), f(e, 0x14)], list));
    }
    Ok(out)
}

fn mid(b: [f32; 2]) -> f32 {
    (b[0] + b[1]) * 0.5
}

/// One tick, seconds.
pub const TICK: f32 = 1.0 / 30.0;

/// The rate (particles per tick, the engine's `+0x8` unit) that spawns
/// `count` over an event of `duration` seconds (floored at one tick).
pub fn spawn_rate(count: [i16; 2], duration: f32) -> f32 {
    let n = (f32::from(count[0].max(0)) + f32::from(count[1].max(count[0]).max(0))) * 0.5;
    n / (30.0 * duration.max(TICK))
}

/// Encode a constant directly in the property definition. The scalar
/// evaluator (0x246cd0) maps to its two equal endpoints. The colour
/// evaluator (0x246d80) reads packed ARGB at +4 with flag 0x10.
fn constant_property(r: &mut Record, field: &str, scalar: f32, colour: Option<u32>) -> Result<()> {
    set(r, &format!("{field}.function"), Val::Data(constant_data(scalar, colour)))
}

fn constant_data(scalar: f32, colour: Option<u32>) -> Vec<u8> {
    let mut data = first_sprite_frame();
    if let Some(argb) = colour {
        data[1] = 0x10;
        data[4..8].copy_from_slice(&argb.to_le_bytes());
        data[8..12].copy_from_slice(&1.0f32.to_le_bytes());
    } else {
        data[4..8].copy_from_slice(&scalar.to_le_bytes());
        data[8..12].copy_from_slice(&scalar.to_le_bytes());
    }
    data
}

/// Halo 1's tint midpoint as a packed colour (alpha 0xff, the alpha is the
/// emitter's own property).
pub fn packed_tint(t: &[[f32; 4]; 2]) -> u32 {
    let c = |k: usize| (((t[0][k] + t[1][k]) * 0.5).clamp(0.0, 1.0) * 255.0).round() as u32;
    0xff00_0000 | c(1) << 16 | c(2) << 8 | c(3)
}

/// The `effe` particle-system record (one emitter) of a Halo 1 effect
/// particle `p` whose `part` converted to `prt3` (`particle`), in an event
/// of `duration` seconds.
pub fn particle_system(
    p: &H1EffectParticle,
    part: &H1Particle,
    particle: TagId,
    duration: f32,
) -> Result<Record> {
    particle_system_at_rate(
        p,
        part,
        particle,
        [spawn_rate(p.count, duration); 2],
        None,
        None,
        H1ParticleBirth::default(),
    )
}

/// Emitter rate in particles per tick, optionally multiplied by the
/// owning effect's primary object function.
fn particle_system_at_rate(
    p: &H1EffectParticle,
    part: &H1Particle,
    particle: TagId,
    rate: [f32; 2],
    scale_rate: Option<bool>,
    movement: Option<TagId>,
    birth: H1ParticleBirth,
) -> Result<Record> {
    let sd = def(b"effe")?.child("events.particle_systems").expect("effe particle systems");
    let ed = sd.child("emitters").expect("emitters");
    let mut s = defs::default_record(sd);
    set(&mut s, "particle", Val::tag(FourCC(*b"prt3"), particle))?;
    set(&mut s, "location", Val::Int(p.location.into()))?;
    if scale_rate.is_some() {
        // 0x00247beb uses a zero word for world-space particles. The
        // object velocity is added in that branch (0x00247da9).
        set(&mut s, "unknown_c", Val::Int(0))?;
    }
    let mut e = defs::default_record(ed);
    set(
        &mut e,
        "movement",
        Val::tag(FourCC(*b"pmov"), movement.unwrap_or(TagId::NULL)),
    )?;
    set(&mut e, "emission_shape", Val::Int(0))?;
    let alpha = mid([p.tint[0][0], p.tint[1][0]]);
    let props: [(&str, f32, Option<u32>); 9] = [
        ("property_8", rate[0], None),
        ("lifespan", mid(part.lifespan), None),
        ("property_28", mid(p.velocity), None),
        ("property_38", 0.0, None),
        ("property_48", 2.0 * mid(p.radius), None),
        ("property_58", 0.0, Some(packed_tint(&p.tint))),
        ("property_68", alpha, None),
        ("shape_input", 0.0, None),
        ("velocity_input", p.velocity_cone.to_degrees(), None),
    ];
    for (f, v, c) in props {
        let mut property = defs::default_record(ed.child(f).expect("emitter property"));
        set(&mut property, "function", Val::Data(constant_data(v, c)))?;
        if f == "property_8"
            && let Some(scale) = scale_rate
        {
            set(
                &mut property,
                "output_modifier",
                Val::Int(if scale { 2 } else { 0 }),
            )?;
            // E 0x00248eac reads effect +0x6c (selector 9); 0x001795e3
            // updates that scalar from the attachment's primary function.
            set(
                &mut property,
                "modifier_input",
                Val::Int(if scale { 9 } else { -1 }),
            )?;
            if rate[0] != rate[1] {
                // Selector 4 reads the system's normalised event time
                // (0x00248e65, assigned at 0x001752f6).
                let ramp = Function {
                    kind: FunctionType::Identity,
                    flags: 0,
                    sub_types: [0; 2],
                    range: rate,
                    unknown_0c: [0; 8],
                    payload: Vec::new(),
                };
                set(&mut property, "input", Val::Int(4))?;
                set(&mut property, "function", Val::Data(ramp.to_bytes()))?;
            }
        }
        if f == "property_28"
            && let Some(words) = birth.words()
        {
            let mut constant = Function {
                kind: FunctionType::Constant,
                flags: 0,
                sub_types: [0; 2],
                range: [0.0; 2],
                unknown_0c: [0; 8],
                payload: vec![0; 8],
            }
            .to_bytes();
            for (bytes, word) in constant[12..].chunks_exact_mut(4).zip(words) {
                bytes.copy_from_slice(&word.to_le_bytes());
            }
            set(&mut property, "output_modifier", Val::Int(0))?;
            set(&mut property, "modifier_input", Val::Int(-1))?;
            set(&mut property, "function", Val::Data(constant))?;
        }
        set(&mut e, f, Val::Struct(property))?;
    }
    set(&mut s, "emitters", Val::Block(vec![e]))?;
    Ok(s)
}

// ----------------------------------------------------- particle systems
//
// **Halo 1 particle systems (`pctl`).** V over all 28 distinct tags
// (`tests/particles_disc.rs` `halo1_pctl_structure_survey`): a `0x68`
// root (`+0x38` point physics, `+0x48` update physics, `+0x50` a block of
// 4-byte constants, `+0x5c` particle types of `0x80`). A type: `+0x0`
// name, `+0x24` initial particle count (u16), `+0x2c` radius, `+0x54`
// creation physics (0, 1 or 2) with `+0x5c` its 4-byte constants, `+0x68`
// system states (`0xc0`) and `+0x74` particle states (`0x178`). A
// particle state: `+0x0` name, `+0x20` duration bounds, `+0x28`
// transition time bounds, `+0x30` bitmap, `+0x40` sequence index (i16),
// `+0x48` radius bounds, `+0x58` rotation rate bounds, `+0x60` and
// `+0x70` colour bounds (alpha, red, green, blue), `+0x84` point
// physics, and from `+0xdc` the shader words `part` has from `+0xd4`
// (`+0xe2` the framebuffer blend function, `part`'s `+0xda`: fire states
// 3, smoke states 0, as on `part`).
//
// A particle moves through its type's particle states in order; Halo 2's
// particle has one look. So each particle state that lasts (its duration
// and transition not both zero) becomes its own particle system, its
// `prt3` built from the state as a `part` ([`H1Particle`]), all created
// with the effect part: the type's initial count each, the state's
// duration plus transition as lifespan, its radius and colour. The
// state sequence (fire turning to smoke), the creation physics and its
// constants (no proven meaning: explosion `0.03 0.08 200`, jet
// `45 0.0355 1`) and rotation rate are not carried. For object
// attachments we also use the system states’ emission rates and timing
// ([`attached_particle_effect`]).

/// Halo 1 `pctl` sizes.
pub const H1_PCTL_SIZE: usize = 0x68;
pub const H1_PCTL_TYPE: usize = 0x80;
pub const H1_PCTL_PARTICLE_STATE: usize = 0x178;
const H1_PCTL_SYSTEM_STATE: usize = 0xc0;
// E (Halo 1) 0x0010fc27 scales the emission rate by the attachment output.
const H1_SCALE_RATE: u32 = 0x1000;
// E (Halo 1) 0x0010edbf: on the last state, stop, wrap or reverse.
const H1_LOOP_STATES: u32 = 1;
const H1_REVERSE_AT_END: u32 = 2;

/// One particle state of a Halo 1 particle system type.
#[derive(Clone, Debug, PartialEq)]
pub struct H1ParticleState {
    pub name: String,
    pub duration: [f32; 2],
    pub transition: [f32; 2],
    pub bitmap: Option<H1TagId>,
    pub sequence: i16,
    pub radius: [f32; 2],
    pub colour: [[f32; 4]; 2],
    pub blend: u16,
    pub physics: Option<H1TagId>,
    pub collision_radius_scale: f32,
}

/// Initial velocity and scatter of particles born in a system state.
/// E (Halo 1) creation dispatch at 0x0010f8c9; the emitter state supplies
/// its kind at +0xb0. The initial burst instead uses the type's +0x54.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct H1ParticleBirth {
    pub kind: u16,
    pub constants: [f32; 3],
}

impl H1ParticleBirth {
    fn words(self) -> Option<[u32; 4]> {
        let kind = match self.kind {
            1 => RADIAL_BIRTH,
            2 => DIRECTIONAL_BIRTH,
            // Kind 0 inherits object velocity natively (0x00247da9).
            _ => return None,
        };
        let [a, b, c] = self.constants.map(f32::to_bits);
        Some([kind, a, b, c])
    }
}

/// The emission phase of a Halo 1 particle system type.
#[derive(Clone, Debug, PartialEq)]
pub struct H1EmitterState {
    pub duration: [f32; 2],
    pub transition: [f32; 2],
    pub particles_per_second: f32,
    pub minimum_population: f32,
    pub birth: H1ParticleBirth,
}

/// A Halo 1 particle system type, with separate emitter and particle states.
#[derive(Clone, Debug, PartialEq)]
pub struct H1ParticleType {
    pub name: String,
    pub flags: u32,
    pub initial_count: u16,
    pub initial_birth: H1ParticleBirth,
    pub particle_radius: f32,
    pub default_physics: Option<H1TagId>,
    pub emitter_states: Vec<H1EmitterState>,
    pub states: Vec<H1ParticleState>,
}

fn name32(b: &[u8]) -> String {
    let end = b.iter().take(32).position(|c| *c == 0).unwrap_or(32.min(b.len()));
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// The particle types of a Halo 1 `pctl`.
pub fn particle_system_types(map: &H1Map, tag: &Tag) -> Result<Vec<H1ParticleType>> {
    let b = tag_bytes(map, tag, b"pctl", H1_PCTL_SIZE)?;
    let f = |x: &[u8], o: usize| f32::from_le_bytes(x[o..o + 4].try_into().expect("4"));
    let h = |x: &[u8], o: usize| u16::from_le_bytes([x[o], x[o + 1]]);
    let birth =
        |bytes: &[u8], kind_at, constants_at, fallback: [f32; 3]| -> Result<H1ParticleBirth> {
            let mut constants = fallback;
            if let Some(block) = h1map::Block::read(bytes, constants_at) {
                for (c, x) in constants
                    .iter_mut()
                    .zip(map.block(block, 4)?.chunks_exact(4))
                {
                    *c = f(x, 0);
                }
            }
            Ok(H1ParticleBirth {
                kind: h(bytes, kind_at),
                constants,
            })
        };
    let mut out = Vec::new();
    let Some(tb) = h1map::Block::read(b, 0x5c) else {
        return Ok(out);
    };
    let mut out = Vec::new();
    let Some(tb) = h1map::Block::read(b, 0x5c) else { return Ok(out) };
    for t in map.block(tb, H1_PCTL_TYPE)?.chunks_exact(H1_PCTL_TYPE) {
        let initial_birth = birth(t, 0x54, 0x5c, [0.0; 3])?;
        let mut emitter_states = Vec::new();
        if let Some(sb) = h1map::Block::read(t, 0x68) {
            for s in map
                .block(sb, H1_PCTL_SYSTEM_STATE)?
                .chunks_exact(H1_PCTL_SYSTEM_STATE)
            {
                emitter_states.push(H1EmitterState {
                    duration: [f(s, 0x20), f(s, 0x24)],
                    transition: [f(s, 0x28), f(s, 0x2c)],
                    // E (Halo 1) 0x0010f6a5 uses this rate times the timestep.
                    particles_per_second: f(s, 0x58),
                    minimum_population: f(s, 0x54),
                    birth: birth(s, 0xb0, 0xb4, initial_birth.constants)?,
                });
            }
        }
        let mut states = Vec::new();
        if let Some(sb) = h1map::Block::read(t, 0x74) {
            for s in map.block(sb, H1_PCTL_PARTICLE_STATE)?.chunks_exact(H1_PCTL_PARTICLE_STATE) {
                let argb = |o: usize| [f(s, o), f(s, o + 4), f(s, o + 8), f(s, o + 0xc)];
                states.push(H1ParticleState {
                    name: name32(s),
                    duration: [f(s, 0x20), f(s, 0x24)],
                    transition: [f(s, 0x28), f(s, 0x2c)],
                    bitmap: reference(map, s, 0x30),
                    sequence: h(s, 0x40) as i16,
                    radius: [f(s, 0x48), f(s, 0x4c)],
                    colour: [argb(0x60), argb(0x70)],
                    blend: h(s, 0xe2),
                    physics: reference(map, s, 0x84),
                    collision_radius_scale: f(s, 0x80),
                });
            }
        }
        out.push(H1ParticleType {
            name: name32(t),
            flags: u32::from_le_bytes(t[0x20..0x24].try_into().expect("4")),
            initial_count: h(t, 0x24),
            initial_birth,
            particle_radius: f(t, 0x2c),
            default_physics: reference(map, b, 0x38),
            emitter_states,
            states,
        });
    }
    Ok(out)
}

impl H1ParticleState {
    /// Lasts some time: its duration or transition bounds not all zero.
    pub fn lasts(&self) -> bool {
        self.duration[1] > 0.0 || self.transition[1] > 0.0
    }

    /// The state as a `part`: its bitmap, sequence and blend function, its
    /// duration plus transition as lifespan.
    pub fn as_particle(&self) -> H1Particle {
        H1Particle {
            flags: 0,
            bitmap: self.bitmap,
            collision_effect: None,
            death_effect: None,
            sequences: [self.sequence.max(0) as u16, 1, 0, 0],
            blend: self.blend,
            lifespan: [self.duration[0] + self.transition[0], self.duration[1] + self.transition[1]],
        }
    }

    /// The state as an effect particle of `count` at `location`, still.
    pub fn as_effect_particle(&self, count: u16, location: i16) -> H1EffectParticle {
        let n = count.min(i16::MAX as u16) as i16;
        H1EffectParticle {
            location,
            particle: None,
            count: [n, n],
            velocity: [0.0, 0.0],
            velocity_cone: 0.0,
            radius: self.radius,
            tint: self.colour,
        }
    }
}

/// A native effect for one attached particle type. Each type has its own
/// clock. An initial burst plays once. Later events follow the emission states,
/// including pauses and the source loop mode.
pub(crate) fn attached_particle_effect(
    ty: &H1ParticleType,
    states: &[AttachmentParticle],
    marker: &str,
) -> Result<Option<Block>> {
    if states.is_empty() {
        return Ok(None);
    }
    let d = def(b"effe")?;
    let mut r = defs::default_record(d);
    let mut location = defs::default_record(d.child("locations").expect("effe locations"));
    set(&mut location, "marker", Val::text(marker))?;
    set(&mut r, "locations", Val::Block(vec![location]))?;
    let ev_def = d.child("events").expect("effe events");
    let event = |bounds: [f32; 2],
                 rate: [f32; 2],
                 scale: bool,
                 minimum: [f32; 2],
                 birth: H1ParticleBirth|
     -> Result<Record> {
        let mut ev = defs::default_record(ev_def);
        set(
            &mut ev,
            "duration_bounds",
            Val::floats(&bounds.map(|v| v.max(TICK))),
        )?;
        set(
            &mut ev,
            "attachment_population",
            Val::Int(population_word(minimum, ty.flags & 0x800 != 0).into()),
        )?;
        let mut systems = Vec::new();
        if rate[0] > 0.0 || rate[1] > 0.0 {
            for p in states {
                systems.push(particle_system_at_rate(
                    &p.source.as_effect_particle(0, 0),
                    &p.source.as_particle(),
                    p.tag,
                                rate,
                    Some(scale),
                    p.movement,
                    birth,
                )?);
            }
        }
        set(&mut ev, "particle_systems", Val::Block(systems))?;
        Ok(ev)
    };
    let mut events = Vec::new();
    if ty.initial_count != 0 {
        events.push(event(
            [TICK; 2],
            [f32::from(ty.initial_count); 2],
            false,
            [0.0; 2],
            ty.initial_birth,
        )?);
    }
    let first_phase = events.len();
    let phases = &ty.emitter_states;
    let loops = ty.flags & H1_LOOP_STATES != 0;
    if phases.iter().any(|s| s.particles_per_second > 0.0) {
        let scale = ty.flags & H1_SCALE_RATE != 0;
        let mut order: Vec<_> = (0..phases.len()).collect();
        if loops && ty.flags & H1_REVERSE_AT_END != 0 {
            order.extend((0..phases.len()).rev());
        }
        for (i, &index) in order.iter().enumerate() {
            let s = &phases[index];
            let rate = s.particles_per_second.max(0.0) * TICK;
            let next = order.get(i + 1).or_else(|| loops.then(|| &order[0]));
            if s.duration[1] > 0.0 || s.transition[1] <= 0.0 || next.is_none() {
                events.push(event(
                    s.duration,
                    [rate; 2],
                    scale,
                    [s.minimum_population; 2],
                    s.birth,
                )?);
            }
            if let Some(&next) = next
                && s.transition[1] > 0.0
            {
                let next = &phases[next];
                let next_rate = next.particles_per_second.max(0.0) * TICK;
                events.push(event(
                    s.transition,
                    [rate, next_rate],
                    scale,
                    [s.minimum_population, next.minimum_population],
                    s.birth,
                )?);
            }
        }
    }
    let loop_start = if loops && events.len() > first_phase {
        first_phase as i64
    } else {
        -1
    };
    if events.is_empty() {
        return Ok(None);
    }
    // The stock effect root ends with two unused words at +0x28.
    // Mark the tag so event population words never affect retail effects.
    set(
        &mut r,
        "unknown_28",
        Val::Bytes([POPULATION_CLOCK.to_le_bytes(), [0; 4]].concat()),
    )?;
    set(&mut r, "loop_start_event", Val::Int(loop_start))?;
    set(&mut r, "events", Val::Block(events))?;
    defs::to_built(d, &r, &mut |_| None)
        .map(Some)
        .map_err(|e| Error::Invalid(e.to_string()))
}

pub(crate) struct AttachmentParticle {
    pub tag: TagId,
    pub source: H1ParticleState,
    pub movement: Option<TagId>,
}

#[cfg(test)]
mod attachment_tests {
    use super::*;

    #[test]
    fn attached_particle_effect_uses_burst_and_emitter_states() {
        let d = def(b"effe").unwrap();
        let state = H1ParticleState {
            name: "smoke".into(),
            duration: [0.5, 0.75],
            transition: [0.25, 0.5],
            bitmap: None,
            sequence: 0,
            radius: [1.0, 2.0],
            colour: [[1.0; 4]; 2],
            blend: 0,
            physics: None,
            collision_radius_scale: 1.0,
        };
        let tag = TagId(0xe100_0007);
        let mut ty = H1ParticleType {
            name: "test".into(),
            flags: H1_LOOP_STATES | H1_SCALE_RATE,
            initial_count: 3,
            initial_birth: H1ParticleBirth::default(),
            particle_radius: 1.0,
            default_physics: None,
            emitter_states: vec![
                H1EmitterState {
                    duration: [1.0, 2.0],
                    transition: [0.25, 0.5],
                    particles_per_second: 30.0,
                    minimum_population: 10.0,
                    birth: H1ParticleBirth::default(),
                },
                H1EmitterState {
                    duration: [2.0, 3.0],
                    transition: [0.0; 2],
                    particles_per_second: 0.0,
                    minimum_population: 2.0,
                    birth: H1ParticleBirth::default(),
                },
            ],
            states: vec![],
        };
        let states = [AttachmentParticle {
            tag,
            source: state,
            movement: None,
        }];
        let root = attached_particle_effect(&ty, &states, "burn")
            .unwrap()
            .unwrap();
        let effect = defs::from_built(d, &root).unwrap();
        assert_eq!(effect.at("loop_start_event"), Some(&Val::Int(1)));
        assert_eq!(effect.at("locations[0].marker"), Some(&Val::text("burn")));
        let events = block_of(&effect, "events");
        assert_eq!(events.len(), 4);
        assert_eq!(
            events[2].at("attachment_population"),
            Some(&Val::Int(population_word([10.0, 2.0], false).into()))
        );
        for (ev, bounds, rate, modifier) in [
            (&events[0], [TICK; 2], [3.0; 2], -1),
            (&events[1], [1.0, 2.0], [1.0; 2], 9),
            (&events[2], [0.25, 0.5], [1.0, 0.0], 9),
        ] {
            assert_eq!(ev.at("duration_bounds"), Some(&Val::floats(&bounds)));
            assert_eq!(ev.at("particle_systems[0].location"), Some(&Val::Int(0)));
            assert!(matches!(ev.at("particle_systems[0].particle"),
                Some(Val::Ref { target: defs::Target::Id(id), .. }) if *id == tag
            ));
            let property = "particle_systems[0].emitters[0].property_8";
            assert_eq!(
                ev.at(&format!("{property}.modifier_input")),
                Some(&Val::Int(modifier))
            );
            let Some(Val::Data(curve)) = ev.at(&format!("{property}.function")) else {
                panic!("missing rate curve")
            };
            let curve = Function::parse(curve).unwrap();
            assert_eq!(curve.range, rate);
            if rate[0] != rate[1] {
                assert_eq!(ev.at(&format!("{property}.input")), Some(&Val::Int(4)));
                assert_eq!(curve.evaluate(0.5, 0.0), 0.5);
            }
        }
        assert_eq!(
            events[3].at("duration_bounds"),
            Some(&Val::floats(&[2.0, 3.0]))
        );
        assert!(block_of(&events[3], "particle_systems").is_empty());

        ty.flags = 0;
        let root = attached_particle_effect(&ty, &states, "burn")
            .unwrap()
            .unwrap();
        let finite = defs::from_built(d, &root).unwrap();
        assert_eq!(finite.at("loop_start_event"), Some(&Val::Int(-1)));

        ty.initial_count = 0;
        ty.emitter_states.clear();
        assert!(
            attached_particle_effect(&ty, &states, "burn")
                .unwrap()
                .is_none()
        );
    }
}
