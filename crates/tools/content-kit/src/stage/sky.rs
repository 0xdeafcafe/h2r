//! The stage's sky: a dome of ours that shows which way is up and which
//! way is north, and where the day bay's sun is.
//!
//! - [`picture`]: an equirectangular picture, drawn in code. A vertical
//!   gradient (deep blue overhead, pale at the horizon, grey below), a
//!   dark line on the horizon, the letters N, W, S and E on plates
//!   `letter_elevation` degrees up at the four compass points, and the
//!   sun's disc and glow where `Layout::lights.sun` puts the sun. North is
//!   +x, the way the player spawns facing; the player's left (+y) is west.
//! - [`dome`]: the model, a sphere of `radius` about the camera, its
//!   triangles facing in, as a Halo 1 model that
//!   `h1_assets::models::mode::convert_model` writes as a `mode` (parts as
//!   strips, as a sky's are). The picture's column `u` is the azimuth
//!   `180 - 360 u` degrees, so the picture reads the right way round from
//!   inside, and its row `v` the elevation `90 - 180 v`.
//! - [`tag`]: the Halo 2 `sky ` tag naming the model, the day bay's sun as
//!   its one light, and the sky fill's mean as its fallback colour.
//! - [`name_sky`]: the scenario's skies block made one element naming it.

use h1_assets::models::h1::{
    Geometry, Model, Node, Part, Permutation, Region, ShaderRef, Vertex, strip_triangles,
    triangles_strip,
};
use map::build::Block;
use map::{FourCC, TagId};

use crate::stage::layout::Layout;
use crate::stage::texture::{Image, Rgba};
use crate::stage::{Error, Result};
use crate::stage::data::{Compass, SkyData, data};

/// The stage manifest's sky section (`content/stage/kit.yaml`).
pub fn sky_data() -> &'static SkyData {
    &data().sky
}

/// 5 by 7 glyphs: a row a byte, bit 4 the leftmost column.
const FONT: [(char, [u8; 7]); 4] = [
    ('N', [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11]),
    ('E', [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f]),
    ('S', [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e]),
    ('W', [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11]),
];

/// The unit vector of azimuth `az` and elevation `el`, degrees.
pub fn direction(az: f32, el: f32) -> [f32; 3] {
    let (az, el) = (az.to_radians(), el.to_radians());
    [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()]
}

/// The azimuth and elevation, degrees, of the picture's point `(u, v)`.
pub fn angles(u: f32, v: f32) -> (f32, f32) {
    (180.0 - 360.0 * u, 90.0 - 180.0 * v)
}

/// The picture's pixel (fractional) of azimuth `az` and elevation `el`.
pub fn pixel(az: f32, el: f32) -> (f32, f32) {
    let u = ((180.0 - az) / 360.0).rem_euclid(1.0);
    (u * sky_data().width as f32, (90.0 - el) / 180.0 * sky_data().height as f32)
}

fn lerp(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let mix = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8;
    [mix(0), mix(1), mix(2), 0xff]
}

/// The gradient and its horizon line at elevation `el`.
fn band(el: f32) -> Rgba {
    if el.abs() < sky_data().line_half {
        sky_data().colors.line
    } else if el >= 0.0 {
        lerp(sky_data().colors.horizon, sky_data().colors.zenith, (el / 90.0).powf(0.6))
    } else {
        lerp(sky_data().colors.ground_near, sky_data().colors.ground_far, -el / 90.0)
    }
}

/// Draws `rows` at `scale` on a plate, centred on pixel `(cx, cy)`; the
/// picture wraps at its sides, where south stands.
fn letter(img: &mut Image, rows: &[u8; 7], cx: i64, cy: i64, scale: u32) {
    let s = i64::from(scale);
    let (w, h) = (i64::from(img.width), i64::from(img.height));
    let (x0, y0) = (cx - 5 * s / 2, cy - 7 * s / 2);
    let mut put = |x: i64, y: i64, c: Rgba| {
        if (0..h).contains(&y) {
            img.set(x.rem_euclid(w) as u32, y as u32, c);
        }
    };
    for y in y0 - s..y0 + 7 * s + s {
        for x in x0 - s..x0 + 5 * s + s {
            put(x, y, sky_data().colors.plate);
        }
    }
    for (r, bits) in rows.iter().enumerate() {
        for c in 0..5 {
            if (*bits >> (4 - c)) & 1 == 1 {
                for dy in 0..s {
                    for dx in 0..s {
                        put(x0 + c * s + dx, y0 + r as i64 * s + dy, sky_data().colors.ink);
                    }
                }
            }
        }
    }
}

