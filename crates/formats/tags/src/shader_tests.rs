//! Synthetic shader tests: every byte below is invented, no game data.
use super::*;
use crate::material::*;

const BASE: u32 = 0x8000_0000;

/// A growable metadata arena with virtual addresses from `BASE`.
#[derive(Default)]
struct Arena {
    b: Vec<u8>,
}
impl Arena {
    fn put(&mut self, bytes: &[u8]) -> u32 {
        while !self.b.len().is_multiple_of(4) {
            self.b.push(0xcc);
        }
        let a = BASE + self.b.len() as u32;
        self.b.extend_from_slice(bytes);
        a
    }
    fn meta(&self) -> Metadata {
        Metadata::new(BASE, self.b.clone())
    }
}
fn w16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn w32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn blk(b: &mut [u8], o: usize, count: u32, address: u32) {
    w32(b, o, count);
    w32(b, o + 4, address);
}
fn range(start: u16, count: u8) -> u16 {
    IndexRange { start, count }.encode()
}

const SHAD_ID: TagId = TagId(0xe000_0001);
const STEM_ID: TagId = TagId(0xe001_0002);
const SPAS_ID: TagId = TagId(0xe002_0003);
const VRTX_ID: TagId = TagId(0xe003_0004);
const BITM_ID: TagId = TagId(0xe004_0005);

struct Fixture {
    arena: Arena,
    tags: Vec<(TagId, FourCC, u32)>,
}

/// One template with one LOD, layer 2 enabled, one pass, two implementations:
/// 0 is static, 1 is a two-fragment variant.
fn fixture() -> Fixture {
    let mut a = Arena::default();
    // bitm: 2 images, image 1 is 64x32 with flag 0x10.
    let mut images = vec![0u8; 2 * 0x74];
    w16(&mut images, 0x74 + 4, 64);
    w16(&mut images, 0x74 + 6, 32);
    images[0x74 + 0xe] = 0x10;
    let images_at = a.put(&images);
    let mut bitm = vec![0u8; 0x4c];
    blk(&mut bitm, 0x44, 2, images_at);
    let bitm_at = a.put(&bitm);

    // spas children.
    let mut stages = Vec::new();
    for k in 0..3u32 {
        for f in 0..8u32 {
            stages.extend((0x100 * (k + 1) + f).to_le_bytes());
        }
    }
    let stages_at = a.put(&stages);
    // variants: 0 = stage 0 only; 1 = stage 1 then final (stage 2).
    let mut variants = vec![0u8; 12];
    w16(&mut variants, 4, range(0, 1));
    variants[6 + 2] = 1;
    w16(&mut variants, 6 + 4, range(1, 2));
    let variants_at = a.put(&variants);
    // fragments: [fixed -> variant 0], [selector int 4 -> base 0 + int4].
    let mut frags = vec![0u8; 8];
    w16(&mut frags, 0, 0xffff);
    w16(&mut frags, 2, 0);
    w16(&mut frags, 4, 4);
    w16(&mut frags, 6, 0);
    let frags_at = a.put(&frags);
    // textures: param slot 5 -> stage 0 (record size), engine lightmap -> stage 1.
    let textures_at = a.put(&[5, 0, 0, 2, 0xff, 11, 1, 0]);
    // render states: ALPHABLENDENABLE=1, CULLMODE=0 (apply), CULLMODE=0x900 (restore).
    let mut rs = Vec::new();
    for (s, v) in [(59u8, 1u32), (0x93, 0), (0x93, 0x900)] {
        rs.push(s);
        rs.extend(v.to_le_bytes());
    }
    let rs_at = a.put(&rs);
    let mut tss = vec![0u8; 0x18];
    tss[0] = 0x20;
    tss[1] = 0x12;
    tss[2] = 0x31;
    tss[3] = 0x45;
    tss[4..7].copy_from_slice(&[1, 3, 4]);
    w32(&mut tss, 8, 0xdead_beef);
    w32(&mut tss, 0x14, 0.5f32.to_bits());
    let tss_at = a.put(&tss);
    // externals: vertex: c bank1 reg 2, mode 4 (vector xyz), source 7.
    //            pixel: final-combiner c0 (0x200), mode 3 (alpha), transform 1 (1-x), source 21.
    let vext = (2u16) | (1 << 4) | (4 << 5);
    let mut ext = vext.to_le_bytes().to_vec();
    ext.extend([0, 7]);
    ext.extend(0x200u16.to_le_bytes());
    ext.extend([3 | (1 << 3), 21]);
    let ext_at = a.put(&ext);

    let mut imps = vec![0u8; 2 * IMPLEMENTATION];
    for k in 0..2 {
        let i = &mut imps[k * IMPLEMENTATION..(k + 1) * IMPLEMENTATION];
        w16(i, 0, range(0, 2));
        w16(i, 2, range(0, 2));
        w16(i, 4, range(0, 1));
        w16(i, 0x11c, range(2, 1));
        w32(i, 0xfc, u32::from_be_bytes(*b"vrtx"));
        w32(i, 0x100, VRTX_ID.0);
        // PSDEF: texture modes word 54 marker, combiner count word 53.
        w32(i, 6 + 54 * 4, 0x0000_0001);
        w32(
            i,
            6 + 53 * 4,
            if k == 0 { 0x0001_1101 } else { 0x0001_1100 },
        );
    }
    {
        let i = &mut imps[0..IMPLEMENTATION];
        w16(i, 0x124, range(0, 1));
        w16(i, 0x122, range(1, 1));
    }
    {
        let i = &mut imps[IMPLEMENTATION..];
        w16(i, 0xf6, range(0, 2));
    }
    let imps_at = a.put(&imps);
    let mut spas_pp = vec![0u8; SPAS_POSTPROCESS];
    blk(&mut spas_pp, 0x00, 2, imps_at);
    blk(&mut spas_pp, 0x08, 2, textures_at);
    blk(&mut spas_pp, 0x10, 3, rs_at);
    blk(&mut spas_pp, 0x18, 1, tss_at);
    blk(&mut spas_pp, 0x20, 2, frags_at);
    blk(&mut spas_pp, 0x28, 2, variants_at);
    blk(&mut spas_pp, 0x30, 3, stages_at);
    blk(&mut spas_pp, 0x38, 2, ext_at);
    let spas_pp_at = a.put(&spas_pp);
    let mut spas = vec![0u8; SPAS_ROOT];
    blk(&mut spas, 0x1c, 1, spas_pp_at);
    let spas_at = a.put(&spas);

    // stem: lod 0 -> layers [none, none, pass list], one pass, two binding rows.
    let mut lods = vec![0u8; 10];
    w16(&mut lods, 0, range(0, 3));
    w32(&mut lods, 2, 1 << 2);
    let lods_at = a.put(&lods);
    let mut layers = vec![0u8; 6];
    w16(&mut layers, 4, range(0, 1));
    let layers_at = a.put(&layers);
    let mut passes = vec![0u8; 10];
    w32(&mut passes, 0, u32::from_be_bytes(*b"spas"));
    w32(&mut passes, 4, SPAS_ID.0);
    w16(&mut passes, 8, range(0, 2));
    let passes_at = a.put(&passes);
    // Bindings: 0 bitmap slot 5 <- bitmap 0; 1 colour c0[1] green <- colour 0;
    // 2 vector v3 (bank 0) xy (mode 9) <- vector 0; 3 vector c bank1 reg 0 xyzw <- vector 1;
    // 4 colour (variant): fragment 1, needs int4 == 1, register 0 -> c1, all of RGB.
    let mut bind = Vec::new();
    bind.extend([5, 0, 0, 0]);
    bind.extend((1u16 << 6).to_le_bytes());
    bind.extend([1, 0]);
    bind.extend((3u16 | (9 << 5)).to_le_bytes());
    bind.extend([0, 0]);
    bind.extend(((1u16 << 4) | (13 << 5)).to_le_bytes());
    bind.extend([0, 1]);
    bind.extend((1u16 | (1 << 3) | 0x400).to_le_bytes());
    bind.extend([4, 0]);
    let bind_at = a.put(&bind);
    let mut rows = vec![0u8; 12];
    w16(&mut rows, 0, range(0, 1));
    w16(&mut rows, 2, range(1, 1));
    w16(&mut rows, 4, range(2, 2));
    w16(&mut rows, 6, range(0, 1));
    w16(&mut rows, 8, range(4, 1));
    let rows_at = a.put(&rows);
    let mut stem_pp = vec![0u8; STEM_POSTPROCESS];
    blk(&mut stem_pp, 0x00, 1, lods_at);
    blk(&mut stem_pp, 0x08, 3, layers_at);
    blk(&mut stem_pp, 0x10, 1, passes_at);
    blk(&mut stem_pp, 0x18, 2, rows_at);
    blk(&mut stem_pp, 0x20, 5, bind_at);
    let stem_pp_at = a.put(&stem_pp);
    let mut stem = vec![0u8; STEM_ROOT];
    for o in [0x40, 0x4c] {
        w32(&mut stem, o, u32::from_be_bytes(*b"shad"));
        w32(&mut stem, o + 4, u32::MAX);
    }
    blk(&mut stem, 0x58, 1, stem_pp_at);
    let stem_at = a.put(&stem);

    // shad: one bitmap (image 3 wraps to 1 of 2), one colour, two vectors.
    let mut bm = vec![0u8; 0xc];
    w32(&mut bm, 0, BITM_ID.0);
    w32(&mut bm, 4, 3);
    w32(&mut bm, 8, 0.25f32.to_bits());
    let bm_at = a.put(&bm);
    let col_at = a.put(&0x11_22_33_44u32.to_le_bytes());
    let mut vecs = Vec::new();
    for v in [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0] {
        vecs.extend(v.to_bits().to_le_bytes());
    }
    let vec_at = a.put(&vecs);
    let mut sp = vec![0u8; SHAD_POSTPROCESS];
    w32(&mut sp, 0, STEM_ID.0);
    blk(&mut sp, 0x04, 1, bm_at);
    blk(&mut sp, 0x0c, 1, col_at);
    blk(&mut sp, 0x14, 2, vec_at);
    let sp_at = a.put(&sp);
    let mut shad = vec![0u8; SHAD_ROOT];
    w32(&mut shad, 0, u32::from_be_bytes(*b"stem"));
    w32(&mut shad, 4, STEM_ID.0);
    blk(&mut shad, 0x20, 1, sp_at);
    w16(&mut shad, 0x3e, 1);
    w32(&mut shad, 0x44, 0.75f32.to_bits());
    let shad_at = a.put(&shad);
    Fixture {
        arena: a,
        tags: vec![
            (SHAD_ID, SHAD, shad_at),
            (STEM_ID, STEM, stem_at),
            (SPAS_ID, SPAS, spas_at),
            (VRTX_ID, VRTX, BASE),
            (BITM_ID, BITM, bitm_at),
        ],
    }
}

