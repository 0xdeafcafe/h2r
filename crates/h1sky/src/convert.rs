//! Halo 1 sky to Halo 2 `sky `.
//!
//! The output is a [`h1models::Converted`] block tree (the same shape
//! `map::build` consumes for converted models): a `0xac`-byte root with the
//! sky's lights as child blocks. What maps where, and what has no Halo 2
//! home, is listed in `docs/halo1/sky.md`.
//!
//! Synthesised values (not read from the source sky, which has no such
//! field): the light flags `15`, the retail-common value; the root word at
//! `+0x10` `0`; the model scale, the least that keeps the whole sky behind
//! the level ([`model_scale`]: Halo 1 sky models are authored full size,
//! but some come within a hundred units of the camera); camera translation
//! scale `1.0` (the Halo 1 sky box stays centred on the camera). Lens
//! flares are converted by the caller (`h1convert::lens`) and passed in
//! by datum, one per light ([`convert_sky_with_flares`]); null otherwise.

use map::build::TagSet;
use map::{CacheType, FourCC, TagId};

use crate::Result;
use crate::h1;

/// Flags written into each converted light's parameter record: the value on
/// 49 of 91 retail skies. Halo 1's light flags (1 or 3) have a different,
/// unknown meaning and are not copied.
pub const LIGHT_FLAGS: u32 = 15;

/// The Halo 2 `sky ` definition (`crates/tags/src/defs/sky_.txt`).
pub fn sky_def() -> &'static tags::defs::StructDef {
    tags::defs::Defs::builtin()
        .group(FourCC(*b"sky "))
        .expect("sky is defined")
}

/// Builds the Halo 2 `sky ` tree for `sky` from its typed definition
/// ([`sky_def`]): every field not set here is the definition's default
/// (zero, null, empty). `model` is the datum of the converted render
/// model ([`h1models::model_from_map`] output, installed into the same
/// set), or `None` for skies with no model; `model_scale` goes to
/// `model_scale` ([`model_scale`]).
pub fn convert_sky(sky: &h1::Sky, model: Option<TagId>, model_scale: f32) -> h1models::Converted {
    convert_sky_with_flares(sky, model, model_scale, &[])
}

/// [`convert_sky`] with light `k`'s lens flare (`lens_flare`) naming
/// `flares[k]` (a converted Halo 1 `lens`); a light past the slice or
/// with `None` keeps a null flare.
pub fn convert_sky_with_flares(
    sky: &h1::Sky,
    model: Option<TagId>,
    model_scale: f32,
    flares: &[Option<TagId>],
) -> h1models::Converted {
    use tags::defs::{self, Val};
    let def = sky_def();
    let mut r = defs::default_record(def);
    let set = |r: &mut defs::Record, p: &str, v: Val| r.set(p, v).expect("sky definition field");
    set(
        &mut r,
        "model",
        Val::tag(FourCC(*b"mode"), model.unwrap_or(TagId::NULL)),
    );
    set(&mut r, "model_scale", Val::Float(model_scale));
    set(&mut r, "camera_translation_scale", Val::Float(1.0));
    set(&mut r, "fallback_color", Val::floats(&sky.ambient_first_rgb()));
    // Halo 1's two fog runs as the two fog blocks, in order.
    for (path, fog) in ["fog_a", "fog_b"].into_iter().zip(sky.fogs()) {
        let Some(f) = fog else { continue };
        let fdef = def.child(path).expect("fog block");
        let mut e = defs::default_record(fdef);
        set(&mut e, "color", Val::floats(&f.color));
        set(&mut e, "max_density", Val::Float(f.max_density));
        set(&mut e, "start_distance", Val::Float(f.start_distance));
        set(&mut e, "opaque_distance", Val::Float(f.opaque_distance));
        set(&mut r, path, Val::Block(vec![e]));
    }
    let ldef = def.child("lights").expect("lights");
    let pdef = ldef.child("parameters").expect("parameters");
    let lights = sky
        .lights
        .iter()
        .enumerate()
        .map(|(k, l)| {
            let mut e = defs::default_record(ldef);
            set(&mut e, "yaw", Val::Float(l.direction[0]));
            set(&mut e, "pitch", Val::Float(l.direction[1]));
            set(
                &mut e,
                "lens_flare",
                Val::tag(FourCC(*b"lens"), flares.get(k).copied().flatten().unwrap_or(TagId::NULL)),
            );
            let mut p = defs::default_record(pdef);
            set(&mut p, "flags", Val::Int(i64::from(LIGHT_FLAGS)));
            set(&mut p, "color", Val::floats(&l.color));
            set(&mut p, "power", Val::Float(l.power));
            set(&mut p, "falloff_distance", Val::Float(l.falloff_distance));
            set(&mut p, "cutoff_angle", Val::Float(l.cutoff_angle));
            set(&mut e, "parameters", Val::Block(vec![p]));
            e
        })
        .collect();
    set(&mut r, "lights", Val::Block(lights));
    let root = defs::to_built(def, &r, &mut |_| None).expect("a sky record builds");
    h1models::Converted {
        group: FourCC(*b"sky "),
        root,
        raw: Vec::new(),
    }
}

