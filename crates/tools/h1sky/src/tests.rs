//! Conversion tests on invented bytes: a Halo 1 sky is built in memory,
//! read, converted, installed in a built v8 map and decoded back through
//! [`crate::h2`], and every field Halo 2 has comes back equal. The real
//! `a30`/`bloodgulch` skies are exercised by the ignored disc tests.

use std::io::Cursor;

use h1map::Space;
use map::Map;

use crate::convert;
use crate::h1;
use crate::h2;

/// The address the invented metadata is loaded at.
const BASE: u32 = 0x8040_0000;

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn putf(b: &mut [u8], at: usize, v: f32) {
    put(b, at, v.to_bits());
}

/// Invented sky bytes laid out like the disc: a root at `ROOT_AT`, then one
/// shader-function element, then two lights. Returns the whole buffer and
/// the root's offset in it.
fn invented_sky() -> (Vec<u8>, usize) {
    let root_at = 0x100;
    let mut bytes = vec![0u8; root_at + h1::ROOT_LEN + h1::FUNCTION_LEN + 2 * h1::LIGHT_LEN];
    let root = root_at;
    // Model reference (null) at 0x00, animation (null) at 0x10.
    put(&mut bytes, root, h1map::Group::new(b"mode").0);
    put(&mut bytes, root + 0x0c, 0xffff_ffff);
    put(&mut bytes, root + 0x10, h1map::Group::new(b"antr").0);
    put(&mut bytes, root + 0x1c, 0xffff_ffff);
    // Fog reference (null) at 0x98.
    put(&mut bytes, root + 0x98, h1map::Group::new(b"fog ").0);
    put(&mut bytes, root + 0xa4, 0xffff_ffff);
    // Lighting scalars: first RGB and a fraction, then the two fog runs
    // (colour, two zeros, density, start, opaque distance).
    let scalars = root + h1::LIGHTING_SCALARS;
    for (i, v) in [
        0.3f32, 0.4, 0.5, 0.25, 0.6, 0.7, 0.8, 0.0, 0.0, 0.5, 1.0, 300.0, 0.2, 0.2, 0.3, 0.0,
        0.0, 0.4, 5.0, 120.0,
    ]
    .into_iter()
        .enumerate()
    {
        putf(&mut bytes, scalars + 4 * i, v);
    }
    // Shader-function element right after the root, then the lights.
    let functions = root + h1::ROOT_LEN;
    let lights = functions + h1::FUNCTION_LEN;
    bytes[functions] = 0x5a;
    put(&mut bytes, root + h1::layout::FUNCTIONS, 1);
    put(
        &mut bytes,
        root + h1::layout::FUNCTIONS + 4,
        BASE + functions as u32,
    );
    put(&mut bytes, root + h1::layout::LIGHTS, 2);
    put(
        &mut bytes,
        root + h1::layout::LIGHTS + 4,
        BASE + lights as u32,
    );
    for (i, (yaw, pitch, color, power, cutoff)) in [
        (
            0.174_533,
            0.698_132,
            [0.9f32, 0.89, 0.79],
            1.8f32,
            0.003_490_7,
        ),
        (
            -std::f32::consts::FRAC_PI_6,
            1.741_548,
            [0.70, 0.63, 0.58],
            0.8,
            0.349_066,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let e = lights + i * h1::LIGHT_LEN;
        // Lens flare reference (null).
        put(
            &mut bytes,
            e + h1::light::LENS_FLARE,
            h1map::Group::new(b"lens").0,
        );
        put(&mut bytes, e + h1::light::LENS_FLARE + 12, 0xffff_ffff);
        put(&mut bytes, e + h1::light::CONSTANT, 0x006e_7500);
        put(&mut bytes, e + h1::light::FLAGS, if i == 0 { 3 } else { 1 });
        for (c, v) in color.into_iter().enumerate() {
            putf(&mut bytes, e + h1::light::COLOR + 4 * c, v);
        }
        putf(&mut bytes, e + h1::light::POWER, power);
        putf(&mut bytes, e + h1::light::FALLOFF, 50.0);
        putf(&mut bytes, e + h1::light::YAW, yaw);
        putf(&mut bytes, e + h1::light::PITCH, pitch);
        putf(&mut bytes, e + h1::light::CUTOFF, cutoff);
    }
    (bytes, root_at)
}

fn read_invented() -> h1::Sky {
    let (bytes, root_at) = invented_sky();
    let space = Space::new(&bytes, BASE);
    let sky = h1::Sky::read(&space, &bytes[root_at..root_at + h1::ROOT_LEN]);
    sky.expect("sky reads")
}

#[test]
fn h1_reader_gets_every_field() {
    let sky = read_invented();
    assert_eq!(sky.model.id, h1map::TagId::NONE);
    assert_eq!(sky.animation.id, h1map::TagId::NONE);
    assert_eq!(sky.fog.id, h1map::TagId::NONE);
    assert_eq!(sky.ambient_first_rgb(), [0.3, 0.4, 0.5]);
    assert_eq!(sky.ambient_first_fraction(), 0.25);
    assert_eq!(sky.ambient_second_rgb(), [0.6, 0.7, 0.8]);
    let fog = |color, max_density, start_distance, opaque_distance| h1::Fog {
        color,
        max_density,
        start_distance,
        opaque_distance,
    };
    assert_eq!(
        sky.fogs(),
        [
            Some(fog([0.6, 0.7, 0.8], 0.5, 1.0, 300.0)),
            Some(fog([0.2, 0.2, 0.3], 0.4, 5.0, 120.0)),
        ]
    );
    assert_eq!(sky.shader_functions.len(), 1);
    assert_eq!(sky.shader_functions[0][0], 0x5a);
    assert_eq!(sky.lights.len(), 2);
    let l = &sky.lights[0];
    assert_eq!(l.flags, 3);
    assert_eq!(l.color, [0.9, 0.89, 0.79]);
    assert_eq!(l.power, 1.8);
    assert_eq!(l.falloff_distance, 50.0);
    assert_eq!(l.direction, [0.174_533, 0.698_132]);
    assert_eq!(l.cutoff_angle, 0.003_490_7);
    assert_eq!(sky.lights[1].flags, 1);
    assert_eq!(sky.lights[1].cutoff_angle, 0.349_066);
}

#[test]
fn a_truncated_root_is_refused() {
    let (bytes, root_at) = invented_sky();
    let space = Space::new(&bytes, BASE);
    assert!(
        h1::Sky::read(&space, &bytes[root_at..root_at + 0x60]).is_err(),
        "a 0x60-byte root must not read"
    );
}

#[test]
fn converted_sky_round_trips_through_a_built_map() {
    let sky = read_invented();
    let (bytes, id, model_id) =
        convert::build_sky_map(&sky, None, "levels\\test\\tiny\\tiny sky", 2.5).expect("builds");
    assert!(model_id.is_none());
    let mut map = Map::from_reader(Cursor::new(bytes)).expect("map reads");
    let out = h2::Sky::read_from_map(&mut map, id).expect("decodes");

    // Fields Halo 2 has and the source supplied.
    assert!(out.model.is_null(), "no model in the set");
    assert!(out.unused_ref.is_null());
    assert_eq!(out.word_10, 0);
    assert_eq!(out.model_scale, 2.5, "the scale given");
    assert_eq!(out.camera_translation_scale, 1.0);
    assert_eq!(out.fallback_rgb, sky.ambient_first_rgb());
    assert_eq!(out.word_44, 0.0);
    assert_eq!(out.rotation, 0.0);
    assert!(out.bitmap_layers.is_empty());
    // Halo 1's outdoor fog as fog_a; its indoor fog has no block.
    let f = sky.outdoor_fog().expect("fog A is a fog");
    assert_eq!(out.gradients_a.len(), 1);
    assert_eq!(out.gradients_a[0].color, f.color);
    assert_eq!(out.gradients_a[0].fraction, f.max_density);
    assert_eq!(out.gradients_a[0].tail, [f.start_distance, f.opaque_distance]);
    assert!(sky.indoor_fog().is_some());
    assert!(out.gradients_b.is_empty(), "the indoor fog is not fog_b");
    assert_eq!(out.lights.len(), sky.lights.len());
    for (a, b) in out.lights.iter().zip(&sky.lights) {
        assert_eq!(a.yaw, b.direction[0]);
        assert_eq!(a.pitch, b.direction[1]);
        assert!(a.lens_flare.is_null());
        let p = a.params.as_ref().expect("params element");
        assert_eq!(p.flags, convert::LIGHT_FLAGS);
        assert_eq!(p.color, b.color);
        assert_eq!(p.power, b.power);
        assert_eq!(p.falloff_distance, b.falloff_distance);
        assert_eq!(p.cutoff_angle, b.cutoff_angle);
        assert_eq!(p.word_04, 0);
    }
}

#[test]
fn h2_decoder_follows_blocks_to_foreign_addresses() {
    // The lights block points outside the payload, as three retail skies do
    // (into another tag's payload); decode must follow it through the reader.
    let mut payload = vec![0u8; h2::ROOT_LEN];
    putf(&mut payload, h2::layout::MODEL_SCALE, 400.0);
    putf(&mut payload, h2::layout::CAMERA_TRANSLATION_SCALE, -1.0);
    putf(&mut payload, h2::layout::ROTATION, 0.5);
    for (i, v) in [0.1f32, 0.2, 0.3].into_iter().enumerate() {
        putf(&mut payload, h2::layout::FALLBACK_RGB + 4 * i, v);
    }
    put(&mut payload, h2::layout::WORD_10, 6);
    // A bitmap layer, to exercise that block too.
    put(&mut payload, h2::layout::BITMAP_LAYERS, 1);
    let away = 0x8072_0000u32;
    put(&mut payload, h2::layout::BITMAP_LAYERS + 4, away);
    put(&mut payload, h2::layout::LIGHTS, 1);
    put(
        &mut payload,
        h2::layout::LIGHTS + 4,
        away + h2::BITMAP_LAYER_LEN as u32,
    );
    // The foreign bytes: a bitmap-layer element, a light record and its
    // params element.
    let mut foreign = vec![0u8; h2::BITMAP_LAYER_LEN + h2::LIGHT_LEN + h2::PARAMS_LEN];
    put(&mut foreign, 0, u32::from_be_bytes(*b"bitm"));
    putf(&mut foreign, 8, 1.5);
    let light = away as usize + h2::BITMAP_LAYER_LEN;
    let light_at = h2::BITMAP_LAYER_LEN;
    putf(&mut foreign, light_at + h2::light::YAW, 3.490_659);
    putf(&mut foreign, light_at + h2::light::PITCH, 0.436_332);
    let params_addr = (light + h2::LIGHT_LEN) as u32;
    put(&mut foreign, light_at + h2::light::PARAMS, 1);
    put(&mut foreign, light_at + h2::light::PARAMS + 4, params_addr);
    let p = h2::BITMAP_LAYER_LEN + h2::LIGHT_LEN;
    put(&mut foreign, p + h2::params::FLAGS, 12);
    for (i, v) in [1.0f32, 0.95, 0.87].into_iter().enumerate() {
        putf(&mut foreign, p + h2::params::COLOR + 4 * i, v);
    }
    let mut reads = 0;
    let out = h2::Sky::decode(&payload, 0x8068_2e58, &mut |at, n| {
        reads += 1;
        let start = (at - away) as usize;
        Ok(foreign[start..start + n as usize].to_vec())
    })
    .expect("decodes");
    assert_eq!(reads, 3, "layer, lights and params reads");
    assert_eq!(out.word_10, 6);
    assert_eq!(out.model_scale, 400.0);
    assert_eq!(out.camera_translation_scale, -1.0);
    assert_eq!(out.rotation, 0.5);
    assert_eq!(out.fallback_rgb, [0.1, 0.2, 0.3]);
    assert_eq!(out.bitmap_layers.len(), 1);
    assert_eq!(out.bitmap_layers[0].value, 1.5);
    let l = &out.lights[0];
    assert_eq!(l.yaw, 3.490_659);
    assert_eq!(l.pitch, 0.436_332);
    let p = l.params.as_ref().unwrap();
    assert_eq!(p.flags, 12);
    assert_eq!(p.color, [1.0, 0.95, 0.87]);
}

/// An invented one-part model: a strip of the given points.
fn strip_model(points: &[[f32; 3]]) -> h1models::h1::Model {
    let vertex = |p: [f32; 3]| h1models::h1::Vertex {
        position: p,
        normal: [0.0, 0.0, 1.0],
        binormal: [0.0, 1.0, 0.0],
        tangent: [1.0, 0.0, 0.0],
        texcoord: [0.0; 2],
        nodes: [0, -1],
        weights: [1.0, 0.0],
    };
    h1models::h1::Model {
        flags: 0,
        node_list_checksum: 0,
        lod_cutoffs: [0.0; 5],
        base_map_scale: [1.0; 2],
        markers: Vec::new(),
        nodes: Vec::new(),
        regions: Vec::new(),
        geometries: vec![h1models::h1::Geometry {
            flags: 0,
            parts: vec![h1models::h1::Part {
                flags: 0,
                shader: 0,
                centroid_nodes: [0, -1],
                centroid_weights: [1.0, 0.0],
                centroid: [0.0; 3],
                vertices: points.iter().copied().map(vertex).collect(),
                strip: (0..points.len() as u16).collect(),
            }],
        }],
        shaders: Vec::new(),
    }
}

/// A ground plate 100 units under the camera, as a30's sky has, and a far
/// dome triangle: the plate decides, at its nearest point (straight down).
#[test]
fn the_nearest_sky_surface_is_the_closest_point_of_any_triangle() {
    let m = strip_model(&[
        [-1000.0, -1000.0, -100.0],
        [1000.0, -1000.0, -100.0],
        [-1000.0, 1000.0, -100.0],
        [1000.0, 1000.0, -100.0],
    ]);
    let d = convert::nearest_surface(&m).expect("a surface");
    assert!((d - 100.0).abs() < 1e-3, "{d}");
    // Past a vertex: the closest point is the corner.
    let corner = strip_model(&[
        [300.0, 400.0, 0.5],
        [900.0, 400.0, 0.5],
        [300.0, 900.0, 10.0],
    ]);
    let d = convert::nearest_surface(&corner).expect("a surface");
    assert!(
        (d - (300.0f32 * 300.0 + 400.0 * 400.0 + 0.25).sqrt()).abs() < 1e-2,
        "{d}"
    );
}

/// A triangle in a plane through the camera is a line on screen at any
/// scale: it doesn't pull the sky back.
#[test]
fn edge_on_triangles_do_not_bound_the_scale() {
    let m = strip_model(&[[10.0, -500.0, 0.0], [10.0, 500.0, 0.0], [5000.0, 0.0, 0.0]]);
    assert_eq!(convert::nearest_surface(&m), None);
    assert_eq!(convert::model_scale(None, 800.0), 1.0);
}

/// A card in a plane through the sky's origin, spanning it, between two
/// far dome triangles: Halo 1 sees it edge-on, but in Halo 2 the eye sits
/// a hair off that plane and the card is a wall at that distance, over the
/// whole level. It is dropped; the dome triangles stay, wound as they were.
#[test]
fn edge_on_triangles_are_dropped_from_the_sky_model() {
    let mut m = strip_model(&[
        // A dome triangle in the plane x = 5000.
        [5000.0, -500.0, 100.0],
        [5000.0, 500.0, 100.0],
        [5000.0, 0.0, 900.0],
        // The card, in the plane z = 0, around the origin.
        [-3000.0, -3000.0, 0.0],
        [3000.0, -3000.0, 0.0],
        [0.0, 3000.0, 0.0],
        // A dome triangle in the plane z = 3000.
        [-4000.0, -4000.0, 3000.0],
        [4000.0, -4000.0, 3000.0],
        [0.0, 4000.0, 3000.0],
    ]);
    let tris = [[0, 1, 2], [3, 4, 5], [6, 7, 8]];
    m.geometries[0].parts[0].strip = h1models::h1::triangles_strip(&tris);
    let nearest = convert::nearest_surface(&m).expect("a surface");
    assert!((nearest - 3000.0).abs() < 1e-2, "{nearest}");
    assert_eq!(convert::drop_edge_on(&mut m), 1);
    assert_eq!(
        h1models::h1::strip_triangles(&m.geometries[0].parts[0].strip),
        vec![[0, 1, 2], [6, 7, 8]]
    );
    assert_eq!(
        convert::nearest_surface(&m),
        Some(nearest),
        "the scale is unchanged"
    );
    assert_eq!(convert::drop_edge_on(&mut m), 0, "nothing left to drop");
}

/// A part whose every triangle is edge-on goes; a geometry keeps its last
/// part (with no triangle) rather than none.
#[test]
fn a_part_left_empty_goes_but_not_a_geometrys_last() {
    let mut m = strip_model(&[[10.0, -500.0, 0.0], [10.0, 500.0, 0.0], [5000.0, 0.0, 0.0]]);
    let mut far = m.geometries[0].parts[0].clone();
    for v in &mut far.vertices {
        v.position[2] = 2000.0;
    }
    m.geometries[0].parts.push(far);
    m.geometries.push(m.geometries[0].clone());
    m.geometries[1].parts.truncate(1);
    assert_eq!(convert::drop_edge_on(&mut m), 2);
    assert_eq!(m.geometries[0].parts.len(), 1);
    assert_eq!(m.geometries[0].parts[0].vertices[0].position[2], 2000.0);
    assert_eq!(m.geometries[1].parts.len(), 1);
    assert!(m.geometries[1].parts[0].strip.is_empty());
}

/// The scale moves the nearest surface out to the level's reach, and never
/// shrinks a sky that is already farther.
#[test]
fn the_model_scale_puts_the_sky_past_the_level() {
    let s = convert::model_scale(Some(100.0), 840.0);
    assert!((s * 100.0 - 840.0).abs() < 1e-3, "{s}");
    assert_eq!(convert::model_scale(Some(5613.0), 200.0), 1.0);
    assert_eq!(convert::model_scale(Some(0.0), 200.0), 1.0);
}

#[test]
fn converted_lights_name_the_flares_given() {
    let sky = read_invented();
    let flare = map::TagId(0xe123_0042);
    let c = convert::convert_sky_with_flares(&sky, None, 1.0, &[None, Some(flare)]);
    let lights = c
        .root
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            map::build::FixupKind::Block { child, .. }
                if f.offset == h2::layout::LIGHTS as u32 =>
            {
                Some(child)
            }
            _ => None,
        })
        .expect("lights block");
    let flare_of = |k: usize| {
        lights
            .fixups
            .iter()
            .find_map(|f| match f.kind {
                map::build::FixupKind::TagRef(id)
                    if f.offset == (k * h2::LIGHT_LEN + h2::light::LENS_FLARE) as u32 =>
                {
                    Some(id)
                }
                _ => None,
            })
            // A null reference built from the definition carries no fixup.
            .unwrap_or(map::TagId::NULL)
    };
    assert_eq!(flare_of(0), map::TagId::NULL);
    assert_eq!(flare_of(1), flare);
}

