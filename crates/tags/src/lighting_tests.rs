//! Invented fixtures only. No disc bytes or asset names.
use crate::{
    Error,
    geometry::{
        BlockResource, GeometryBlockInfo, LoadedBlock, ResourceKind, Section, TagView,
        decode_section,
    },
    lighting::*,
    structure::{BUCKET_SECTION, Lightmap},
};
use map::{FourCC, TagId};
const BASE: u32 = 0x8100_0000;
fn put(b: &mut [u8], offset: usize, n: u32) {
    b[offset..offset + 4].copy_from_slice(&n.to_le_bytes());
}
fn block(b: &mut [u8], field: usize, count: u32, offset: usize) {
    put(b, field, count);
    put(b, field + 4, BASE + offset as u32);
}
fn view(bytes: &[u8]) -> TagView<'_> {
    TagView::new(bytes, BASE)
}
fn lightmap(groups: usize, extra: usize) -> Vec<u8> {
    let mut b = vec![0; 0x104 + groups * 0x68 + extra];
    block(&mut b, 0x80, groups as u32, 0x104);
    for i in 0..groups {
        put(&mut b, 0x104 + i * 0x68 + 0x1c, u32::MAX);
    }
    b
}
#[test]
fn groups_use_engine_stride_and_keep_probe_coefficients() {
    let mut b = lightmap(2, 0xdc);
    let probe = 0x104 + 2 * 0x68;
    put(&mut b, 0x104 + 4, 0x1234);
    put(&mut b, 0x104 + 0x68 + 4, 0x5678);
    block(&mut b, 0x104 + 0x68 + 0x38, 1, probe);
    put(&mut b, probe, 4.5f32.to_bits());
    put(&mut b, probe + 0xc, 2.25f32.to_bits());
    put(&mut b, probe + 0x30 + 32, (-0.75f32).to_bits());
    put(&mut b, probe + 0xa4, 1f32.to_bits());
    let l = LightmapLighting::decode(TagId(1), &view(&b)).unwrap();
    assert_eq!(l.linkage.groups[1].structure_checksum, 0x5678);
    assert!(l.probes[0].is_empty());
    let p = &l.probes[1][0];
    assert_eq!(p.position, [4.5, 0.0, 0.0]);
    assert_eq!(p.coefficients[0][0], 2.25);
    assert_eq!(p.coefficients[1][8], -0.75);
    assert_eq!(p.interpolated_direction, [1.0, 0.0, 0.0]);
    assert_eq!(&p.raw[..], &b[probe..probe + 0xdc]);
}
#[test]
fn repeated_palette_aliases_are_bounded_before_linkage_allocation() {
    let mut b = lightmap(10, 0x400);
    let palette = 0x104 + 10 * 0x68;
    for i in 0..10 {
        block(&mut b, 0x104 + i * 0x68 + 8, 1, palette);
    }
    assert!(
        LightmapLighting::decode(TagId(1), &view(&b))
            .unwrap_err()
            .to_string()
            .contains("budget")
    );
    // A small alias is allowed; it is charged for each logical decoded copy,
    // so it fits only while the copies fit the payload length. The 0x104-byte
    // header is not a block, so this payload is exactly the charged size.
    let mut b = lightmap(2, 0x800 - 0x104);
    let palette = 0x104 + 2 * 0x68;
    for i in 0..2 {
        block(&mut b, 0x104 + i * 0x68 + 8, 1, palette);
    }
    assert!(LightmapLighting::decode(TagId(1), &view(&b)).is_ok());
    b.truncate(b.len() - 4);
    assert!(matches!(
        LightmapLighting::decode(TagId(1), &view(&b)),
        Err(Error::Invalid("tag blocks exceed the payload budget"))
    ));
}
#[test]
fn public_lightmap_decoder_enforces_the_shared_alias_budget() {
    // 128 groups all naming one table of 256 palettes: a ~270 KiB payload
    // that would decode to 32 MiB of palettes through any caller.
    let groups = 128;
    let mut b = lightmap(groups, 256 * 0x400);
    let palettes = 0x104 + groups * 0x68;
    for i in 0..groups {
        block(&mut b, 0x104 + i * 0x68 + 8, 256, palettes);
    }
    for r in [
        Lightmap::decode(TagId(1), &view(&b)).map(|_| ()),
        LightmapLighting::decode(TagId(1), &view(&b)).map(|_| ()),
    ] {
        assert!(matches!(
            r,
            Err(Error::Invalid("tag blocks exceed the payload budget"))
        ));
    }
    // One reference to the same table is fine.
    for i in 1..groups {
        put(&mut b, 0x104 + i * 0x68 + 8, 0);
    }
    assert_eq!(
        Lightmap::decode(TagId(1), &view(&b)).unwrap().groups[0]
            .palettes
            .len(),
        256
    );
}
#[test]
fn probe_blocks_share_the_linkage_budget() {
    // Three groups whose probe blocks name one probe: the third copy exceeds
    // the payload once the linkage itself has been charged.
    let mut b = lightmap(3, 0xdc);
    let probe = 0x104 + 3 * 0x68;
    for i in 0..3 {
        block(&mut b, 0x104 + i * 0x68 + 0x38, 1, probe);
    }
    assert!(matches!(
        LightmapLighting::decode(TagId(1), &view(&b)),
        Err(Error::Invalid("tag blocks exceed the payload budget"))
    ));
}
#[test]
fn reviewed_pointer_corruption_and_nonfinite_values_fail() {
    let mut b = lightmap(1, 0xdc);
    let probe = 0x104 + 0x68;
    block(&mut b, 0x104 + 0x38, 1, probe);
    let good = b.clone();
    for pointer in [
        0,
        u32::MAX,
        BASE - 4,
        BASE + probe as u32 + 1,
        BASE + b.len() as u32,
    ] {
        let mut bad = good.clone();
        put(&mut bad, 0x104 + 0x3c, pointer);
        assert!(LightmapLighting::decode(TagId(1), &view(&bad)).is_err());
    }
    put(&mut b, probe + 0x54, f32::NAN.to_bits());
    assert!(LightmapLighting::decode(TagId(1), &view(&b)).is_err());
}
#[test]
fn empty_descriptors_do_not_dereference_stale_pointers() {
    let mut b = lightmap(1, 0);
    put(&mut b, 0x104 + 0x3c, u32::MAX);
    let l = LightmapLighting::decode(TagId(1), &view(&b)).unwrap();
    assert!(l.probes[0].is_empty());
}
#[test]
fn palette_words_are_argb_and_alpha_is_retained() {
    let mut p = [0; 256];
    p[17] = 0x4723_91e8;
    assert_eq!(palette_rgba(&p, 17), [0x23, 0x91, 0xe8, 0x47]);
    assert_eq!(palette_rgba(&p, 0), [0, 0, 0, 0]);
}
#[test]
fn sound_environment_decodes_float_parameters_and_retains_opaque_bits() {
    let mut b = vec![0xff; 0x48];
    for i in 0..12 {
        put(&mut b, 8 + i * 4, (i as f32 - 8.5).to_bits());
    }
    let Atmosphere::SoundEnvironment(s) = Atmosphere::decode(FourCC(*b"snde"), &view(&b)).unwrap()
    else {
        panic!()
    };
    assert_eq!(s.room_db, -8.5);
    assert_eq!(s.decay_seconds, -5.5);
    assert_eq!(s.hf_reference_hz, 2.5);
    assert_eq!(s.opaque, b);
    put(&mut b, 0x14, f32::INFINITY.to_bits());
    assert!(Atmosphere::decode(FourCC(*b"snde"), &view(&b)).is_err());
}
#[test]
fn sky_lights_are_bounded_and_opaque_values_are_not_interpreted() {
    let mut b = vec![0; 0xac + 0x34];
    put(&mut b, 4, u32::MAX);
    block(&mut b, 0x78, 1, 0xac);
    put(&mut b, 0x34, 0.25f32.to_bits());
    put(&mut b, 0xac + 0xc, 1.5f32.to_bits());
    put(&mut b, 0xac + 0x14, FourCC(*b"lens").as_u32());
    put(&mut b, 0xac + 0x18, 0xe111_0002);
    // An unreviewed descriptor carries an invalid pointer; don't follow it.
    block(&mut b, 0x48, 0xffffffff, 0xdead);
    let Atmosphere::Sky(s) = Atmosphere::decode(FourCC(*b"sky "), &view(&b)).unwrap() else {
        panic!()
    };
    assert_eq!(s.lights[0].yaw_pitch[0], 1.5);
    assert_eq!(s.lights[0].lens_flare.id, TagId(0xe111_0002));
    assert_eq!(s.fallback_rgb, [0.25, 0.0, 0.0]);
    assert_eq!(s.opaque, b);
    put(&mut b, 0x78, u32::MAX);
    assert!(Atmosphere::decode(FourCC(*b"sky "), &view(&b)).is_err());
}
#[test]
fn optical_fog_fields_remain_opaque_while_references_are_retained() {
    let mut b = vec![0xff; 0x60];
    put(&mut b, 0x40, FourCC(*b"snde").as_u32());
    put(&mut b, 0x44, 0xe333_0020);
    let a = Atmosphere::decode(FourCC(*b"fog "), &view(&b)).unwrap();
    assert_eq!(a.references()[1].id, TagId(0xe333_0020));
    assert_eq!(a.opaque_bytes(), b.len());
    for group in [FourCC(*b"gldf"), FourCC(*b"weat")] {
        let a = Atmosphere::decode(group, &view(&b)).unwrap();
        assert!(a.references().is_empty());
        assert_eq!(a.opaque_bytes(), 0x60);
    }
}
#[test]
fn light_volume_layer_stride_and_fields() {
    let mut b = vec![0; 0x10 + 2 * 0x98];
    for i in 0..2 {
        put(&mut b, 0x10 + i * 0x98 + 8, u32::MAX);
    }
    block(&mut b, 8, 2, 0x10);
    put(&mut b, 0x10, 0x28);
    put(&mut b, 0x10 + 0xc, 7);
    put(&mut b, 0x10 + 0x98 + 0xc, 23);
    let Atmosphere::LightVolume(v) = Atmosphere::decode(FourCC(*b"MGS2"), &view(&b)).unwrap()
    else {
        panic!()
    };
    assert_eq!(v.layers.len(), 2);
    assert_eq!(v.layers[0].flags, 0x28);
    assert_eq!(v.layers[1].sample_count, 23);
    b.pop();
    assert!(Atmosphere::decode(FourCC(*b"MGS2"), &view(&b)).is_err());
}
#[test]
fn known_fixed_prefixes_reject_truncation() {
    for (g, n) in [
        (b"sky ", 0xac),
        (b"fog ", 0x60),
        (b"fpch", 0x50),
        (b"MGS2", 0x10),
        (b"snde", 0x48),
    ] {
        for end in 0..n {
            assert!(Atmosphere::decode(FourCC(*g), &view(&vec![0; end])).is_err());
        }
    }
    assert!(Atmosphere::decode(FourCC(*b"nope"), &view(&[])).is_err());
}
/// An invented framed bucket block: one descriptor at field 0 and one
/// kind-2 vertex resource per entry of `data` (descriptor index, bytes).
fn bucket_block(type_id: u8, stride: u8, data: &[(u16, Vec<u8>)]) -> crate::Result<LoadedBlock> {
    let mut desc = vec![0u8; 0x20];
    desc[0] = type_id;
    desc[1] = stride;
    let mut entries = vec![(ResourceKind::Block, 0x20u16, desc)];
    entries.extend(
        data.iter()
            .map(|(i, d)| (ResourceKind::VertexBuffer, *i, d.clone())),
    );
    let mut area = Vec::new();
    let mut resources = Vec::new();
    for (kind, secondary, bytes) in entries {
        area.extend(b"crsr");
        resources.push(BlockResource {
            kind,
            unknown_1: 2,
            unknown_2: 0,
            field_offset: 0,
            secondary,
            size: bytes.len() as u32,
            offset: Some(area.len() as u32),
        });
        area.extend(bytes);
    }
    let mut bytes = b"hklb".to_vec();
    bytes.extend((area.len() as u32).to_le_bytes());
    bytes.extend(1u32.to_le_bytes()); // section: one descriptor at field 0
    bytes.extend(0u32.to_le_bytes());
    bytes.extend(&area);
    bytes.extend(b"fklb");
    let info = GeometryBlockInfo {
        block: None,
        block_size: bytes.len() as u32,
        section_size: 8,
        resource_size: area.len() as u32,
        resources,
        owner: TagId(1),
        runtime_block_offset: 0x24,
        runtime_loaded: 0,
        unknown_1f: 1,
        runtime_cache_handle: u32::MAX,
    };
    LoadedBlock::parse(&info, bytes)
}
fn bucket(type_id: u8, stride: u8, data: Vec<u8>) -> crate::Result<Section> {
    decode_section(
        bucket_block(type_id, stride, &[(0, data)])?,
        &BUCKET_SECTION,
        None,
    )
}
fn lighting(type_id: u8, stride: u8, data: Vec<u8>) -> crate::Result<Option<VertexLighting>> {
    let s = bucket(type_id, stride, data)?;
    vertex_lighting(&s, &s.vertex_buffers[0])
}
#[test]
fn bucket_rgb_has_distinct_byte_and_word_orders() {
    let Some(VertexLighting::Color(c)) = lighting(0x2f, 3, vec![255, 0, 128]).unwrap() else {
        panic!()
    };
    assert_eq!(c, vec![[1.0, 0.0, 128.0 / 255.0]]);
    let Some(VertexLighting::Color(c)) = lighting(0x2e, 4, vec![128, 0, 255, 17]).unwrap() else {
        panic!()
    };
    assert_eq!(c, vec![[1.0, 0.0, 128.0 / 255.0]]);
    assert!(lighting(0x2f, 4, vec![0; 4]).is_err());
    assert!(lighting(0x2f, 3, vec![0; 4]).is_err());
    assert!(lighting(0x99, 1, vec![0]).unwrap().is_none());
}
#[test]
fn duplicate_bucket_vertex_resources_are_rejected() {
    // The engine fixup (0x00223c70) applies resources in order, so a repeated
    // (field, index) key would make the last one win. Reject it instead of
    // choosing either: here red, then blue, for descriptor 0.
    let r = bucket_block(0x2f, 3, &[(0, vec![255, 0, 0]), (0, vec![0, 0, 255])]);
    assert!(matches!(r, Err(Error::Invalid("duplicate block resource"))));
    // Distinct keys are fine; an index with no descriptor is not.
    let b = bucket_block(0x2f, 3, &[(0, vec![255, 0, 0]), (1, vec![0, 0, 255])]).unwrap();
    assert!(matches!(
        decode_section(b, &BUCKET_SECTION, None),
        Err(Error::Invalid("vertex resource has no descriptor"))
    ));
}
#[test]
fn lighting_vector_uses_midpoint_quantization_and_is_not_normalized() {
    let max = 1023 | (1023 << 11) | (511 << 22);
    let min = 1024 | (1024 << 11) | (512 << 22);
    assert_eq!(unpack_lighting_vector(max), [1.0; 3]);
    assert_eq!(unpack_lighting_vector(min), [-1.0; 3]);
    assert_eq!(
        unpack_lighting_vector(0),
        [1.0 / 2047.0, 1.0 / 2047.0, 1.0 / 1023.0]
    );
    let Some(VertexLighting::Vector(v)) = lighting(0x30, 4, max.to_le_bytes().to_vec()).unwrap()
    else {
        panic!()
    };
    assert_eq!(v, vec![[1.0; 3]]);
}