fn library(f: &Fixture) -> ShaderLibrary {
    ShaderLibrary::new(f.arena.meta(), f.tags.clone())
}

fn u32_in(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
/// Arena offset of the block whose descriptor is at `offset` in the first
/// postprocess element of fixture tag `tag` (0 shad, 1 stem, 2 spas).
fn pp_offset(f: &Fixture, tag: usize, root: usize) -> usize {
    let r = (f.tags[tag].2 - BASE) as usize;
    (u32_in(&f.arena.b, r + root + 4) - BASE) as usize
}
fn shad_pp(f: &Fixture) -> usize {
    pp_offset(f, 0, 0x20)
}
fn stem_pp(f: &Fixture) -> usize {
    pp_offset(f, 1, 0x58)
}
fn spas_pp(f: &Fixture) -> usize {
    pp_offset(f, 2, 0x1c)
}
/// Arena offset of the block a descriptor at `at` points to.
fn target(f: &Fixture, at: usize) -> usize {
    (u32_in(&f.arena.b, at + 4) - BASE) as usize
}
/// The innermost error under any field context.
fn root_cause(mut e: &Error) -> &Error {
    while let Error::Field { source, .. } = e {
        e = source;
    }
    e
}
fn invalid(r: Result<impl std::fmt::Debug>) -> &'static str {
    match r {
        Err(e) => match root_cause(&e) {
            Error::Invalid(m) => m,
            other => panic!("expected an invalid-tag error, got {other:?}"),
        },
        Ok(v) => panic!("expected an error, got {v:?}"),
    }
}
fn c20(m: &Material, imp: usize) -> Register {
    *m.layers[0].passes[0].implementations[imp]
        .vertex_constants
        .iter()
        .find(|r| r.index == 20)
        .unwrap()
}

#[test]
fn index_range_packs_nine_and_seven_bits() {
    let r = IndexRange::decode(0x3200);
    assert_eq!((r.start, r.count), (0, 25));
    assert_eq!(
        IndexRange::decode(0x0207),
        IndexRange { start: 7, count: 1 }
    );
    assert_eq!(
        IndexRange {
            start: 511,
            count: 127
        }
        .encode(),
        0xffff
    );
    // Start bit 8 is part of the start, not the count.
    assert_eq!(
        IndexRange::decode(0x0100),
        IndexRange {
            start: 256,
            count: 0
        }
    );
    assert_eq!(
        IndexRange::decode(0x03ff),
        IndexRange {
            start: 511,
            count: 1
        }
    );
    assert_eq!(IndexRange::decode(0x0300).indices(), 256..257);
    assert!(IndexRange::decode(0x0403).check(4, "x").is_err());
    assert!(IndexRange::decode(0x0402).check(4, "x").is_ok());
}

#[test]
fn decodes_records() {
    let f = fixture();
    let meta = f.arena.meta();
    let s = ShaderInstance::decode(&meta, f.tags[0].2).unwrap();
    assert_eq!(s.template.id, STEM_ID);
    assert_eq!(s.int_inputs, [1, 0]);
    assert_eq!(s.scalar_input, 0.75);
    let p = s.compiled().unwrap();
    assert_eq!(p.bitmaps[0].image, 3);
    assert_eq!(p.vectors[1], [5.0, 6.0, 7.0, 8.0]);
    let pass = ShaderPass::decode(&meta, f.tags[2].2).unwrap();
    let pp = pass.compiled().unwrap();
    pp.validate().unwrap();
    let st = pp.stage_states[0];
    assert_eq!((st.min_filter, st.mag_filter, st.mip_filter), (2, 2, 1));
    assert_eq!(
        (
            st.alpha_kill,
            st.color_key_op,
            st.max_anisotropy,
            st.max_mip_level
        ),
        (1, 3, 5, 4)
    );
    assert_eq!(
        (st.address_u, st.address_v, st.address_w, st.border_color),
        (1, 3, 4, 0xdead_beef)
    );
    assert_eq!(
        pp.implementations[0].restore_states,
        IndexRange { start: 2, count: 1 }
    );
    assert_eq!(pp.implementations[0].vertex_shader.id, VRTX_ID);
}

#[test]
fn resolves_static_implementation() {
    let f = fixture();
    let mut lib = library(&f);
    let m = lib.material(SHAD_ID, None).unwrap();
    assert_eq!(m.layers.len(), 1);
    assert_eq!((m.layers[0].lod, m.layers[0].layer), (0, 2));
    let imps = &m.layers[0].passes[0].implementations;
    assert_eq!(imps.len(), 2);
    let i = &imps[0];
    assert_eq!(i.vertex_shader, Some(VRTX_ID));
    // Stage 0: bitmap image 3 wrapped by 2 images -> 1 for binding. The size
    // refresh compares the raw frame 3 with the count 2 first (0x00018b31)
    // and is skipped, and the stage carries the fresh (zero) slot size.
    let t0 = i.textures[0].unwrap();
    assert_eq!(
        t0.source,
        TextureSource::Bitmap {
            bitmap: BITM_ID,
            image: 1,
            lod_offset: 0.25
        }
    );
    assert_eq!(t0.size, None);
    assert_eq!(t0.stage_size, Some((0, 0)));
    assert_eq!(i.textures[1].unwrap().source, TextureSource::Lightmap);
    assert_eq!(i.textures[1].unwrap().stage_size, None);
    assert_eq!(i.render_states.len(), 2);
    assert_eq!(i.restore_states[0].value, 0x900);
    // Colour 0x11223344 green lane into c0[1].
    let c0_1 = u32::from_le_bytes(
        i.pixel_shader[(10 + 1) * 4..(10 + 1) * 4 + 4]
            .try_into()
            .unwrap(),
    );
    assert_eq!(c0_1, 0x0000_3300);
    // Engine pixel constant: final c0 alpha = 1 - scalar21 = 0.25 -> 64.
    let fc0 = u32::from_le_bytes(i.pixel_shader[43 * 4..44 * 4].try_into().unwrap());
    assert_eq!(fc0, 64 << 24);
    assert_eq!(i.c0_written, (1 << 1) | (1 << 8));
    assert!(i.pixel_constants_from_engine);
    assert!(i.per_draw_inputs, "c[20] reads engine vector 7");
    // v3.xy from vector 0, c[18].xyzw from vector 1, c[20] from engine vector 7.
    let v3 = i.vertex_defaults.iter().find(|r| r.index == 3).unwrap();
    assert_eq!(v3.value, [1.0, 2.0, 0.0, 0.0]);
    let c18 = i.vertex_constants.iter().find(|r| r.index == 18).unwrap();
    assert_eq!(c18.value, [5.0, 6.0, 7.0, 8.0]);
    let c20 = i.vertex_constants.iter().find(|r| r.index == 20).unwrap();
    assert_eq!(c20.origin, Origin::Engine);
}

