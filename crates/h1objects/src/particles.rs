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
// The colour and vector values are the parameters' compiled defaults;
// they are not derived here but taken from the base's particles on the
// same stem (the most common set: [`ParticleShader`]), as a role.
// Two words vary on retail with nothing reading them (a lod's `+0x6`, a
// pass's `+0x2`); they are written zero.

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

fn block_of<'r>(r: &'r Record, path: &str) -> &'r [Record] {
    match r.at(path) {
        Some(Val::Block(b)) => b,
        _ => &[],
    }
}

fn int(r: &Record, path: &str) -> i64 {
    match r.at(path) {
        Some(Val::Int(v)) => *v,
        _ => 0,
    }
}

fn float_list(r: &Record, path: &str) -> Vec<f32> {
    match r.at(path) {
        Some(Val::List(v)) => v
            .iter()
            .map(|x| match x {
                Val::Float(f) => *f,
                _ => 0.0,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Every stem the base's particles use, with its role
/// ([`ParticleShader`]): the colour and vector values most of its
/// particles hold.
pub fn particle_shaders(base: &crate::base::Base) -> Result<Vec<ParticleShader>> {
    use std::collections::BTreeMap;
    let d = def(b"prt3")?;
    let meta = tags::shader::Metadata::new(base.meta.base, base.meta.bytes.clone());
    type Values = (Vec<u32>, Vec<[u32; 4]>);
    let mut tally: BTreeMap<u32, BTreeMap<Values, usize>> = BTreeMap::new();
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"prt3")) {
        let r = base.record(d, t.0)?;
        let Some(Val::Ref { target: defs::Target::Id(stem), .. }) = r.at("shader_template") else {
            continue;
        };
        let Some(pp) = block_of(&r, "postprocess").first() else { continue };
        let colors = block_of(pp, "colors").iter().map(|c| int(c, "color") as u32).collect();
        let vectors = block_of(pp, "vectors")
            .iter()
            .map(|v| {
                let f = float_list(v, "vector");
                std::array::from_fn(|k| f.get(k).copied().unwrap_or(0.0).to_bits())
            })
            .collect();
        *tally.entry(stem.0).or_default().entry((colors, vectors)).or_default() += 1;
    }
    let mut out = Vec::new();
    for (stem, values) in tally {
        let stem = TagId(stem);
        let Some(t) = base.tags.iter().find(|t| t.0 == stem) else { continue };
        let template = tags::shader::ShaderTemplate::decode(&meta, t.3)
            .map_err(|e| Error::Invalid(format!("{}: {e}", t.2)))?
            .compiled()
            .map_err(|e| Error::Invalid(format!("{}: {e}", t.2)))?
            .clone();
        let ((colors, vectors), _) = values
            .into_iter()
            .max_by_key(|(v, n)| (*n, std::cmp::Reverse(v.clone())))
            .expect("tallied");
        out.push(ParticleShader {
            name: t.2.clone(),
            stem,
            template,
            colors,
            vectors: vectors.into_iter().map(|v| v.map(f32::from_bits)).collect(),
        });
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
// base's static particles write them ([`ParticleRole`]): white, opaque,
// unit size (the emitter scales it), no rotation, the first sprite.

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

/// The base's particle role: its stems' compiled shaders and the root
/// words of a static particle (the five constant properties, the input
/// mask, vertex shader, quad and UV window most of the base's particles
/// with white, opaque, unit-size, unrotated constants hold).
#[derive(Clone, Debug)]
pub struct ParticleRole {
    pub shaders: Vec<ParticleShader>,
    pub statics: Vec<(&'static str, Val)>,
}

/// The root fields [`ParticleRole::statics`] holds.
pub const STATIC_FIELDS: [&str; 9] =
    ["color", "alpha", "size", "rotation", "frame", "input_mask", "vertex_shader", "quad", "uv_window"];

fn constant(r: &Record, slot: &str) -> Option<[f32; 2]> {
    let Some(Val::Data(b)) = r.at(&format!("{slot}.function")) else { return None };
    let f = tags::effects::function::Function::parse(b).ok()?;
    (f.kind == tags::effects::function::FunctionType::Constant).then_some(f.range)
}

pub fn particle_role(base: &crate::base::Base) -> Result<Option<ParticleRole>> {
    use std::collections::BTreeMap;
    let d = def(b"prt3")?;
    let mut tally: BTreeMap<String, (usize, Vec<(&'static str, Val)>)> = BTreeMap::new();
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"prt3")) {
        let r = base.record(d, t.0)?;
        let white = matches!(r.at("color.function"), Some(Val::Data(b)) if b.get(4..8) == Some(&[0xff; 4][..]));
        let frame_static = matches!(r.at("frame.function"), Some(Val::Data(b)) if b.is_empty())
            || constant(&r, "frame") == Some([0.0, 0.0]);
        if !(white
            && constant(&r, "alpha") == Some([1.0, 1.0])
            && constant(&r, "size") == Some([1.0, 1.0])
            && constant(&r, "rotation") == Some([0.0, 0.0])
            && frame_static)
        {
            continue;
        }
        let vals: Vec<_> = STATIC_FIELDS.iter().map(|f| (*f, r.at(f).cloned().unwrap_or(Val::Int(0)))).collect();
        let e = tally.entry(format!("{vals:?}")).or_insert((0, vals));
        e.0 += 1;
    }
    let Some((_, (_, statics))) = tally.into_iter().max_by_key(|(k, (n, _))| (*n, std::cmp::Reverse(k.clone()))) else {
        return Ok(None);
    };
    Ok(Some(ParticleRole {
        shaders: particle_shaders(base)?,
        statics,
    }))
}

/// The `prt3` root of a Halo 1 particle on `shader`: its bitmap (converted
/// tag and first image's size), its collision and death effects
/// (converted, with their groups).
pub fn particle(
    p: &H1Particle,
    role: &ParticleRole,
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
    for (f, v) in &role.statics {
        set(&mut r, f, v.clone())?;
    }
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
// Bounds become their midpoint. The record words nothing above names
// (the particle system's flags, fade and LOD distances; the emitter's
// `+0x9c..+0xb8`; each constant property's input words) are the base's
// most common ([`EmitterRole`]). Halo 1's distribution radius, relative
// direction and offset, angular velocity and physics have no proven slot
// and are not carried.

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

/// The base's emitter role: the most common particle-system words, emitter
/// words (on shape-0 emitters) and per property slot the most common
/// constant encoding.
#[derive(Clone, Debug)]
pub struct EmitterRole {
    pub system: Vec<(&'static str, Val)>,
    pub emitter: Vec<(&'static str, Val)>,
    /// Per emitter property field, a constant property record of the base.
    pub constants: Vec<(&'static str, Record)>,
}

/// Particle-system fields the role holds.
pub const SYSTEM_ROLE_FIELDS: [&str; 8] =
    ["unknown_c", "unknown_e", "unknown_10", "unknown_12", "unknown_14", "unknown_16", "fade", "lod"];
/// Emitter fields the role holds.
pub const EMITTER_ROLE_FIELDS: [&str; 3] = ["unknown_9c", "unknown_b0", "unknown_b4"];
/// Emitter property fields.
pub const EMITTER_PROPERTIES: [&str; 9] = [
    "property_8", "lifespan", "property_28", "property_38", "property_48", "property_58", "property_68",
    "shape_input", "velocity_input",
];

fn is_constant(r: &Record, colour: bool) -> bool {
    let Some(Val::Data(b)) = r.at("function") else { return false };
    let Ok(f) = tags::effects::function::Function::parse(b) else { return false };
    f.kind == tags::effects::function::FunctionType::Constant
        && f.flags & 1 == 0
        && (f.flags & 0xf0 != 0) == colour
}

pub fn emitter_role(base: &crate::base::Base) -> Result<Option<EmitterRole>> {
    use std::collections::BTreeMap;
    let d = def(b"effe")?;
    fn top<T: Clone>(m: BTreeMap<String, (usize, T)>) -> Option<T> {
        m.into_iter()
            .max_by_key(|(k, (n, _))| (*n, std::cmp::Reverse(k.clone())))
            .map(|(_, (_, v))| v)
    }
    let mut systems: BTreeMap<String, (usize, Vec<(&'static str, Val)>)> = BTreeMap::new();
    let mut emitters: BTreeMap<String, (usize, Vec<(&'static str, Val)>)> = BTreeMap::new();
    let mut constants: BTreeMap<&'static str, BTreeMap<String, (usize, Record)>> = BTreeMap::new();
    let tally = |m: &mut BTreeMap<String, (usize, Vec<(&'static str, Val)>)>, v: Vec<(&'static str, Val)>| {
        m.entry(format!("{v:?}")).or_insert((0, v)).0 += 1;
    };
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"effe")) {
        let r = base.record(d, t.0)?;
        for ev in block_of(&r, "events") {
            for s in block_of(ev, "particle_systems") {
                tally(&mut systems, SYSTEM_ROLE_FIELDS.iter().map(|f| (*f, s.at(f).cloned().unwrap_or(Val::Int(0)))).collect());
                for e in block_of(s, "emitters") {
                    if int(e, "emission_shape") != 0 {
                        continue;
                    }
                    tally(&mut emitters, EMITTER_ROLE_FIELDS.iter().map(|f| (*f, e.at(f).cloned().unwrap_or(Val::Int(0)))).collect());
                    for f in EMITTER_PROPERTIES {
                        let Some(Val::Struct(p)) = e.at(f) else { continue };
                        if !is_constant(p, f == "property_58") {
                            continue;
                        }
                        let mut key = p.clone();
                        // The encoding, not the value.
                        if let Some(Val::Data(b)) = key.at("function").cloned() {
                            let mut b = b.clone();
                            b[4..12].fill(0);
                            key.set("function", Val::Data(b)).map_err(|e| Error::Invalid(e.to_string()))?;
                        }
                        constants.entry(f).or_default().entry(format!("{key:?}")).or_insert((0, p.clone())).0 += 1;
                    }
                }
            }
        }
    }
    let (Some(system), Some(emitter)) = (top(systems), top(emitters)) else { return Ok(None) };
    let mut list = Vec::new();
    for f in EMITTER_PROPERTIES {
        let Some(c) = constants.remove(f).and_then(top) else { return Ok(None) };
        list.push((f, c));
    }
    Ok(Some(EmitterRole { system, emitter, constants: list }))
}

/// The role's constant for `field` with value `v` (a scalar), or the packed
/// colour `argb` for the tint.
fn constant_property(role: &EmitterRole, field: &str, v: f32, argb: Option<u32>) -> Result<Record> {
    let mut p = role
        .constants
        .iter()
        .find(|c| c.0 == field)
        .map(|c| c.1.clone())
        .ok_or_else(|| Error::Invalid(format!("no constant for {field}")))?;
    let Some(Val::Data(b)) = p.at("function") else { return Err(Error::Invalid("constant".into())) };
    let mut b = b.clone();
    match argb {
        Some(c) => b[4..8].copy_from_slice(&c.to_le_bytes()),
        None => {
            b[4..8].copy_from_slice(&v.to_le_bytes());
            b[8..12].copy_from_slice(&v.to_le_bytes());
        }
    }
    set(&mut p, "function", Val::Data(b))?;
    Ok(p)
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
    role: &EmitterRole,
    duration: f32,
) -> Result<Record> {
    let sd = def(b"effe")?.child("events.particle_systems").expect("effe particle systems");
    let ed = sd.child("emitters").expect("emitters");
    let mut s = defs::default_record(sd);
    set(&mut s, "particle", Val::tag(FourCC(*b"prt3"), particle))?;
    set(&mut s, "location", Val::Int(p.location.into()))?;
    for (f, v) in &role.system {
        set(&mut s, f, v.clone())?;
    }
    let mut e = defs::default_record(ed);
    set(&mut e, "movement", Val::tag(FourCC(*b"pmov"), TagId::NULL))?;
    for (f, v) in &role.emitter {
        set(&mut e, f, v.clone())?;
    }
    set(&mut e, "emission_shape", Val::Int(0))?;
    let alpha = mid([p.tint[0][0], p.tint[1][0]]);
    let props: [(&str, f32, Option<u32>); 9] = [
        ("property_8", spawn_rate(p.count, duration), None),
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
        set(&mut e, f, Val::Struct(constant_property(role, f, v, c)?))?;
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
// `45 0.0355 1`), the system states and the rotation rate are not
// carried: the particles start still at the part's location.

/// Halo 1 `pctl` sizes.
pub const H1_PCTL_SIZE: usize = 0x68;
pub const H1_PCTL_TYPE: usize = 0x80;
pub const H1_PCTL_PARTICLE_STATE: usize = 0x178;

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
}

/// A Halo 1 particle system type: its initial count and particle states.
#[derive(Clone, Debug, PartialEq)]
pub struct H1ParticleType {
    pub name: String,
    pub initial_count: u16,
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
    let mut out = Vec::new();
    let Some(tb) = h1map::Block::read(b, 0x5c) else { return Ok(out) };
    for t in map.block(tb, H1_PCTL_TYPE)?.to_vec().chunks_exact(H1_PCTL_TYPE) {
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
                });
            }
        }
        out.push(H1ParticleType { name: name32(t), initial_count: h(t, 0x24), states });
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
