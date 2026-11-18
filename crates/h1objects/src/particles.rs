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
}

impl H1Particle {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"part", H1_PART_SIZE)?;
        let h = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        Ok(Self {
            flags: u32::from_le_bytes(b[0..4].try_into().expect("4")),
            bitmap: reference(map, b, 0x4),
            collision_effect: reference(map, b, 0x48),
            death_effect: reference(map, b, 0x58),
            sequences: [h(0x98), h(0x9a), h(0x9c), h(0x9e)],
            blend: h(0xda),
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
