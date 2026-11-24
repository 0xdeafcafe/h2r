//! The Shader tab's read over the scenario crate's invented material
//! (`fixture::fixture_with_shader_colour`: one base map, one colour bound
//! to the final combiner, one pass in layer 5; no game data).
use super::*;
use scenario_edit::fixture::{self, LAYERED_LAYER, SHADER_COLOUR};
use scenario_edit::native_view::preview::MaterialDraws;

fn library(bytes: Vec<u8>) -> ShaderLibrary {
    let mut map = map::Map::from_reader(std::io::Cursor::new(bytes)).unwrap();
    ShaderLibrary::read(&mut map).unwrap()
}

fn shader() -> TagId {
    fixture::id(6)
}

#[test]
fn the_invented_material_reads_its_slots_and_what_reads_them() {
    let mut lib = library(fixture::fixture_with_shader_colour(LAYERED_LAYER));
    let r = read_from(&mut lib, shader()).unwrap();
    assert_eq!(r.template, fixture::id(10));
    assert_eq!((r.lod, r.lods), (0, 1));
    assert!(!r.names_known);
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    assert_eq!(
        r.passes,
        BTreeMap::from([(usize::from(LAYERED_LAYER), 1)])
    );

    // The base map: no binding reads it at this level of detail.
    assert_eq!(r.bitmaps.len(), 1);
    assert_eq!(
        r.bitmaps[0].value,
        SlotValue::Bitmap {
            bitmap: fixture::id(7),
            image: 0
        }
    );
    assert!(r.bitmaps[0].used_by.is_empty());
    assert!(r.bitmaps[0].label().ends_with("not read at this level of detail"));

    // The colour: the final combiner's c0 of the one pass.
    assert_eq!(r.colours.len(), 1);
    assert_eq!(r.colours[0].value, SlotValue::Colour(SHADER_COLOUR));
    assert_eq!(
        r.colours[0].used_by,
        [UsedBy {
            layer: usize::from(LAYERED_LAYER),
            pass: 0,
            implementation: 0,
            reads: "final combiner c0".into(),
        }]
    );
    assert_eq!(
        r.colours[0].label(),
        "Colour 0 · final combiner c0 of pass 0 in layer 5 (implementation 0)"
    );
    assert_eq!(r.colours[0].path(), "postprocess[0].colors[0].color");
    assert!(r.vectors.is_empty());
    assert!(r.animations.is_empty());
}

/// The layered map's shader has no colour, so its template's binding
/// would name a slot the shader lacks: that is said, not dropped.
#[test]
fn a_binding_past_the_shaders_slots_is_named() {
    let mut bytes = fixture::fixture_with_shader_colour(LAYERED_LAYER);
    // The shader's colour list emptied again.
    fixture::put(&mut bytes, fixture::META_FILE + fixture::TEX_SHAD + 0x60 + 0xc, 0);
    let mut lib = library(bytes);
    let r = read_from(&mut lib, shader()).unwrap();
    assert!(r.colours.is_empty());
    assert_eq!(
        r.problems,
        ["the template binds colour 0, which the shader does not have"]
    );
}

#[test]
fn binding_words_read_as_registers() {
    assert_eq!(colour_reads(0x200), "final combiner c0");
    assert_eq!(colour_reads(0x600), "final combiner c1");
    assert_eq!(colour_reads(3 << 6), "combiner stage 3 c0");
    assert_eq!(vector_reads(2), "vertex default v2");
    assert_eq!(vector_reads(0x10 | 5), "vertex constant c23");
    for k in SlotKind::ALL {
        assert_eq!(SlotKind::from_id(k.id()), Some(k));
    }
    assert_eq!(SlotKind::from_id("color"), Some(SlotKind::Colour));
    assert_eq!(SlotKind::from_id("scalar"), None);
}

/// Drawn and refused layers count their passes; the base look with no
/// layer-3 pass of its own counts as one.
#[test]
fn preview_passes_count_each_layers_passes() {
    let mut lib = library(fixture::fixture_with_shader_colour(LAYERED_LAYER));
    let read = read_from(&mut lib, shader()).unwrap();
    let draws = MaterialDraws {
        base: None,
        drawn: vec![LAYERED_LAYER],
        refused: vec![(1, "layer 1: stencil mode not checked against the game".into())],
        unresolved: Vec::new(),
    };
    let p = PreviewPasses::of(&read, &draws);
    assert_eq!(p.drawn, 1);
    assert_eq!(p.refused.len(), 1);
    assert_eq!(
        p.sentences(),
        ["layer 1 (1 pass) is left out: layer 1: stencil mode not checked against the game"]
    );
    assert_eq!(p.json()["passes_drawn"], 1);
    assert_eq!(p.json()["refusals"][0]["layer"], 1);
}