#[test]
fn selected_rebinds_preserve_unwritten_lanes_stages_and_variant_words() {
    let mut f = fixture();
    // Compare mode in the embedded nonvariant definition. A later variant
    // updates its texture modes and combiners while retaining this word.
    let first = target(&f, spas_pp(&f));
    w32(&mut f.arena.b, first + 6 + 42 * 4, 0x1234);
    let mut lib = library(&f);
    let inputs = EngineInputs::for_shader(&lib.shader(SHAD_ID).unwrap());
    let mut state = ParameterState::default();
    state.banks[0][3] = [9., 8., 7., -0.];
    state.banks[1][2][3] = 37.;
    state.slot_sizes.insert(5, Some((21, 34)));
    let inherited = StageTexture { source: TextureSource::Engine(2), size: None, stage_size: Some((8, 9)) };
    state.stages[3] = Some(inherited);
    let selection = PassSelection { lod: 0, layer: 2, pass: 0, implementation: 0 };
    let bind = lib.selected_implementation(SHAD_ID, selection, Some(&inputs), Some(&mut state)).unwrap().implementation;
    assert_eq!(state.banks[0][3].map(f32::to_bits), [1.0f32, 2., 7., -0.].map(f32::to_bits));
    assert_eq!(state.banks[1][2][3], 37.);
    assert_eq!(bind.textures[0].unwrap().stage_size, Some((21, 34)));
    assert_eq!(state.stages[3], Some(inherited));

    let variant = PassSelection { implementation: 1, ..selection };
    let selected = lib.selected_implementation(SHAD_ID, variant, Some(&inputs), Some(&mut state)).unwrap();
    let writes: Vec<_> = selected.render_writes.iter().map(|s| (s.state, s.value)).collect();
    assert_eq!(writes, [(0x93, 0x900), (59, 1), (0x93, 0)]);
    assert_eq!(state.fixed.render.get(&0x93), Some(&0));
    assert_eq!(state.fixed.samplers[0], selected.implementation.stage_states.first().copied());
    assert!(lib.selected_implementation(SHAD_ID, variant, Some(&inputs), Some(&mut state)).unwrap().render_writes.is_empty());
    let bind = selected.implementation;
    assert!(bind.variant);
    assert_eq!(bind.pixel_shader[42 * 4..43 * 4], 0x1234u32.to_le_bytes());
    assert_eq!(state.stages[3], Some(inherited));
    assert_eq!(state.banks[1][2][3], 37.);
    let before = state.clone();
    let missing = PassSelection { implementation: 3, ..selection };
    assert!(lib.selected_implementation(SHAD_ID, missing, Some(&inputs), Some(&mut state)).is_err());
    assert_eq!(state, before);
}

#[test]
fn builds_variant_combiners_from_engine_inputs() {
    let f = fixture();
    let mut lib = library(&f);
    // int4 = 1 (from the shad root): fragment 1 selects variant 1 = stage 1 + final.
    let m = lib.material(SHAD_ID, None).unwrap();
    let i = &m.layers[0].passes[0].implementations[1];
    assert!(i.variant);
    let word = |n: usize| u32::from_le_bytes(i.pixel_shader[n * 4..n * 4 + 4].try_into().unwrap());
    assert_eq!(word(53), 0x0001_1102, "two general stages");
    assert_eq!(word(34), 0x100, "fragment 0 rgb inputs");
    assert_eq!(word(34 + 1), 0x200, "fragment 1 rgb inputs");
    assert_eq!(word(8), 0x300, "final ABCD");
    assert_eq!(word(9), 0x302, "final EFG");
    assert_eq!(word(54), 1, "texture modes kept");
    // Conditional colour (fragment 1, needs int4 == 1) lands in c1[0 + offset 1].
    assert_eq!(word(18 + 1) & 0x00ff_ffff, 0x0022_3344);
    // With int4 = 0, fragment 1 picks variant 0 (no final), and the colour is skipped.
    let mut e = EngineInputs::default();
    e.ints[4] = 0;
    let m = lib.material(SHAD_ID, Some(e)).unwrap();
    let i = &m.layers[0].passes[0].implementations[1];
    let word = |n: usize| u32::from_le_bytes(i.pixel_shader[n * 4..n * 4 + 4].try_into().unwrap());
    assert_eq!((word(8), word(9)), (0xc, 0x2000));
    assert_eq!(i.c1_written, 0);
}

#[test]
fn a_resolved_pixel_write_updates_the_final_register_and_its_dirty_bit() {
    let mut m = library(&fixture()).material(SHAD_ID, None).unwrap();
    let imp = &mut m.layers[0].passes[0].implementations[0];
    let final_c1 = 44 * 4;
    imp.pixel_shader[final_c1..final_c1 + 4].copy_from_slice(&0x3564_a1b2u32.to_le_bytes());
    let c0 = imp.c0_written;
    let c1 = imp.c1_written;
    imp.write_pixel_constant((true, 8), 0x0000_ff00, 0x00ff_7eff).unwrap();
    assert_eq!(&imp.pixel_shader[final_c1..final_c1 + 4], &0x3564_7eb2u32.to_le_bytes());
    assert_eq!((imp.c0_written, imp.c1_written), (c0, c1 | (1 << 8)));
    assert!(imp.write_pixel_constant((true, 9), u32::MAX, 0).is_err());
}

#[test]
fn aliased_blocks_resolve_across_the_region() {
    // Two shad postprocess blocks share one array that lives outside the tag.
    let mut a = Arena::default();
    let shared = a.put(&[0x01, 0x02, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00]);
    let mut sp = vec![0u8; SHAD_POSTPROCESS];
    blk(&mut sp, 0x0c, 2, shared);
    blk(&mut sp, 0x24, 4, shared);
    let sp_at = a.put(&sp);
    let mut shad = vec![0u8; SHAD_ROOT];
    blk(&mut shad, 0x20, 1, sp_at);
    let at = a.put(&shad);
    let s = ShaderInstance::decode(&a.meta(), at).unwrap();
    let p = s.compiled().unwrap();
    assert_eq!(p.colors, vec![0x0201, 0x0201]);
    assert_eq!(p.layers.len(), 4);
}

#[test]
fn budgets_and_bounds_are_enforced() {
    let mut a = Arena::default();
    let mut sp = vec![0u8; SHAD_POSTPROCESS];
    blk(&mut sp, 0x0c, MAX_BLOCK_ELEMENTS + 1, BASE);
    let sp_at = a.put(&sp);
    let mut shad = vec![0u8; SHAD_ROOT];
    blk(&mut shad, 0x20, 1, sp_at);
    let at = a.put(&shad);
    assert!(ShaderInstance::decode(&a.meta(), at).is_err());

    // A block running past the metadata region is a bounds error.
    let mut a = Arena::default();
    let mut sp = vec![0u8; SHAD_POSTPROCESS];
    blk(&mut sp, 0x14, 64, BASE);
    let sp_at = a.put(&sp);
    let mut shad = vec![0u8; SHAD_ROOT];
    blk(&mut shad, 0x20, 1, sp_at);
    let at = a.put(&shad);
    assert!(ShaderInstance::decode(&a.meta(), at).is_err());

    // Root outside the region.
    assert!(ShaderInstance::decode(&a.meta(), BASE - 4).is_err());
}

#[test]
fn rejects_bad_ranges_and_groups() {
    let f = fixture();
    let meta = f.arena.meta();
    let mut pass = ShaderPass::decode(&meta, f.tags[2].2).unwrap();
    let pp = &mut pass.postprocess[0];
    pp.implementations[0].textures = IndexRange { start: 1, count: 2 };
    assert!(pp.validate().is_err());
    // Wrong group for a reference.
    let mut tags = f.tags.clone();
    tags[1].1 = SPAS;
    let mut lib = ShaderLibrary::new(meta.clone(), tags);
    assert!(lib.material(SHAD_ID, None).is_err());
    // Populated never-used guard block in spas.
    let mut a = Arena::default();
    let mut spp = vec![0u8; SPAS_POSTPROCESS];
    blk(&mut spp, 0x40, 1, BASE);
    let at = a.put(&spp);
    let mut root = vec![0u8; SPAS_ROOT];
    blk(&mut root, 0x1c, 1, at);
    let r = a.put(&root);
    assert!(matches!(
        ShaderPass::decode(&a.meta(), r),
        Err(Error::Field { .. })
    ));
}