/// The sky's picture (module docs).
pub fn picture(layout: &Layout) -> Image {
    let sun = layout.lights.sun.direction();
    let mut img = Image::new(sky_data().width, sky_data().height, [0, 0, 0, 0xff]);
    for y in 0..sky_data().height {
        let el = 90.0 - (y as f32 + 0.5) / sky_data().height as f32 * 180.0;
        let row = band(el);
        for x in 0..sky_data().width {
            let az = 180.0 - (x as f32 + 0.5) / sky_data().width as f32 * 360.0;
            let d = direction(az, el);
            let cos = (d[0] * sun[0] + d[1] * sun[1] + d[2] * sun[2]).clamp(-1.0, 1.0);
            let apart = cos.acos().to_degrees();
            let c = if apart < sky_data().disc {
                sky_data().colors.sun
            } else if apart < sky_data().glow {
                let t = 1.0 - (apart - sky_data().disc) / (sky_data().glow - sky_data().disc);
                lerp(row, sky_data().colors.sun, t * t * 0.6)
            } else {
                row
            };
            img.set(x, y, c);
        }
    }
    for Compass { letter: ch, azimuth: az } in sky_data().compass.iter().copied() {
        let rows = FONT
            .iter()
            .find(|(g, _)| *g == ch)
            .map(|(_, r)| r)
            .expect("the font has the compass letters");
        let (x, y) = pixel(az, sky_data().letter_elevation);
        letter(
            &mut img,
            rows,
            x.round() as i64,
            y.round() as i64,
            sky_data().letter_scale,
        );
    }
    img
}

fn vertex(i: u32, j: u32) -> Vertex {
    let az = 180.0 - 360.0 * j as f32 / sky_data().segments as f32;
    let el = -90.0 + 180.0 * i as f32 / sky_data().rings as f32;
    let d = direction(az, el);
    let (a, e) = (az.to_radians(), el.to_radians());
    // Along increasing u (azimuth falls) and increasing v (elevation falls).
    let tangent = [a.sin(), -a.cos(), 0.0];
    let binormal = [e.sin() * a.cos(), e.sin() * a.sin(), -e.cos()];
    Vertex {
        position: [d[0] * sky_data().radius, d[1] * sky_data().radius, d[2] * sky_data().radius],
        normal: [-d[0], -d[1], -d[2]],
        binormal,
        tangent,
        texcoord: [j as f32 / sky_data().segments as f32, 1.0 - i as f32 / sky_data().rings as f32],
        nodes: [0, -1],
        weights: [1.0, 0.0],
    }
}

/// The dome's triangles, wound counter-clockwise seen from inside, as
/// indices into its vertices; those with no area (at the poles) dropped.
pub fn triangles() -> Vec<[u16; 3]> {
    let at = |i: u32, j: u32| (i * (sky_data().segments + 1) + j) as u16;
    let mut out = Vec::new();
    for i in 0..sky_data().rings {
        for j in 0..sky_data().segments {
            for t in [
                [at(i, j), at(i, j + 1), at(i + 1, j)],
                [at(i, j + 1), at(i + 1, j + 1), at(i + 1, j)],
            ] {
                let p: Vec<[f32; 3]> = t
                    .iter()
                    .map(|&k| {
                        let (i, j) = (u32::from(k) / (sky_data().segments + 1), u32::from(k) % (sky_data().segments + 1));
                        vertex(i, j).position
                    })
                    .collect();
                let (u, v) = (
                    [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]],
                    [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]],
                );
                let n = [
                    u[1] * v[2] - u[2] * v[1],
                    u[2] * v[0] - u[0] * v[2],
                    u[0] * v[1] - u[1] * v[0],
                ];
                if n[0] * n[0] + n[1] * n[1] + n[2] * n[2] > 1.0 {
                    out.push(t);
                }
            }
        }
    }
    out
}