#[test]
fn sh_term_order_and_ambient_contribution_follow_the_engine() {
    let z = sh_basis([0.0, 0.0, 1.0]);
    assert!((z[0] - 0.2820948).abs() < 1e-6);
    assert!((z[2] - 0.4886025).abs() < 1e-6);
    assert!((z[6] - 0.6307831).abs() < 1e-6);
    assert_eq!(z[1], 0.0);
    assert_eq!(z[3], 0.0);
    assert!(sh_basis([1.0, 0.0, 0.0])[3] < 0.0);
    assert!(sh_basis([0.0, 1.0, 0.0])[1] < 0.0);
    assert!(sh_basis([0.5, 0.5, 0.5])[4] > 0.0);
    assert!(sh_basis([0.5, 0.5, 0.5])[5] < 0.0);
    assert!(sh_basis([0.5, 0.5, 0.5])[7] < 0.0);
    let mut p = LightProbe {
        position: [0.0; 3],
        coefficients: [[0.0; 9]; 3],
        rotation: 0.0,
        interpolated_direction: [0.0; 3],
        raw: [0; 0xdc],
    };
    p.coefficients[0][0] = 2.0;
    p.coefficients[1][2] = 3.0;
    p.coefficients[2][3] = 4.0;
    assert_eq!(p.ambient_rgb(), [2.0 * z[0], 0.0, 0.0]);
    assert_eq!(p.evaluate([0.0, 0.0, 1.0]), [2.0 * z[0], 3.0 * z[2], 0.0]);
    assert!(p.evaluate([1.0, 0.0, 0.0])[2] < 0.0); // Don't silently clamp.
}

