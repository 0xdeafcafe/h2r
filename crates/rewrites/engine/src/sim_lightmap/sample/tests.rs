use super::*;
use std::collections::HashMap;

/// Guest memory as a byte map (unwritten bytes read 0), with the store's
/// answers beside it.
#[derive(Default)]
struct Flat {
    bytes: HashMap<u32, u8>,
    sections: HashMap<u32, u32>,
    pixels: HashMap<u32, u32>,
}

impl Flat {
    fn w(&mut self, a: u32, v: u32) {
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.bytes.insert(a + i as u32, b);
        }
    }
    fn h(&mut self, a: u32, v: u16) {
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.bytes.insert(a + i as u32, b);
        }
    }
    fn b(&mut self, a: u32, v: u8) {
        self.bytes.insert(a, v);
    }
    fn f(&mut self, a: u32, v: f32) {
        self.w(a, v.to_bits());
    }
    /// Tag `datum`'s address in the tag table.
    fn tag(&mut self, datum: u32, addr: u32) {
        self.w(TABLE + datum * 16 + 8, addr);
    }
}

impl Source for Flat {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), GuestFault> {
        for (i, d) in dst.iter_mut().enumerate() {
            *d = self.bytes.get(&addr.wrapping_add(i as u32)).copied().unwrap_or(0);
        }
        Ok(())
    }
    fn section(&self, info: u32) -> Result<Option<u32>, GuestFault> {
        Ok(self.sections.get(&info).copied())
    }
    fn pixels(&self, image: u32) -> Result<Option<u32>, GuestFault> {
        Ok(self.pixels.get(&image).copied())
    }
    fn prt_entries(&self) -> u32 {
        0
    }
}

const TABLE: u32 = 0x0060_0000;
const BSP_TAG: u32 = 0x0061_0000;
const CLUSTERS: u32 = 0x0061_1000;
const MATERIALS: u32 = 0x0061_2000;
const INSTANCES: u32 = 0x0061_3000;
const DEFINITIONS: u32 = 0x0061_4000;
const LM: u32 = 0x0062_0000;
const GROUP: u32 = 0x0062_1000;
const RENDER_INFO: u32 = 0x0062_2000;
const STREAMS: u32 = 0x0062_3000;
const BUFFERS: u32 = 0x0062_4000;
const FIRST: u32 = 0x0062_5000;
const VECTORS: u32 = 0x0063_0000;
const HIT: u32 = 0x0064_0000;
const OUT: u32 = 0x0064_1000;
const LM_BITMAP: u32 = 0x0065_0000;
const IMAGE: u32 = 0x0065_1000;
const SCENARIO_TAG: u32 = 0x0066_0000;
const BSPS: u32 = 0x0066_1000;
const LM_TAG: u32 = 0x0067_0000;
const PALETTE_GROUPS: u32 = 0x0067_1000;
const PALETTE: u32 = 0x8010_0000;

// Store tokens.
const SECTION: u32 = 0xd000_0000;
const MESHES: u32 = 0xd000_1000;
const TRIS: u32 = 0xd000_2000;
const RECORDS: u32 = 0xd000_3000;
const TEXCOORDS: u32 = 0xd000_4000;
const LIGHTMAP_UV: u32 = 0xd000_5000;
const NORMALS: u32 = 0xd000_6000;
const PIXELS: u32 = 0xd000_7000;
const BUFFER_SECTION: u32 = 0xd001_0000;
const BUFFER_RECORDS: u32 = 0xd001_1000;
const BUFFER_COLOURS: u32 = 0xd001_2000;

/// The cluster's block info (`[bsp+0xa0] + 0 * 0xb0 + 0x28`).
const CLUSTER_INFO: u32 = CLUSTERS + 0x28;
const LIGHT: u32 = 0xff40_80c0;
/// 1/2047 and 1/1023: the normal of a zero word.
const N11: u32 = 0x3a00_1002;
const N10: u32 = 0x3a80_2008;