#[test]
fn render_state_names_follow_engine_tables() {
    assert_eq!(render_state_name(67), Some("COLORWRITEENABLE"));
    assert_eq!(render_state_name(74), Some("BLENDOP"));
    assert_eq!(render_state_name(92), Some("FOGENABLE"));
    assert_eq!(render_state_name(147), Some("CULLMODE"));
    assert_eq!(render_state_name(158), Some("SAMPLEALPHA"));
    assert_eq!(render_state_name(166), None);
    assert_eq!(stage_state_name(29), Some("BORDERCOLOR"));
}

#[test]
fn input_mask_offsets_follow_engine_consumers() {
    // 0x00016e04: +0x10c/+0x110 -> state +0x44/+0x48, walked for the scalar
    // evaluator 0x000336f0; +0x104/+0x108 -> state +0x4c/+0x50, walked for
    // the vector evaluator 0x00033980.
    let mut bytes = [0u8; IMPLEMENTATION];
    w32(&mut bytes, 0x104, 1 << 7);
    w32(&mut bytes, 0x108, 1 << 3);
    w32(&mut bytes, 0x10c, 1 << 21);
    w32(&mut bytes, 0x110, 1 << 2);
    w32(&mut bytes, 0x114, 1 << 5);
    let i = Implementation::decode(&bytes).unwrap();
    assert_eq!(i.scalar_inputs, (1u64 << 21) | (1u64 << 34));
    assert_eq!(i.vector_inputs, (1u64 << 7) | (1u64 << 35));
    assert_eq!(i.int_inputs, 1 << 5);
}

#[test]
fn block_cap_rejects_an_in_bounds_oversized_block() {
    // The metadata really backs every element, so only the cap can refuse.
    for (count, ok) in [(MAX_BLOCK_ELEMENTS, true), (MAX_BLOCK_ELEMENTS + 1, false)] {
        let mut a = Arena::default();
        let colours = a.put(&vec![0; (MAX_BLOCK_ELEMENTS as usize + 1) * 4]);
        let mut sp = vec![0u8; SHAD_POSTPROCESS];
        blk(&mut sp, 0x0c, count, colours);
        let sp_at = a.put(&sp);
        let mut shad = vec![0u8; SHAD_ROOT];
        blk(&mut shad, 0x20, 1, sp_at);
        let at = a.put(&shad);
        let r = ShaderInstance::decode(&a.meta(), at);
        if ok {
            assert_eq!(r.unwrap().postprocess[0].colors.len(), count as usize);
        } else {
            assert_eq!(invalid(r), "block count exceeds the element budget");
        }
    }
}

#[test]
fn element_budget_is_shared_by_every_postprocess_record() {
    // n records all alias one 1,024-colour array: n * 1,025 elements from a
    // few KB. 63 fit in 65,536; 64 do not, though each record alone would.
    for (n, ok) in [(63u32, true), (64, false)] {
        let mut a = Arena::default();
        let shared = a.put(&vec![0; 1024 * 4]);
        let mut pp = vec![0; SHAD_POSTPROCESS];
        blk(&mut pp, 0x0c, 1024, shared);
        let pp_at = a.put(&pp.repeat(n as usize));
        let mut root = vec![0; SHAD_ROOT];
        blk(&mut root, 0x20, n, pp_at);
        let at = a.put(&root);
        let r = ShaderInstance::decode(&a.meta(), at);
        if ok {
            assert_eq!(r.unwrap().postprocess.len(), n as usize);
        } else {
            assert_eq!(invalid(r), "tag exceeds its total element budget");
        }
    }
}

#[test]
fn byte_budget_counts_aliased_function_blobs() {
    // n properties share one 4 KiB function blob; every copy is charged.
    let per = 0x14 + MAX_FUNCTION_BYTES as usize;
    let fits = (MAX_TAG_BYTES - SHAD_POSTPROCESS) / per;
    for (n, ok) in [(fits, true), (fits + 1, false)] {
        let mut a = Arena::default();
        let blob = a.put(&vec![0x5a; MAX_FUNCTION_BYTES as usize]);
        let mut prop = vec![0u8; 0x14];
        blk(&mut prop, 0x0c, MAX_FUNCTION_BYTES, blob);
        let props = a.put(&prop.repeat(n));
        let mut sp = vec![0u8; SHAD_POSTPROCESS];
        blk(&mut sp, 0x3c, n as u32, props);
        let sp_at = a.put(&sp);
        let mut shad = vec![0u8; SHAD_ROOT];
        blk(&mut shad, 0x20, 1, sp_at);
        let at = a.put(&shad);
        let r = ShaderInstance::decode(&a.meta(), at);
        if ok {
            assert_eq!(r.unwrap().postprocess[0].properties.len(), n);
        } else {
            assert_eq!(invalid(r), "tag exceeds its total byte budget");
        }
    }
}

#[test]
fn material_expansion_is_budgeted_before_resolving() {
    // Five layers each select the same 127 passes, each selecting the same
    // 127 implementations: every range is valid, the output is not bounded
    // by any decoder budget.
    let mut f = fixture();
    let spp = spas_pp(&f);
    let orig = target(&f, spp);
    let imp = f.arena.b[orig..orig + IMPLEMENTATION].to_vec();
    let imps = f.arena.put(&imp.repeat(127));
    blk(&mut f.arena.b, spp, 127, imps);
    let tpp = stem_pp(&f);
    let old_rows = target(&f, tpp + 0x18);
    let row = f.arena.b[old_rows..old_rows + 6].to_vec();
    let rows = f.arena.put(&row.repeat(127));
    blk(&mut f.arena.b, tpp + 0x18, 127, rows);
    let mut pass = vec![0; 10];
    w32(&mut pass, 0, SPAS.as_u32());
    w32(&mut pass, 4, SPAS_ID.0);
    w16(&mut pass, 8, range(0, 127));
    let passes = f.arena.put(&pass.repeat(127));
    blk(&mut f.arena.b, tpp + 0x10, 127, passes);
    let layers = f.arena.put(&range(0, 127).to_le_bytes().repeat(5));
    blk(&mut f.arena.b, tpp + 0x08, 5, layers);
    let lod = target(&f, tpp);
    w16(&mut f.arena.b, lod, range(0, 5));
    w32(&mut f.arena.b, lod + 2, 0x1f);
    assert_eq!(
        invalid(library(&f).material(SHAD_ID, None)),
        "material expands past its pass/implementation budget"
    );
    // One layer of 32 passes x 127 implementations fits the count budget.
    w16(&mut f.arena.b, lod, range(0, 1));
    w32(&mut f.arena.b, lod + 2, 1);
    let one = f.arena.put(&range(0, 32).to_le_bytes());
    blk(&mut f.arena.b, tpp + 0x08, 1, one);
    let m = library(&f).material(SHAD_ID, None).unwrap();
    let n: usize = m.layers[0]
        .passes
        .iter()
        .map(|p| p.implementations.len())
        .sum();
    assert_eq!(n, 32 * 127);
}