/// Six invented tags: globals, scenario, two skies and two identities of one fog.
fn alias_map() -> Vec<u8> {
    let mut b = vec![0u8; 0x3000];
    b[..4].copy_from_slice(b"daeh");
    b[0x7fc..0x800].copy_from_slice(b"toof");
    b[0x120..0x12e].copy_from_slice(b"02.09.27.09809");
    for (o, v) in [
        (4, 8),
        (8, 0x3000),
        (0x10, 0x800),
        (0x14, 0x800),
        (0x18, 0x2000),
        (0x1c, 0x2000),
    ] {
        put(&mut b, o, v);
    }
    let classes = [*b"matg", *b"scnr", *b"sky ", *b"fog "];
    for (i, g) in classes.iter().enumerate() {
        put(&mut b, 0x820 + i * 12, FourCC(*g).as_u32());
        put(&mut b, 0x824 + i * 12, u32::MAX);
        put(&mut b, 0x828 + i * 12, u32::MAX);
    }
    for (o, v) in [
        (0, 0x80000020),
        (4, 4),
        (8, 0x80000050),
        (12, 0xa0000001),
        (16, 0xa0000000),
        (24, 6),
        (28, FourCC(*b"tags").as_u32()),
    ] {
        put(&mut b, 0x800 + o, v);
    }
    for (i, (g, offset, size)) in [
        (*b"matg", 0, 0x10),
        (*b"scnr", 0x100, 0x218),
        (*b"sky ", 0x400, 0xac),
        (*b"sky ", 0x600, 0xe0),
        (*b"fog ", 0x800, 0x60),
        (*b"fog ", 0x800, 0),
    ]
    .iter()
    .enumerate()
    {
        let row = 0x850 + i * 16;
        put(&mut b, row, FourCC(*g).as_u32());
        put(&mut b, row + 4, 0xa0000000 + i as u32);
        put(&mut b, row + 8, BASE + offset);
        put(&mut b, row + 12, *size);
    }
    for root in [0x1400, 0x1600] {
        put(&mut b, root + 4, u32::MAX);
    }
    block(&mut b, 0x1400 + 0x78, 1, 0x6ac);
    block(&mut b, 0x1600 + 0x78, 1, 0x6ac);
    put(&mut b, 0x16ac + 0x18, u32::MAX);
    put(&mut b, 0x16ac + 0xc, 0.75f32.to_bits());
    for o in [0x3c, 0x44, 0x54, 0x5c] {
        put(&mut b, 0x1800 + o, u32::MAX);
    }
    b
}
fn map_fixture(bytes: Vec<u8>) -> map::Map<std::io::Cursor<Vec<u8>>> {
    map::Map::from_reader(std::io::Cursor::new(bytes)).unwrap()
}
#[test]
fn sky_children_and_zero_sized_fog_roots_can_alias_same_class_payloads() {
    let mut m = map_fixture(alias_map());
    let Atmosphere::Sky(s) = Atmosphere::read(&mut m, TagId(0xa0000002)).unwrap() else {
        panic!()
    };
    assert!(s.lights_alias);
    assert_eq!(s.lights[0].yaw_pitch[0], 0.75);
    assert_eq!(s.opaque.len(), 0xac);
    let Atmosphere::Sky(s) = Atmosphere::read(&mut m, TagId(0xa0000003)).unwrap() else {
        panic!()
    };
    assert!(!s.lights_alias);
    let a = Atmosphere::read(&mut m, TagId(0xa0000005)).unwrap();
    assert_eq!(a.opaque_bytes(), 0x60);
}
#[test]
fn alias_resolution_rejects_unowned_wrong_class_or_out_of_space_ranges() {
    for pointer in [BASE + 0x1200, BASE + 0x800, BASE + 0x2000, BASE + 0x6ad] {
        let mut b = alias_map();
        put(&mut b, 0x1400 + 0x7c, pointer);
        assert!(Atmosphere::read(&mut map_fixture(b), TagId(0xa0000002)).is_err());
    }
    let mut b = alias_map();
    put(&mut b, 0x1400 + 0x78, 2);
    assert!(Atmosphere::read(&mut map_fixture(b), TagId(0xa0000002)).is_err());
    let mut b = alias_map();
    put(&mut b, 0x850 + 5 * 16 + 8, BASE + 0xa00);
    assert!(Atmosphere::read(&mut map_fixture(b), TagId(0xa0000005)).is_err());
    let mut b = alias_map();
    put(&mut b, 0x1400 + 0x78, u32::MAX);
    assert!(Atmosphere::read(&mut map_fixture(b), TagId(0xa0000002)).is_err());
}