/// A cluster hit at the first corner of triangle 0 of a resident section
/// whose lightmap UV lands on texel (2, 1) of a 4x4 P8 page, index 7,
/// palette 0 entry 7 `0xff4080c0`; the mesh's shader has no map.
fn world() -> Flat {
    let mut m = Flat::default();
    m.w(TAGS, TABLE);
    m.w(BSP, BSP_TAG);
    m.w(BSP_TAG + 0xa0, CLUSTERS);
    m.w(BSP_TAG + 0xa8, MATERIALS);
    m.w(BSP_TAG + 0x144, INSTANCES);
    m.w(BSP_TAG + 0x13c, DEFINITIONS);
    m.w(MATERIALS + 0xc, u32::MAX);
    m.w(LIGHTMAP, LM);
    m.w(LM + 0x80, 1);
    m.w(LM + 0x84, GROUP);
    m.w(GROUP + 0x1c, 1);
    m.w(GROUP + 0x2c, RENDER_INFO);
    m.h(RENDER_INFO, 0);
    m.b(RENDER_INFO + 2, 0);
    for (i, p) in [DEFAULT_NORMAL, DEFAULT_ALBEDO, DEFAULT_LIGHT].into_iter().enumerate() {
        let v = VECTORS + 0x10 * i as u32;
        m.w(p, v);
        m.f(v, 1.0);
    }
    // The hit: cluster 0, no instance, no object, mesh 0, triangle 0,
    // barycentrics (0, 0).
    m.w(HIT + 4, u32::MAX);
    m.w(HIT + 8, u32::MAX);
    // The section.
    m.sections.insert(CLUSTER_INFO, SECTION);
    m.w(SECTION + 4, MESHES);
    m.w(SECTION + 0x24, TRIS);
    m.w(SECTION + 0x38, 5);
    m.w(SECTION + 0x3c, RECORDS);
    for (i, c) in [0u16, 1, 2].into_iter().enumerate() {
        m.h(TRIS + 2 * i as u32, c);
    }
    m.b(RECORDS + 0x20, 0x18);
    m.w(RECORDS + 0x2c, TEXCOORDS);
    m.b(RECORDS + 0x60, 0x1e);
    m.w(RECORDS + 0x6c, LIGHTMAP_UV);
    m.b(RECORDS + 0x80, 0x30);
    m.w(RECORDS + 0x8c, NORMALS);
    for i in 0..3 {
        m.f(TEXCOORDS + 8 * i, 0.25);
        m.f(TEXCOORDS + 8 * i + 4, 0.75);
        m.f(LIGHTMAP_UV + 8 * i, 0.6);
        m.f(LIGHTMAP_UV + 8 * i + 4, 0.3);
    }
    // The lightmap's bitmap: one swizzled 4x4 P8 image.
    m.tag(1, LM_BITMAP);
    m.w(LM_BITMAP + 0x44, 1);
    m.w(LM_BITMAP + 0x48, IMAGE);
    m.h(IMAGE + 4, 4);
    m.h(IMAGE + 6, 4);
    m.h(IMAGE + 0xc, 0x12);
    m.h(IMAGE + 0xe, 0x1209);
    m.pixels.insert(IMAGE, PIXELS);
    m.b(PIXELS + 6, 7);
    // The palette table's source: the scenario's BSP 0's lightmap.
    m.w(0x004e_0350, SCENARIO_TAG);
    m.w(SCENARIO_TAG + 0x214, BSPS);
    m.w(BSPS + 0x1c, 2);
    m.tag(2, LM_TAG);
    m.w(LM_TAG + 0x80, 1);
    m.w(LM_TAG + 0x84, PALETTE_GROUPS);
    m.w(PALETTE_GROUPS + 8, 1);
    m.w(PALETTE_GROUPS + 0xc, PALETTE);
    m.w(PALETTE + 7 * 4, LIGHT);
    m
}

fn args(defaults: u32) -> [u32; 4] {
    [HIT, OUT, 0, defaults]
}

fn words(w: &[(u32, [u8; 4])]) -> Vec<(u32, u32)> {
    w.iter().map(|(a, b)| (*a, u32::from_le_bytes(*b))).collect()
}

#[test]
fn a_block_not_resident_returns_2_and_stores_nothing() {
    let mut m = world();
    m.sections.clear();
    let s = run(&m, args(1)).unwrap();
    assert_eq!(s.eax, 2);
    assert!(s.writes.is_empty());
}

#[test]
fn an_unlit_mesh_stores_the_defaults_only_when_asked() {
    let mut m = world();
    m.h(MESHES, 4);
    let s = run(&m, args(1)).unwrap();
    assert_eq!(s.eax, 0);
    assert_eq!(
        words(&s.writes),
        [
            (OUT, 0),
            (OUT + 4, 0),
            (OUT + 0xc, 0xff40_4040),
            (OUT + 0x10, 0xff40_4040),
            (OUT + 8, 0xbf80_0000),
        ]
    );
    // Only the last argument's low byte asks.
    let s = run(&m, args(0x100)).unwrap();
    assert_eq!(s.eax, 0);
    assert!(s.writes.is_empty());
}

#[test]
fn a_surface_hit_reads_the_page_through_its_palette() {
    let m = world();
    let s = run(&m, args(0)).unwrap();
    // The lock path.
    assert_eq!(s.eax, 3);
    assert_eq!(
        words(&s.writes),
        [
            // No base map: grey, 0.5 * 255 rounded to even.
            (OUT + 0xc, 0x0080_8080),
            (OUT + 0x10, LIGHT & 0x00ff_ffff),
            (OUT, N11),
            (OUT + 4, N11),
            (OUT + 8, N10),
        ]
    );
}

#[test]
fn a_page_not_resident_is_black_and_returns_1() {
    let mut m = world();
    m.pixels.clear();
    let s = run(&m, args(0)).unwrap();
    assert_eq!(s.eax, 1);
    assert_eq!(words(&s.writes)[1], (OUT + 0x10, 0));
}