#[test]
fn material_work_budget_bounds_animation_copies() {
    // One implementation whose animation leaf names 127 bindings, each with
    // 127 property refs, repeated over 32 aliased passes x 127 implementations.
    let mut f = fixture();
    let spp = spas_pp(&f);
    let orig = target(&f, spp);
    let imp = f.arena.b[orig..orig + IMPLEMENTATION].to_vec();
    let imps = f.arena.put(&imp.repeat(127));
    blk(&mut f.arena.b, spp, 127, imps);
    let tpp = stem_pp(&f);
    let old_rows = target(&f, tpp + 0x18);
    let row = f.arena.b[old_rows..old_rows + 6].to_vec();
    let rows = f.arena.put(&row.repeat(127));
    blk(&mut f.arena.b, tpp + 0x18, 127, rows);
    let mut pass = vec![0; 10];
    w32(&mut pass, 0, SPAS.as_u32());
    w32(&mut pass, 4, SPAS_ID.0);
    w16(&mut pass, 8, range(0, 127));
    let passes = f.arena.put(&pass.repeat(32));
    blk(&mut f.arena.b, tpp + 0x10, 32, passes);
    let layers_at = target(&f, tpp + 0x08);
    w16(&mut f.arena.b, layers_at + 4, range(0, 32));
    // Instance animation tree: lod 0, layer 2, every pass and implementation
    // share one leaf.
    let sp = shad_pp(&f);
    let mut lod = vec![0u8; 6];
    w32(&mut lod, 0, 1 << 2);
    w16(&mut lod, 4, range(0, 3));
    let lods = f.arena.put(&lod);
    blk(&mut f.arena.b, sp + 0x1c, 1, lods);
    let layers = f.arena.put(&range(0, 32).to_le_bytes().repeat(3));
    blk(&mut f.arena.b, sp + 0x24, 3, layers);
    let passes = f.arena.put(&range(0, 127).to_le_bytes().repeat(32));
    blk(&mut f.arena.b, sp + 0x2c, 32, passes);
    let mut leaf = vec![0u8; 10];
    w16(&mut leaf, 8, range(0, 127));
    let leaves = f.arena.put(&leaf.repeat(127));
    blk(&mut f.arena.b, sp + 0x34, 127, leaves);
    let props = f.arena.put(&[0u8; 0x14]);
    blk(&mut f.arena.b, sp + 0x3c, 1, props);
    let refs = f.arena.put(&[0u8; 4 * 127]);
    blk(&mut f.arena.b, sp + 0x44, 127, refs);
    let pr = f.arena.put(&range(0, 127).to_le_bytes());
    blk(&mut f.arena.b, sp + 0x4c, 1, pr);
    let anims = f.arena.put(&[0u8; 4 * 127]);
    blk(&mut f.arena.b, sp + 0x54, 127, anims);
    assert_eq!(
        invalid(library(&f).material(SHAD_ID, None)),
        "material exceeds its resolution work budget"
    );
}

#[test]
fn texture_stage_above_three_is_rejected() {
    let mut f = fixture();
    let meta = f.arena.meta();
    let mut pass = ShaderPass::decode(&meta, f.tags[2].2).unwrap();
    pass.postprocess[0].textures[1].stage = 4;
    assert_eq!(
        invalid(pass.postprocess[0].validate()),
        "texture binding stage above 3"
    );
    pass.postprocess[0].textures[1].stage = 3;
    pass.postprocess[0].validate().unwrap();
    // The same through the material path: an error, never a panic.
    let tex = target(&f, spas_pp(&f) + 0x08);
    f.arena.b[tex + 4 + 2] = 4;
    assert_eq!(
        invalid(library(&f).material(SHAD_ID, None)),
        "texture binding stage above 3"
    );
}

#[test]
fn animation_targets_follow_the_instance_tree() {
    let mut f = fixture();
    let sp = shad_pp(&f);
    // lod 0: layers 0-2, layer 2 animated -> pass range [0] -> implementation
    // range [0, 1]; implementation 1's leaf animates one vertex constant
    // from property range 0 = refs 0-1 of property 0.
    let mut lod = vec![0u8; 6];
    w32(&mut lod, 0, 1 << 2);
    w16(&mut lod, 4, range(0, 3));
    let lods = f.arena.put(&lod);
    blk(&mut f.arena.b, sp + 0x1c, 1, lods);
    let mut layers = vec![0u8; 6];
    w16(&mut layers, 4, range(0, 1));
    let layers = f.arena.put(&layers);
    blk(&mut f.arena.b, sp + 0x24, 3, layers);
    let passes = f.arena.put(&range(0, 2).to_le_bytes());
    blk(&mut f.arena.b, sp + 0x2c, 1, passes);
    let mut leaves = vec![0u8; 20];
    w16(&mut leaves, 10 + 8, range(0, 1));
    let leaves = f.arena.put(&leaves);
    blk(&mut f.arena.b, sp + 0x34, 2, leaves);
    let blob = f.arena.put(&[1, 2, 3, 4, 5, 6, 7, 8]);
    let mut prop = vec![0u8; 0x14];
    blk(&mut prop, 0x0c, 8, blob);
    let props = f.arena.put(&prop);
    blk(&mut f.arena.b, sp + 0x3c, 1, props);
    let refs = f.arena.put(&[0, 0, 1, 0, 0, 0, 2, 0]);
    blk(&mut f.arena.b, sp + 0x44, 2, refs);
    let pr = f.arena.put(&range(0, 2).to_le_bytes());
    blk(&mut f.arena.b, sp + 0x4c, 1, pr);
    let anims = f.arena.put(&[3, 0, 0, 0]);
    blk(&mut f.arena.b, sp + 0x54, 1, anims);

    let m = library(&f).material(SHAD_ID, None).unwrap();
    let imps = &m.layers[0].passes[0].implementations;
    assert!(imps[0].animated.is_empty());
    assert_eq!(
        imps[1].animated,
        vec![AnimatedTarget {
            kind: AnimationKind::VertexConstant,
            pixel_register: None,
            binding: AnimationBinding {
                target: 3,
                b1: 0,
                b2: 0,
                properties: 0
            },
            properties: vec![
                PropertyRef {
                    property: 0,
                    component: 1
                },
            ],
            stages: Vec::new(),
        }]
    );
    // Direct constant writers mask off the count and dereference the start.
    let range_at = target(&f, sp + 0x4c);
    w16(&mut f.arena.b, range_at, range(1, 0));
    let m = library(&f).material(SHAD_ID, None).unwrap();
    assert_eq!(m.layers[0].passes[0].implementations[1].animated[0].properties,
        [PropertyRef { property: 0, component: 2 }]);
    // Feed the same references into two pixel constants of fragment 1.
    // The first matches its selected variant. The second requires variant 0.
    let leaf = target(&f, sp + 0x34) + 10;
    w16(&mut f.arena.b, leaf + 8, 0);
    w16(&mut f.arena.b, leaf + 6, range(0, 2));
    let anims = f.arena.put(&[9, 4, 2, 0, 1, 2, 4, 0]);
    blk(&mut f.arena.b, sp + 0x54, 2, anims);
    let destinations = |inputs| {
        let m = library(&f).material(SHAD_ID, inputs).unwrap();
        m.layers[0].passes[0].implementations[1].animated.iter()
            .map(|a| a.pixel_register).collect::<Vec<_>>()
    };
    assert_eq!(destinations(None), [Some((true, 1)), None]);
    assert_eq!(destinations(Some(EngineInputs::default())), [None, Some((false, 8))]);

    // A bitmap animation of slot 5 names the stage the implementation's
    // binding copies that slot to (stage 0), with the bitmap's image count;
    // the lightmap stage copies no slot. Its references are the range's.
    w16(&mut f.arena.b, leaf + 6, 0);
    w16(&mut f.arena.b, leaf, range(0, 1));
    w16(&mut f.arena.b, range_at, range(0, 2));
    let anims = f.arena.put(&[5, 0, 0, 0]);
    blk(&mut f.arena.b, sp + 0x54, 1, anims);
    let m = library(&f).material(SHAD_ID, None).unwrap();
    let a = &m.layers[0].passes[0].implementations[1].animated;
    assert_eq!(a.len(), 1);
    assert_eq!(a[0].kind, AnimationKind::Bitmap);
    assert_eq!(a[0].stages, [FrameStage { stage: 0, images: 2 }]);
    assert_eq!(a[0].properties.len(), 2);
    // A slot no stage copies feeds none.
    let anims = f.arena.put(&[6, 0, 0, 0]);
    blk(&mut f.arena.b, sp + 0x54, 1, anims);
    let m = library(&f).material(SHAD_ID, None).unwrap();
    assert!(m.layers[0].passes[0].implementations[1].animated[0].stages.is_empty());

    // A clear layer bit means no animation (0x000174d0).
    let lod_at = target(&f, sp + 0x1c);
    w32(&mut f.arena.b, lod_at, 1 << 1);
    let m = library(&f).material(SHAD_ID, None).unwrap();
    assert!(m.layers[0].passes[0].implementations[1].animated.is_empty());
}