/// A Halo 1 sky read for conversion: the tag, its converted render model
/// (tag path and tree) when it has one, and the model scale that keeps it
/// behind the level it was read with ([`model_scale`]).
#[derive(Debug, Clone)]
pub struct FromMap {
    /// The Halo 1 sky tag.
    pub sky: h1::Sky,
    /// The render model's tag path and conversion.
    pub model: Option<(String, h1models::Converted)>,
    /// `sky +0x14`.
    pub model_scale: f32,
    /// The Halo 1 shader of each of the model's materials, in material
    /// order (the converted model's material `k` is Halo 1's shader `k`).
    pub shaders: Vec<h1map::TagId>,
}

/// Reads the `sky ` tag `tag` of `map`, and converts its render model when
/// it has one. The model scale is the least that puts every surface of the
/// model at least [`level_reach`] of `map` from the camera. Pair the model
/// with [`install_sky`], which installs both into one set and links the
/// reference.
pub fn sky_from_map(map: &h1map::H1Map, tag: &h1map::Tag) -> Result<FromMap> {
    let sky = h1::Sky::from_map(map, tag)?;
    let mut model_scale = 1.0;
    let mut shaders = Vec::new();
    let model = if sky.model.id.is_none() {
        None
    } else {
        let tag = map.tag(sky.model.id)?;
        if tag.group != h1map::Group::new(b"mode") {
            return Err(crate::Error::Invalid(format!(
                "sky model reference names a {} tag",
                tag.group
            )));
        }
        let (h1_model, converted) =
            h1models::model_from_map(map, tag, &h1models::mode::ModelOptions::default())?;
        model_scale = self::model_scale(nearest_surface(&h1_model), level_reach(map));
        shaders = h1_model.shaders.iter().map(|s| s.tag).collect();
        Some((tag.name.clone(), converted))
    };
    Ok(FromMap {
        sky,
        model,
        model_scale,
        shaders,
    })
}

/// The farthest one point of `map`'s level can be from another: the
/// diagonal of the box around every structure BSP's world bounds. 0 for a
/// map with no BSP that reads.
pub fn level_reach(map: &h1map::H1Map) -> f32 {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for r in map.structure_bsps().unwrap_or_default() {
        let Ok(bsp) = map.structure_bsp(&r) else {
            continue;
        };
        let b = bsp.world_bounds();
        for a in 0..3 {
            if b[2 * a].is_finite() && b[2 * a + 1].is_finite() && b[2 * a] <= b[2 * a + 1] {
                lo[a] = lo[a].min(b[2 * a]);
                hi[a] = hi[a].max(b[2 * a + 1]);
            }
        }
    }
    if (0..3).any(|a| lo[a] > hi[a]) {
        return 0.0;
    }
    len(sub(hi, lo))
}

/// Installs `model` (if any) and the converted `sky` into `set`, linking
/// the sky's model reference, and returns the sky's datum. `name` is the
/// Halo 1 sky's tag path.
pub fn install_sky(
    set: &mut TagSet,
    sky: &h1::Sky,
    model: Option<(&str, h1models::Converted)>,
    name: &str,
    model_scale: f32,
) -> Result<TagId> {
    install_sky_with_flares(set, sky, model, name, model_scale, &[])
}

/// [`install_sky`] with the lights' lens flares
/// ([`convert_sky_with_flares`]).
pub fn install_sky_with_flares(
    set: &mut TagSet,
    sky: &h1::Sky,
    model: Option<(&str, h1models::Converted)>,
    name: &str,
    model_scale: f32,
    flares: &[Option<TagId>],
) -> Result<TagId> {
    let model_id = match model {
        Some((model_name, converted)) => Some(converted.install(set, model_name)?),
        None => None,
    };
    Ok(convert_sky_with_flares(sky, model_id, model_scale, flares).install(set, name)?)
}