#[test]
fn an_image_with_its_own_pixels_is_read_swizzled_without_the_store() {
    let mut m = world();
    m.pixels.clear();
    m.w(IMAGE + 0x54, PIXELS);
    // Texel (2, 0): row by row it is byte 2, swizzled (x 0b10 -> 0b100)
    // byte 4.
    for i in 0..3 {
        m.f(LIGHTMAP_UV + 8 * i + 4, 0.1);
    }
    m.b(PIXELS + 6, 0);
    m.b(PIXELS + 2, 3);
    m.b(PIXELS + 4, 7);
    for i in 0..4u16 {
        let spread = (0..2).map(|b| ((i >> b) & 1) << (2 * b)).sum::<u16>();
        m.h(0x0045_0840 + 2 * u32::from(i), spread);
    }
    let s = run(&m, args(0)).unwrap();
    assert_eq!(s.eax, 0);
    assert_eq!(words(&s.writes)[1], (OUT + 0x10, LIGHT & 0x00ff_ffff));
}

#[test]
fn an_instance_lit_by_its_vertices_sets_its_stream_offset_first() {
    let mut m = world();
    m.w(HIT + 4, 0);
    m.h(INSTANCES + 0x34, 0);
    m.h(INSTANCES + 0x56, 1);
    m.sections.clear();
    m.sections.insert(DEFINITIONS + 0x28, SECTION);
    m.w(GROUP + 0x54, STREAMS);
    m.w(GROUP + 0x44, BUFFERS);
    m.h(STREAMS + 2, 0);
    m.w(STREAMS + 8, FIRST);
    m.h(FIRST, 5);
    m.b(BUFFERS, 2);
    m.sections.insert(BUFFERS + 0xc, BUFFER_SECTION);
    m.w(BUFFER_SECTION + 4, BUFFER_RECORDS);
    m.b(BUFFER_RECORDS + 0x20, 0x2e);
    m.w(BUFFER_RECORDS + 0x2c, BUFFER_COLOURS);
    m.w(BUFFER_COLOURS + 5 * 4, 0xff10_2030);
    let s = run(&m, args(0)).unwrap();
    assert_eq!(s.eax, 0);
    let w = words(&s.writes);
    assert_eq!(&w[..2], [(BUFFER_RECORDS + 0x24, 20), (BUFFER_RECORDS + 0x28, 20)]);
    assert_eq!(w[3], (OUT + 0x10, 0x0010_2030));
}

#[test]
fn a_dxt1_base_map_is_read_by_block() {
    const SHADER: u32 = 0x0068_0000;
    const TEMPLATE: u32 = 0x0068_1000;
    const MAP_INDEX: u32 = 0x0068_2000;
    const MAPS: u32 = 0x0068_3000;
    const BASE_BITMAP: u32 = 0x0068_4000;
    const BASE_IMAGE: u32 = 0x0068_5000;
    const BLOCKS: u32 = 0xd002_0000;
    let mut m = world();
    m.w(MATERIALS + 0xc, 4);
    m.tag(4, SHADER);
    m.w(SHADER + 0x24, TEMPLATE);
    m.w(TEMPLATE + 0x60, MAP_INDEX);
    m.h(MAP_INDEX, 0);
    m.w(TEMPLATE + 8, MAPS);
    m.w(MAPS, 3);
    m.tag(3, BASE_BITMAP);
    m.w(BASE_BITMAP + 0x44, 1);
    m.w(BASE_BITMAP + 0x48, BASE_IMAGE);
    m.h(BASE_IMAGE + 4, 4);
    m.h(BASE_IMAGE + 6, 4);
    m.h(BASE_IMAGE + 0xc, 0xe);
    m.h(BASE_IMAGE + 0xe, 0x203);
    m.pixels.insert(BASE_IMAGE, BLOCKS);
    // DXT1 is 4 bits a texel (0x00453550).
    m.h(0x0045_3550 + 2 * 0xe, 4);
    // Texcoord (0.25, 0.75) is texel (0, 2) (2.5 rounds to even). Ends
    // red and blue, the red the greater word: index 2 is two thirds red.
    m.h(BLOCKS, 0xf800);
    m.h(BLOCKS + 2, 0x001f);
    m.w(BLOCKS + 4, 2 << 16);
    let s = run(&m, args(0)).unwrap();
    assert_eq!(s.eax, 3);
    assert_eq!(words(&s.writes)[0], (OUT + 0xc, 0x00aa_0055));
}

#[test]
fn colours_pack_as_the_x87_rounds() {
    let h = 0.5f32.to_bits();
    assert_eq!(pack3([h, h, h]).unwrap(), 0x0080_8080);
    // Lanes past a byte keep only their low byte; negatives wrap.
    assert_eq!(pack3([2.0f32.to_bits(), 0, (-1.0f32).to_bits()]).unwrap(), 0x00fe_0001);
    let c = unpack(0xff40_80c0).unwrap();
    assert_eq!(pack3(c).unwrap(), 0x0040_80c0);
}