#[test]
fn engine_vertex_constants_override_instance_vectors() {
    // Instance binding 3 now also targets c[20] (bank 1 reg 2, xyzw); the
    // engine binding writes c[20].xyz from vector 7 afterwards (0x00019be0
    // runs after 0x00017000), so w keeps the instance value.
    let mut f = fixture();
    let bind = target(&f, stem_pp(&f) + 0x20);
    w16(&mut f.arena.b, bind + 12, 2 | (1 << 4) | (13 << 5));
    let mut e = EngineInputs::default();
    e.ints[4] = 1;
    e.scalars[21] = 0.75;
    e.vectors[7] = [9.0, 10.0, 11.0];
    let m = library(&f).material(SHAD_ID, Some(e.clone())).unwrap();
    let c = c20(&m, 0);
    assert_eq!(c.value, [9.0, 10.0, 11.0, 8.0]);
    assert_eq!(c.origin, Origin::Engine);
    // Where vector 7 landed, so a caller can put another value there.
    assert_eq!(
        m.layers[0].passes[0].implementations[0].vector_bindings,
        vec![VectorBinding {
            source: 7,
            target: VectorTarget::Vertex {
                bank: 1,
                reg: 2,
                components: 0b0111
            }
        }]
    );
    // Without the engine binding the instance value stands.
    let imp0 = target(&f, spas_pp(&f));
    w16(&mut f.arena.b, imp0 + 0x124, 0);
    let m = library(&f).material(SHAD_ID, Some(e)).unwrap();
    let c = c20(&m, 0);
    assert_eq!(c.value, [5.0, 6.0, 7.0, 8.0]);
    assert_eq!(c.origin, Origin::Instance);
    assert!(m.layers[0].passes[0].implementations[0].vector_bindings.is_empty());
}

#[test]
fn a_vector_binding_keeps_only_what_later_bindings_leave() {
    let v = |reg, components| VectorTarget::Vertex { bank: 1, reg, components };
    let p = |slot, mask| VectorTarget::Pixel { c1: false, slot, mask, half: false, transform: 0 };
    let got = vector_bindings(&[
        (v(2, 0b0111), Some(5)),
        (v(2, 0b0001), None),
        (v(3, 0b0111), Some(6)),
        (p(1, 0x00ff_ffff), Some(5)),
        (p(1, 0x0000_00ff), None),
        (p(8, 0x00ff_ffff), Some(6)),
        (p(8, 0x00ff_ffff), Some(5)),
        (v(4, 0b0111), Some(6)),
        (v(4, 0b0111), None),
    ]);
    assert_eq!(
        got,
        vec![
            VectorBinding { source: 5, target: v(2, 0b0110) },
            VectorBinding { source: 6, target: v(3, 0b0111) },
            VectorBinding { source: 5, target: p(1, 0x00ff_ff00) },
            VectorBinding { source: 5, target: p(8, 0x00ff_ffff) },
        ],
        "overwritten lanes and wholly overwritten bindings dropped"
    );
    assert_eq!(pack_engine_colour([1.0, 0.5, 0.0, 0.0], false), 0x00ff_8000);
    assert_eq!(pack_engine_colour([1.0, 0.0, -1.0, 0.0], true), 0x80ff_8000);
    assert_eq!(pack_engine_colour([2.0, -1.0, 0.25, 1.0], false), 0xffff_0040);
}

/// An implementation's flag bit 20 (`+0x118`) is what makes a flagged
/// shader's draw call `0x0003bea0`; the resolved implementation carries it
/// and no other bit of the word.
#[test]
fn the_resolved_implementation_carries_flag_bit_20() {
    let mut f = fixture();
    let imp0 = target(&f, spas_pp(&f));
    let resolved = |f: &Fixture| library(f).material(SHAD_ID, None).unwrap().layers[0].passes[0].implementations[0].copies_target;
    assert!(!resolved(&f));
    w32(&mut f.arena.b, imp0 + 0x118, !(1 << 20));
    assert!(!resolved(&f), "every other bit");
    w32(&mut f.arena.b, imp0 + 0x118, 1 << 20);
    assert!(resolved(&f));
    let mut v = vec![0u8; IMPLEMENTATION];
    w32(&mut v, 0x118, 1 << 20);
    assert!(Implementation::decode(&v).unwrap().copies_target());
}

#[test]
fn stage_copies_slot_size_before_refresh() {
    let mut f = fixture();
    // Raw frame 1 < 2 images: the refresh runs (image 1 is 64x32, flag 0x10).
    let bm = target(&f, shad_pp(&f) + 0x04);
    w32(&mut f.arena.b, bm + 4, 1);
    // Bindings: slot 5 -> stage 0 (refresh), lightmap -> stage 1, slot 5 ->
    // stage 2 (refresh again).
    let spp = spas_pp(&f);
    let tex = f.arena.put(&[5, 0, 0, 2, 0xff, 11, 1, 0, 5, 0, 2, 2]);
    blk(&mut f.arena.b, spp + 0x08, 3, tex);
    // Engine vertex constants: c[20].xy = size of stage 0 (selector 1),
    // c[21].xy = size of stage 2 (selector 3), then the pixel constant.
    let mut ext = Vec::new();
    ext.extend((2u16 | (1 << 4) | (9 << 5) | (1 << 13)).to_le_bytes());
    ext.extend([0, 0]);
    ext.extend((3u16 | (1 << 4) | (9 << 5) | (3 << 13)).to_le_bytes());
    ext.extend([0, 0]);
    ext.extend(0x200u16.to_le_bytes());
    ext.extend([3 | (1 << 3), 21]);
    let ext = f.arena.put(&ext);
    blk(&mut f.arena.b, spp + 0x38, 3, ext);
    let imp0 = target(&f, spp);
    w16(&mut f.arena.b, imp0, range(0, 3));
    w16(&mut f.arena.b, imp0 + 0x124, range(0, 2));
    w16(&mut f.arena.b, imp0 + 0x122, range(2, 1));

    let m = library(&f).material(SHAD_ID, None).unwrap();
    let i = &m.layers[0].passes[0].implementations[0];
    let (t0, t2) = (i.textures[0].unwrap(), i.textures[2].unwrap());
    // 0x00018afb copies the (fresh, zero) slot to stage 0 before
    // 0x00018b90/0x00018ba2 refresh the slot.
    assert_eq!(t0.size, Some((64, 32)));
    assert_eq!(t0.stage_size, Some((0, 0)));
    // The second binding of the slot copies the refreshed size.
    assert_eq!(t2.stage_size, Some((64, 32)));
    assert_eq!(c20(&m, 0).value, [0.0; 4]);
    let c21 = i.vertex_constants.iter().find(|r| r.index == 21).unwrap();
    assert_eq!(c21.value, [64.0, 32.0, 0.0, 0.0]);
    // Without flags & 2 nothing is refreshed.
    let tex = target(&f, spp + 0x08);
    f.arena.b[tex + 3] = 0;
    f.arena.b[tex + 11] = 0;
    let m = library(&f).material(SHAD_ID, None).unwrap();
    let i = &m.layers[0].passes[0].implementations[0];
    assert_eq!(i.textures[0].unwrap().size, None);
    assert_eq!(i.textures[2].unwrap().stage_size, Some((0, 0)));
}

#[test]
fn only_inputs_outside_the_instance_make_a_pass_per_draw() {
    let pass = |externals: Vec<[u8; 4]>, fragments: Vec<i16>| PassPostprocess {
        implementations: Vec::new(),
        textures: Vec::new(),
        render_states: Vec::new(),
        stage_states: Vec::new(),
        fragments: fragments.into_iter().map(|selector| CombinerFragment { selector, base: 0 }).collect(),
        variants: Vec::new(),
        stages: Vec::new(),
        externals: externals.into_iter().map(|bytes| ExternalBinding { bytes }).collect(),
    };
    // Masks: scalar 21 and integer 5, the instance's own. One pixel
    // binding (range 0, count 1) and one fragment (range 0, count 1).
    let mut v = vec![0u8; IMPLEMENTATION];
    w32(&mut v, 0x10c, 1 << 21);
    w32(&mut v, 0x114, 1 << 5);
    w16(&mut v, 0x122, 1 << 9);
    w16(&mut v, 0xf6, 1 << 9);
    let imp = Implementation::decode(&v).unwrap();
    // Mode 3 (alpha) from scalar 21; a fragment chosen by integer 5.
    assert!(!per_draw_inputs(&pass(vec![[0, 1, 3, 21]], vec![5]), &imp));
    // Mode 3 from scalar 3; mode 4 (a vector); mode 5 (a stage size) is static.
    assert!(per_draw_inputs(&pass(vec![[0, 1, 3, 3]], vec![5]), &imp));
    assert!(per_draw_inputs(&pass(vec![[0, 1, 4, 21]], vec![5]), &imp));
    assert!(!per_draw_inputs(&pass(vec![[0, 1, 5, 3]], vec![-1]), &imp));
    // A fragment chosen by integer 2.
    assert!(per_draw_inputs(&pass(vec![[0, 1, 3, 21]], vec![2]), &imp));
    // A vector in the masks.
    w32(&mut v, 0x104, 1);
    assert!(per_draw_inputs(&pass(vec![[0, 1, 3, 21]], vec![5]), &Implementation::decode(&v).unwrap()));
}