/// a10's and the menu's skies: fog A black, density 1, start and opaque
/// distance 0. Halo 2 would raise the opaque distance to 0.0001 and fog
/// everything black (E 0x001301c0); Halo 1 draws no fog for it.
#[test]
fn a_fog_opaque_at_its_start_is_no_fog() {
    let mut sky = read_invented();
    let a = h1::FOG_RUNS[0];
    sky.lighting_scalars[a..a + 8].copy_from_slice(&[0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert!(sky.fogs()[0].is_some(), "the run has a density");
    assert_eq!(sky.outdoor_fog(), None);
    let (bytes, id, _) =
        convert::build_sky_map(&sky, None, "levels\\test\\tiny\\tiny sky", 1.0).expect("builds");
    let mut map = Map::from_reader(Cursor::new(bytes)).expect("map reads");
    let out = h2::Sky::read_from_map(&mut map, id).expect("decodes");
    assert!(out.gradients_a.is_empty());
    assert!(out.gradients_b.is_empty());
}

/// Blood Gulch's sky: a warm outdoor haze and Halo 1's untuned purple
/// indoor fog (density 1, opaque at 120). Only the haze converts.
#[test]
fn only_the_outdoor_fog_converts() {
    let mut sky = read_invented();
    sky.lighting_scalars[4..20].copy_from_slice(&[
        1.0, 0.9, 0.77, 0.0, 0.0, 0.4, 3.0, 100.0, 0.196, 0.172, 0.329, 0.0, 0.0, 1.0, 1.0, 120.0,
    ]);
    let (bytes, id, _) =
        convert::build_sky_map(&sky, None, "levels\\test\\tiny\\tiny sky", 1.0).expect("builds");
    let mut map = Map::from_reader(Cursor::new(bytes)).expect("map reads");
    let out = h2::Sky::read_from_map(&mut map, id).expect("decodes");
    assert_eq!(out.gradients_a.len(), 1);
    assert_eq!(out.gradients_a[0].color, [1.0, 0.9, 0.77]);
    assert_eq!(out.gradients_a[0].fraction, 0.4);
    assert_eq!(out.gradients_a[0].tail, [3.0, 100.0]);
    assert!(out.gradients_b.is_empty());
}

/// c20's library fog: blue, density 0.1, 10 to 30 units. It goes into an
/// atmosphere element's first fog, where Halo 2 reads the sky's `fog_a`
/// (E `0x0012efb0`), with nothing else set.
#[test]
fn the_indoor_fog_is_an_atmosphere_element_s_first_fog() {
    use crate::atmosphere::{self, layout};
    let mut sky = read_invented();
    sky.lighting_scalars[12..20]
        .copy_from_slice(&[0.06, 0.07, 0.53, 0.0, 0.0, 0.1, 10.0, 30.0]);
    let fog = sky.indoor_fog().expect("fog B has a density");
    let e = atmosphere::element(&fog);
    assert_eq!(e.len(), atmosphere::LEN);
    assert_eq!(atmosphere::fog(&e), Some(fog));
    let f = |at: usize| f32::from_le_bytes(e[at..at + 4].try_into().unwrap());
    assert_eq!([f(4), f(8), f(0xc)], [0.06, 0.07, 0.53]);
    assert_eq!([f(0x18), f(0x1c), f(0x20)], [0.1, 10.0, 30.0]);
    assert_eq!(f(layout::FADE_DISTANCE), 0.0, "the engine's default fade");
    // No second fog, no patchy fog: only the null `fpch` reference.
    let set: Vec<usize> = (0..atmosphere::LEN)
        .filter(|&k| e[k] != 0 && !(4..0x24).contains(&k))
        .collect();
    assert_eq!(set, (0xd0..0xd8).collect::<Vec<_>>());
    assert_eq!(&e[0xd0..0xd4], b"hcpf", "the group, as retail stores it");
    assert_eq!(&e[0xd4..0xd8], [0xff; 4]);
    // No density, no indoor fog.
    sky.lighting_scalars[17] = 0.0;
    assert_eq!(sky.indoor_fog(), None);
}