/// Builds a set holding one converted sky and returns its bytes and the
/// datums (the sky's, and the model's when present). Tests and the disc
/// checks use this; real imports add the sky to the level's own set.
pub fn build_sky_map(
    sky: &h1::Sky,
    model: Option<(&str, h1models::Converted)>,
    name: &str,
    model_scale: f32,
) -> Result<(Vec<u8>, TagId, Option<TagId>)> {
    let mut set = TagSet::new(CacheType::Multiplayer, "h1skytest", "invented\\h1skytest");
    let id = install_sky(&mut set, sky, model, name, model_scale)?;
    let model_id = set
        .slots
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            s.tag
                .as_ref()
                .is_some_and(|t| *i != id.index() as usize && t.group == FourCC(*b"mode"))
        })
        .find_map(|(i, _)| set.slots[i].tag.as_ref().map(|t| t.id));
    let (bytes, _) = set.build().map_err(crate::Error::Map)?;
    Ok((bytes, id, model_id))
}

/// Triangles whose plane passes closer than this to the sky's origin are
/// seen edge-on from the camera (Halo 1 draws a sky around the eye): they
/// cover no area on screen at any scale, so they don't bound it.
pub const EDGE_ON: f32 = 1.0;

/// How near the camera any surface of a Halo 1 sky model comes, in the
/// model's own units: the least distance from its origin to a triangle
/// that isn't seen edge-on ([`EDGE_ON`]). `None` for a model with no such
/// triangle.
pub fn nearest_surface(model: &h1models::h1::Model) -> Option<f32> {
    let mut nearest: Option<f32> = None;
    for part in model.geometries.iter().flat_map(|g| &g.parts) {
        for t in h1models::h1::strip_triangles(&part.strip) {
            let Some([a, b, c]) = t
                .iter()
                .map(|&i| part.vertices.get(usize::from(i)).map(|v| v.position))
                .collect::<Option<Vec<_>>>()
                .and_then(|v| <[[f32; 3]; 3]>::try_from(v).ok())
            else {
                continue;
            };
            let n = cross(sub(b, a), sub(c, a));
            let len = dot(n, n).sqrt();
            if len == 0.0 || (dot(n, a) / len).abs() < EDGE_ON {
                continue;
            }
            let d = origin_to_triangle(a, b, c);
            nearest = Some(nearest.map_or(d, |m| m.min(d)));
        }
    }
    nearest
}

/// The model scale (`sky +0x14`) that keeps every surface of a sky whose
/// nearest one is `nearest` from its origin ([`nearest_surface`]) at least
/// `reach` from the camera: never below 1.
///
/// Halo 1 draws its sky first and the level over it, so a sky surface
/// nearer the eye than the level (a30's distant ground, 113 units below
/// the camera) still sits behind it. Halo 2 draws the sky after the opaque
/// level, depth-tested (the scene loop's sky step, E `0x0002c0bc` calling
/// `0x0003ede0` after layers 1 to 6), centred on the camera when the
/// camera translation scale is 1 (E `0x0003ebd0`: the translation is that
/// scale times the camera position). Scaling the model about the camera
/// leaves its picture unchanged and only moves it back in depth, so a
/// level no wider than `reach` is always drawn in front of it.
pub fn model_scale(nearest: Option<f32>, reach: f32) -> f32 {
    match nearest {
        Some(d) if d > 0.0 && reach > d => reach / d,
        _ => 1.0,
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn lerp(a: [f32; 3], d: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + t * d[0], a[1] + t * d[1], a[2] + t * d[2]]
}

/// Distance from the origin to triangle `abc`: the closest point by the
/// triangle's Voronoi regions (vertices, edges, face).
fn origin_to_triangle(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let p = [0.0; 3];
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot(ab, ap), dot(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return len(a);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot(ab, bp), dot(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return len(b);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return len(lerp(a, ab, d1 / (d1 - d3)));
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot(ab, cp), dot(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return len(c);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return len(lerp(a, ac, d2 / (d2 - d6)));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && d4 - d3 >= 0.0 && d5 - d6 >= 0.0 {
        return len(lerp(b, sub(c, b), (d4 - d3) / ((d4 - d3) + (d5 - d6))));
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    len([
        a[0] + ab[0] * v + ac[0] * w,
        a[1] + ab[1] * v + ac[1] * w,
        a[2] + ab[2] * v + ac[2] * w,
    ])
}