/// `input_reads`: the masks, the engine constants' sources (pixel modes
/// 0..3 scalar and 4 vector in byte 2; vertex modes in bits 5..9 of the
/// word) and the fragment selectors, each kind in its own mask.
#[test]
fn input_reads_unions_masks_bindings_and_selectors() {
    let range = |start: u16, count: u8| IndexRange { start, count };
    let ext = |bytes: [u8; 4]| ExternalBinding { bytes };
    let frag = |selector: i16| CombinerFragment { selector, base: 0 };
    let imp = Implementation {
        textures: IndexRange::default(),
        render_states: IndexRange::default(),
        stage_states: IndexRange::default(),
        pixel_shader: [0; PSDEF_SIZE],
        fragments: range(0, 3),
        unknown_f8: [0; 4],
        vertex_shader: TagRef { group: map::FourCC(*b"xtrv"), id: map::TagId(u32::MAX) },
        scalar_inputs: 1 << 21,
        vector_inputs: 1 << 40,
        int_inputs: 1 << 4,
        unknown_118: 0,
        restore_states: IndexRange::default(),
        animated_render_states: IndexRange::default(),
        animated_stage_states: IndexRange::default(),
        external_pixel_constants: range(0, 3),
        external_vertex_constants: range(3, 2),
        unknown_126: [0; 12],
    };
    let spas = PassPostprocess {
        implementations: vec![imp.clone()],
        textures: Vec::new(),
        render_states: Vec::new(),
        stage_states: Vec::new(),
        // -1 is fixed; 6 an integer; 40 is out of the integers' range.
        fragments: vec![frag(-1), frag(6), frag(40)],
        variants: Vec::new(),
        stages: Vec::new(),
        externals: vec![
            ext([0, 0, 2, 16]),    // pixel, mode 2: scalar 16
            ext([0, 0, 4, 5]),     // pixel, mode 4: vector 5
            ext([0, 0, 6, 9]),     // pixel, mode 6: a stage size
            ext([4 << 5, 0, 0, 6]), // vertex, mode 4: vector 6
            ext([1 << 5, 0, 0, 3]), // vertex, mode 1: scalar 3
        ],
    };
    let r = input_reads(&spas, &imp);
    assert_eq!(r.scalars, 1 << 21 | 1 << 16 | 1 << 3);
    assert_eq!(r.vectors, 1 << 40 | 1 << 5 | 1 << 6);
    assert_eq!(r.ints, 1 << 4 | 1 << 6);
}

/// `0x0001a170`'s and `0x0001ab50`'s bindings: a render state's target in
/// byte 0, a stage state's stage in byte 0 and target in byte 1; source
/// 1 a scalar slot, any other a colour slot.
#[test]
fn an_engine_state_is_its_target_source_and_slot() {
    let b = |bytes| ExternalBinding { bytes };
    // Render target 2: ALPHAREF (0x3d), an integer, from scalar slot 5.
    let r = EngineState::render(b([2, 1, 0, 5])).unwrap();
    assert_eq!(r, EngineState { state: 0x3d, stage: None, kind: EngineStateKind::Integer, source: EngineStateSource::Scalar(5) });
    assert_eq!(r.name(), Some("ALPHAREF"));
    // Render target 6: FOGCOLOR (0x8a), a colour, from colour slot 1.
    let f = EngineState::render(b([6, 0, 0, 1])).unwrap();
    assert_eq!((f.state, f.kind, f.source), (0x8a, EngineStateKind::Colour, EngineStateSource::Colour(1)));
    assert_eq!(EngineState::render(b([7, 1, 0, 0])), None, "past the table");
    // Stage 2, target 0: MIPMAPLODBIAS (6), a float, from scalar slot 3.
    let s = EngineState::stage(b([2, 0, 1, 3])).unwrap();
    assert_eq!(s, EngineState { state: 6, stage: Some(2), kind: EngineStateKind::Float, source: EngineStateSource::Scalar(3) });
    assert_eq!(s.name(), Some("MIPMAPLODBIAS"));
    assert_eq!(EngineState::stage(b([4, 0, 1, 3])), None, "past the four stages");
    assert_eq!(EngineState::stage(b([0, 10, 1, 3])), None, "past the table");
}

/// `0x0001a170`'s and `0x0001ab50`'s conversions: a scalar rounded, as its
/// bits or as the top byte; a colour packed into the low 24 bits.
#[test]
fn an_engine_state_value_converts_its_input_by_kind() {
    let b = |bytes| ExternalBinding { bytes };
    let mut inputs = EngineInputs::default();
    inputs.scalars[2] = 0.5;
    inputs.scalars[3] = -0.25;
    inputs.scalars[5] = 127.5;
    inputs.scalars[6] = 126.5;
    inputs.scalars[7] = 1e10;
    inputs.vectors[1] = [1.0, 0.5, 0.0];
    inputs.vectors[4] = [1.5, 0.0, 0.0];
    inputs.vectors[63] = [0.0, 0.0, 1.0];
    let held = 0x1122_3344;
    let value = |bytes, stage: bool| {
        let s = if stage { EngineState::stage(b(bytes)) } else { EngineState::render(b(bytes)) };
        s.unwrap().value(&inputs, held)
    };
    // ALPHAREF from a scalar: rounded to nearest, ties to even.
    assert_eq!(value([2, 1, 0, 5], false), Some(128));
    assert_eq!(value([2, 1, 0, 6], false), Some(126));
    assert_eq!(value([2, 1, 0, 7], false), Some(0x8000_0000), "out of range: the integer indefinite");
    // BLENDCOLOR from a scalar: 0.5 x 255 rounded, the top byte.
    assert_eq!(value([0, 1, 0, 2], false), Some(0x8022_3344));
    // FOGCOLOR from a colour: red at 16, the top byte kept.
    assert_eq!(value([6, 0, 0, 1], false), Some(0x11ff_8000));
    // 1.5 x 255 = 382.5 rounds to 382 and wraps to 0x7e: no clamp.
    assert_eq!(value([6, 0, 0, 4], false), Some(0x117e_0000));
    // Stage 2 MIPMAPLODBIAS from a scalar: the float's bits.
    assert_eq!(value([2, 0, 1, 3], true), Some((-0.25f32).to_bits()));
    // Stage 1 BORDERCOLOR from colour slot 63.
    assert_eq!(value([1, 2, 0, 63], true), Some(0x1100_00ff));
    // A slot past the inputs.
    assert_eq!(value([2, 1, 0, 64], false), None);
    assert_eq!(value([6, 0, 0, 64], false), None);
}

/// `0x00019be0`'s word: register, bank, mode and stage, the input in
/// byte 3; a mode writes its components only.
#[test]
fn an_engine_vertex_constant_is_its_word() {
    // copy_texaccum_to_fb's c[18 + 1]: bank 1, mode 15 ([0, h, 0, 0]), stage 1.
    let w: u16 = 1 | 1 << 4 | 15 << 5 | 1 << 13;
    let c = EngineVertexConstant::of(ExternalBinding { bytes: [w as u8, (w >> 8) as u8, 0, 0] });
    assert_eq!(c, EngineVertexConstant { bank: 1, reg: 1, mode: 15, stage: 1, source: 0 });
    assert!(c.reads_size());
    assert_eq!(c.components(), 0b1111);
    let mut row = [7.0; 4];
    c.eval(&mut row, (320.0, 240.0), &EngineInputs::default()).unwrap();
    assert_eq!(row, [0.0, 240.0, 0.0, 0.0]);
    // Mode 3 from scalar 41: w only.
    let s = EngineVertexConstant::of(ExternalBinding { bytes: [3 << 5, 0, 0, 41] });
    assert_eq!((s.mode, s.source, s.reads_size()), (3, 41, false));
    let mut e = EngineInputs::default();
    e.scalars[41] = 0.5;
    let mut row = [7.0; 4];
    s.eval(&mut row, (0.0, 0.0), &e).unwrap();
    assert_eq!(row, [7.0, 7.0, 7.0, 0.5]);
    let spas = PassPostprocess {
        implementations: Vec::new(),
        textures: Vec::new(),
        render_states: Vec::new(),
        stage_states: Vec::new(),
        fragments: Vec::new(),
        variants: Vec::new(),
        stages: Vec::new(),
        externals: vec![ExternalBinding { bytes: [0, 0, 3, 41] }, ExternalBinding { bytes: [3 << 5, 0, 0, 41] }],
    };
    let mut v = vec![0u8; IMPLEMENTATION];
    w16(&mut v, 0x124, range(1, 1));
    let imp = Implementation::decode(&v).unwrap();
    assert_eq!(engine_vertex_constants(&spas, &imp), vec![s], "the vertex range only");
}