/// The dome as a Halo 1 model: one node, one region of one permutation,
/// one geometry of one part, one shader.
pub fn dome() -> Model {
    let mut vertices = Vec::new();
    for i in 0..=sky_data().rings {
        for j in 0..=sky_data().segments {
            vertices.push(vertex(i, j));
        }
    }
    Model {
        flags: 0,
        node_list_checksum: 0,
        lod_cutoffs: [0.0; 5],
        base_map_scale: [1.0, 1.0],
        markers: Vec::new(),
        nodes: vec![Node {
            name: "sky".to_owned(),
            next_sibling: -1,
            first_child: -1,
            parent: -1,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            distance_from_parent: 0.0,
        }],
        regions: vec![Region {
            name: "sky".to_owned(),
            permutations: vec![Permutation {
                name: "default".to_owned(),
                flags: 0,
                lods: [0; 5],
            }],
        }],
        geometries: vec![Geometry {
            flags: 0,
            parts: vec![Part {
                flags: 0,
                shader: 0,
                centroid_nodes: [0, -1],
                centroid_weights: [1.0, 0.0],
                centroid: [0.0; 3],
                vertices,
                strip: triangles_strip(&triangles()),
            }],
        }],
        shaders: vec![ShaderRef {
            group: h1map::Group::new(b"soso"),
            tag: h1map::TagId::NONE,
            permutation: 0,
        }],
    }
}

/// The dome's triangles as the converter reads its strip.
pub fn model_triangles(model: &Model) -> Vec<[u16; 3]> {
    strip_triangles(&model.geometries[0].parts[0].strip)
}

/// The dome converted: a `mode` whose one material names `shader`, its
/// part a strip as a sky's is.
pub fn convert(shader: TagId) -> Result<h1_assets::models::Converted> {
    let options = h1_assets::models::mode::ModelOptions {
        shaders: vec![shader],
        strips: true,
    };
    h1_assets::models::mode::convert_model(&dome(), "stage sky", &options)
        .map_err(|e| Error::Build(format!("the sky's model: {e}")))
}


/// The Halo 2 `sky ` tag: the model at `model`, scale 1 (the dome is past
/// the stage already), the day bay's sun as its one light, and the mean of
/// the sky fill as its fallback colour.
pub fn tag(layout: &Layout, model: TagId) -> Result<h1_assets::models::Converted> {
    use tags::defs::{self, Val};
    let fail =
        |what: &str, e: &dyn std::fmt::Display| Error::Build(format!("the sky: {what}: {e}"));
    let def = h1_assets::sky::convert::sky_def();
    let set = |r: &mut defs::Record, p: &str, v: Val| r.set(p, v).map_err(|e| fail(p, &e));
    let sun = &layout.lights.sun;
    let mut r = defs::default_record(def);
    set(&mut r, "model", Val::tag(FourCC(*b"mode"), model))?;
    set(&mut r, "model_scale", Val::Float(1.0))?;
    set(&mut r, "camera_translation_scale", Val::Float(1.0))?;
    set(&mut r, "fallback_color", Val::floats(&layout.lights.fill))?;
    let ldef = def
        .child("lights")
        .ok_or_else(|| fail("lights", &"no such block"))?;
    let pdef = ldef
        .child("parameters")
        .ok_or_else(|| fail("lights.parameters", &"no such block"))?;
    let mut e = defs::default_record(ldef);
    set(&mut e, "yaw", Val::Float(sun.azimuth.to_radians()))?;
    set(&mut e, "pitch", Val::Float(sun.elevation.to_radians()))?;
    set(
        &mut e,
        "lens_flare",
        Val::tag(FourCC(*b"lens"), TagId::NULL),
    )?;
    let mut p = defs::default_record(pdef);
    set(
        &mut p,
        "flags",
        Val::Int(i64::from(h1_assets::sky::convert::LIGHT_FLAGS)),
    )?;
    set(&mut p, "color", Val::floats(&sun.color))?;
    set(&mut p, "power", Val::Float(sun.power))?;
    set(&mut p, "falloff_distance", Val::Float(50.0))?;
    set(&mut p, "cutoff_angle", Val::Float(sky_data().light_cutoff))?;
    set(&mut e, "parameters", Val::Block(vec![p]))?;
    set(&mut r, "lights", Val::Block(vec![e]))?;
    let root = defs::to_built(def, &r, &mut |_| None).map_err(|e| fail("building it", &e))?;
    Ok(h1_assets::models::Converted {
        group: FourCC(*b"sky "),
        root,
        raw: Vec::new(),
    })
}

/// `scnr +0x08`: the skies block, an 8-byte tag reference an element.
pub const SKIES: u32 = 0x08;

/// Makes the scenario's skies block `root` one element naming `sky`: the
/// sky every cluster of the stage's structure draws (cluster sky 0). Any
/// block already there is replaced.
pub fn name_sky(root: &mut Block, sky: TagId) {
    root.fixups.retain(|f| f.offset != SKIES);
    let mut element = Block::zeroed(8);
    element.tag_ref(0, sky);
    root.block(SKIES, 1, element);
}