/// Expected basis values for two mixed-axis unit directions, computed
/// independently in f64 from the closed forms (1/(2*sqrt(pi)), sqrt(3)/(2*sqrt(pi)),
/// sqrt(15)/(2*sqrt(pi)), sqrt(5)/(4*sqrt(pi)), sqrt(15)/(4*sqrt(pi))) with the
/// engine signs of 0x00143360. Every term is nonzero in at least one row.
const SH_CASES: [([f32; 3], [f32; 9]); 2] = [
    (
        [2.0 / 7.0, 3.0 / 7.0, 6.0 / 7.0],
        [
            0.2820948, -0.2094011, 0.4188022, -0.1396007, 0.1337814, -0.4013443, 0.3797572,
            -0.2675629, -0.0557423,
        ],
    ),
    (
        [-0.6, 0.0, 0.8],
        [
            0.2820948, 0.0, 0.390882, 0.2931615, 0.0, 0.0, 0.2901602, 0.5244232, 0.1966587,
        ],
    ),
];
#[test]
fn every_sh_term_matches_independent_values_at_mixed_directions() {
    for (d, expected) in SH_CASES {
        let got = sh_basis(d);
        for (i, (g, e)) in got.iter().zip(expected).enumerate() {
            assert!((g - e).abs() < 2e-6, "term {i} at {d:?}: {g} != {e}");
        }
    }
    // A basis vector per term: evaluation is the plain nine-term dot product
    // per channel, with no clamping, so each coefficient lands on its term.
    let (d, expected) = SH_CASES[0];
    for (term, e) in expected.into_iter().enumerate() {
        let mut p = LightProbe {
            position: [0.0; 3],
            coefficients: [[0.0; 9]; 3],
            rotation: 0.0,
            interpolated_direction: [0.0; 3],
            raw: [0; 0xdc],
        };
        p.coefficients[0][term] = 1.0;
        p.coefficients[1][term] = -2.0;
        p.coefficients[2][term] = 0.5;
        let rgb = p.evaluate(d);
        for (c, (got, want)) in rgb.into_iter().zip([e, -2.0 * e, 0.5 * e]).enumerate() {
            assert!((got - want).abs() < 4e-6, "term {term} channel {c}");
        }
    }
}
#[test]
fn sky_scalar_fields_come_from_their_own_offsets() {
    // Distinct sentinels in every reviewed scalar and its neighbours, so a
    // shifted read picks up the wrong value.
    let mut b = vec![0; 0xac];
    put(&mut b, 4, u32::MAX);
    for (o, v) in [
        (0x10, 9.0f32),
        (0x14, 1.25),
        (0x18, 2.5),
        (0x1c, 9.5),
        (0x30, 8.0),
        (0x34, 0.125),
        (0x38, 0.25),
        (0x3c, 0.375),
        (0x40, 7.0),
        (0x7c, 0.0),
        (0x80, 3.75),
        (0x84, -6.5),
    ] {
        put(&mut b, o, v.to_bits());
    }
    let Atmosphere::Sky(s) = Atmosphere::decode(FourCC(*b"sky "), &view(&b)).unwrap() else {
        panic!()
    };
    assert_eq!(s.scale, 1.25);
    assert_eq!(s.camera_translation_scale, 2.5);
    assert_eq!(s.fallback_rgb, [0.125, 0.25, 0.375]);
    assert_eq!(s.rotation, 3.75);
}
#[test]
fn light_volume_fade_is_camera_distance_not_height() {
    let mut b = vec![0; 0x10];
    put(&mut b, 0, 0f32.to_bits());
    put(&mut b, 4, 10f32.to_bits());
    let Atmosphere::LightVolume(v) = Atmosphere::decode(FourCC(*b"MGS2"), &view(&b)).unwrap()
    else {
        panic!()
    };
    assert_eq!(v.distance_fade, [0.0, 10.0]);
    // A purely horizontal displacement of 6 fades by distance: 0.4. A
    // relative-height reading would give 1.0.
    assert!((v.distance_fade_factor([6.0, 0.0, 0.0]) - 0.4).abs() < 1e-6);
    assert!((v.distance_fade_factor([0.0, 3.0, 4.0]) - 0.5).abs() < 1e-6);
    assert_eq!(v.distance_fade_factor([0.0, 0.0, 0.0]), 1.0);
    assert_eq!(v.distance_fade_factor([20.0, 0.0, 0.0]), 0.0);
    // A nonzero near bound: (10 - 6) / (10 - 2).
    let mut v = v;
    v.distance_fade = [2.0, 10.0];
    assert!((v.distance_fade_factor([0.0, -6.0, 0.0]) - 0.5).abs() < 1e-6);
    // An empty or inverted interval keeps the engine's initial 1.0.
    v.distance_fade = [10.0, 10.0];
    assert_eq!(v.distance_fade_factor([50.0, 0.0, 0.0]), 1.0);
}