const SHAD2_ID: TagId = TagId(0xe005_0006);
const MATG_ID: TagId = TagId(0xe006_0007);

/// The fixture with stage 1 bound to engine texture 26 (in place of the
/// lightmap) and the shader's `+0x5c` block naming bitmap parameter
/// `index`.
fn property_fixture(index: i16) -> Fixture {
    let mut f = fixture();
    let tex = target(&f, spas_pp(&f) + 0x08);
    f.arena.b[tex + 5] = PROPERTY_BITMAP_TEXTURE;
    let mut block = vec![0xffu8; 0x14];
    block[0x10..0x12].copy_from_slice(&index.to_le_bytes());
    let block = f.arena.put(&block);
    let sp = shad_pp(&f);
    blk(&mut f.arena.b, sp + 0x5c, 5, block);
    f
}

fn stage1(f: &Fixture) -> StageTexture {
    let m = library(f).material(SHAD_ID, None).unwrap();
    m.layers[0].passes[0].implementations[0].textures[1].unwrap()
}

/// `0x00019a68`: the index names the shader's own parameter, bound through
/// `0x000143c0` with no frame wrap and no LOD offset, and the stage takes
/// the image's size.
#[test]
fn engine_texture_26_binds_the_bitmap_parameter_the_shader_names() {
    let mut f = property_fixture(0);
    // Image 3 of 2: `0x000143c0` takes the rasterizer default texture.
    assert_eq!(stage1(&f), StageTexture { source: TextureSource::Default, size: None, stage_size: None });
    let bm = target(&f, shad_pp(&f) + 0x04);
    // Only the low half of +4 is the image (`mov cx, word [eax+4]`).
    w32(&mut f.arena.b, bm + 4, 0x0007_0001);
    assert_eq!(
        stage1(&f),
        StageTexture {
            source: TextureSource::Bitmap { bitmap: BITM_ID, image: 1, lod_offset: 0.0 },
            size: None,
            stage_size: Some((64, 32)),
        }
    );
    // A null bitmap is the default texture.
    w32(&mut f.arena.b, bm, TagId::NULL.0);
    assert_eq!(stage1(&f).source, TextureSource::Default);
    // An index past the parameters, or a block that does not read, leaves
    // the engine texture for a consumer to refuse.
    let f = property_fixture(1);
    assert_eq!(stage1(&f).source, TextureSource::Engine(PROPERTY_BITMAP_TEXTURE));
    let mut f = property_fixture(0);
    let sp = shad_pp(&f);
    blk(&mut f.arena.b, sp + 0x5c, 5, BASE - 0x100);
    assert_eq!(stage1(&f).source, TextureSource::Engine(PROPERTY_BITMAP_TEXTURE));
}

/// An index of -1 takes the globals' rasterizer shader (`matg +0x108`, its
/// first element's `+0xd0`) and the parameter its own index names, which
/// is not checked for -1 again.
#[test]
fn engine_texture_26_of_minus_one_takes_the_rasterizer_shaders_parameter() {
    let mut f = property_fixture(-1);
    // Without globals the engine texture stays.
    assert_eq!(stage1(&f).source, TextureSource::Engine(PROPERTY_BITMAP_TEXTURE));
    // The rasterizer shader: a copy of the fixture's whose postprocess
    // names its parameter 0, image 1 of the fixture's bitmap.
    let mut bm = vec![0u8; 0xc];
    w32(&mut bm, 0, BITM_ID.0);
    w32(&mut bm, 4, 1);
    w32(&mut bm, 8, 0.5f32.to_bits());
    let bm = f.arena.put(&bm);
    let mut block = vec![0u8; 0x14];
    block[0x10..0x12].copy_from_slice(&0i16.to_le_bytes());
    let block = f.arena.put(&block);
    let sp = shad_pp(&f);
    let mut sp2 = f.arena.b[sp..sp + SHAD_POSTPROCESS].to_vec();
    blk(&mut sp2, 0x04, 1, bm);
    blk(&mut sp2, 0x5c, 5, block);
    let sp2 = f.arena.put(&sp2);
    let root = (f.tags[0].2 - BASE) as usize;
    let mut shad2 = f.arena.b[root..root + SHAD_ROOT].to_vec();
    blk(&mut shad2, 0x20, 1, sp2);
    let shad2 = f.arena.put(&shad2);
    let mut element = vec![0u8; 0xd4];
    w32(&mut element, 0xd0, SHAD2_ID.0);
    let element = f.arena.put(&element);
    let mut matg = vec![0u8; 0x110];
    blk(&mut matg, 0x108, 1, element);
    let matg = f.arena.put(&matg);
    f.tags.push((SHAD2_ID, SHAD, shad2));
    f.tags.push((MATG_ID, FourCC(*b"matg"), matg));
    assert_eq!(
        stage1(&f),
        StageTexture {
            source: TextureSource::Bitmap { bitmap: BITM_ID, image: 1, lod_offset: 0.0 },
            size: None,
            stage_size: Some((64, 32)),
        }
    );
    // The rasterizer shader's own -1 is not followed again.
    let at = (block - BASE) as usize + 0x10;
    f.arena.b[at..at + 2].copy_from_slice(&(-1i16).to_le_bytes());
    assert_eq!(stage1(&f).source, TextureSource::Engine(PROPERTY_BITMAP_TEXTURE));
}

#[test]
fn particles_draw_their_embedded_postprocess_and_model_particles_their_shad_under_zero_inputs() {
    const PRT3_ID: TagId = TagId(0xe005_0006);
    const PRTM_ID: TagId = TagId(0xe006_0007);
    let mut f = fixture();
    // prt3 +0x8c points at the shad's own postprocess element; PRTM +0x18
    // references the shad.
    let shad_root = (f.tags[0].2 - BASE) as usize;
    let sp_at = u32_in(&f.arena.b, shad_root + 0x24);
    let mut prt3 = vec![0u8; PRT3_ROOT];
    blk(&mut prt3, PRT3_POSTPROCESS_BLOCK, 1, sp_at);
    let prt3_at = f.arena.put(&prt3);
    let mut prtm = vec![0u8; PRTM_ROOT];
    w32(&mut prtm, PRTM_SHADER, u32::from_be_bytes(*b"shad"));
    w32(&mut prtm, PRTM_SHADER + 4, SHAD_ID.0);
    let prtm_at = f.arena.put(&prtm);
    let mut empty = vec![0u8; PRT3_ROOT];
    blk(&mut empty, PRT3_POSTPROCESS_BLOCK, 0, 0);
    let empty_at = f.arena.put(&empty);
    f.tags.extend([(PRT3_ID, PRT3, prt3_at), (PRTM_ID, PRTM, prtm_at), (TagId(0xe007_0008), PRT3, empty_at)]);
    let mut lib = library(&f);
    let selection = PassSelection { lod: 0, layer: 2, pass: 0, implementation: 0 };

    let shad = lib.drawn_shader(SHAD_ID).unwrap();
    assert_eq!((shad.shader, shad.template), (SHAD_ID, STEM_ID));
    assert_eq!(shad.inputs, EngineInputs::for_shader(&lib.shader(SHAD_ID).unwrap()));
    let own = lib.selected_implementation(SHAD_ID, selection, None, None).unwrap().implementation;
    assert_eq!(lib.selected_drawn(&shad, selection).unwrap().implementation, own);

    let zero = lib.selected_implementation(SHAD_ID, selection, Some(&EngineInputs::default()), None).unwrap().implementation;
    assert_ne!(zero, own, "the shad's scalar 21 reaches the final combiner");
    for id in [PRT3_ID, PRTM_ID] {
        let drawn = lib.drawn_shader(id).unwrap();
        assert_eq!(drawn.template, STEM_ID);
        assert_eq!(drawn.inputs, EngineInputs::default());
        assert_eq!(lib.selected_drawn(&drawn, selection).unwrap().implementation, zero);
    }
    assert_eq!(lib.drawn_shader(PRT3_ID).unwrap().shader, PRT3_ID);
    assert_eq!(lib.drawn_shader(PRTM_ID).unwrap().shader, SHAD_ID);
    assert!(lib.drawn_shader(TagId(0xe007_0008)).is_err(), "a prt3 without its postprocess");
    assert!(lib.drawn_shader(BITM_ID).is_err());
    assert!(lib.drawn_shader(TagId(0xe0ff_00ff)).is_err());
}
