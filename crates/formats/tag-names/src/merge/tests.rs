use std::path::Path;

use tags::defs::{Defs, parse_fourcc};
use tags::registry::Registry;
use tags::schema::Schema;

use super::*;
use crate::{Source, assembly};

fn plugin(size: u32, fields: &str) -> Plugin {
    let xml = format!(r#"<plugin game="Halo2Xbox" baseSize="{size}">{fields}</plugin>"#);
    assembly::parse("zzzz", &xml).unwrap()
}

fn definition(size: u32, fields: &str) -> StructDef {
    let mut defs = Defs::default();
    let text = format!("group zzzz {size}\n{fields}\n");
    defs.parse_into("invented", &text).unwrap();
    defs.groups.remove(&parse_fourcc("zzzz").unwrap()).unwrap()
}

#[test]
fn our_name_and_description_precede_the_plugin_alias_and_comment() {
    let def = definition(4, r#"  0 f32 invented_spin doc="Our invented rate.""#);
    let ext = plugin(4, r#"<float32 offset="0" name="Invented whirl" comment="Invented note."/>"#);
    let result = merge(Some(&def), &ext, 4);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.named, 0);
    assert_eq!(result.layout.fields[0].name, "invented_spin");
    let meta = result.rows.get(&[0][..]).unwrap();
    assert_eq!(meta.name_source, NameSource::Ours);
    assert_eq!(meta.theirs.as_deref(), Some("Invented whirl"));
    assert_eq!(meta.description, "Our invented rate.\nAssembly: Invented whirl — Invented note.");
}

#[test]
fn tiled_padding_and_unknown_words_take_external_types_and_value_labels() {
    let def = definition(12, "  0 pad 4\n  4 unknown 8");
    let ext = plugin(12, r#"
      <enum16 offset="0" name="Invented dial">
        <option value="41000" name="Invented distant stop"/>
        <option value="-1" name="Invented last stop"/>
      </enum16>
      <bitfield16 offset="2" name="Invented mask" comment="Invented switches.">
        <bit index="3" name="Invented switch"/>
      </bitfield16>
      <vector2 offset="4" name="Invented sweep"/>
    "#);
    let result = merge(Some(&def), &ext, 12);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.named, 3);
    let fields = &result.layout.fields;
    assert_eq!(fields.len(), 3);
    assert!(matches!(fields[0].kind, Type::Enum { bytes: 2, .. }));
    assert!(matches!(&fields[1].kind, Type::Flags { bits, .. } if bits[3] == "Invented switch"));
    assert!(matches!(fields[2].kind, Type::Float { count: 2, .. }));
    let dial = result.rows.get(&[0][..]).unwrap();
    assert_eq!(dial.options[&41000], "Invented distant stop");
    assert_eq!(dial.options[&65535], "Invented last stop");
    assert_eq!(dial.name_source, NameSource::External { source: "assembly" });
    assert_eq!(result.rows.get(&[2][..]).unwrap().description, "Assembly: Invented switches.");
}

#[test]
fn indexed_references_packed_components_and_bounds_keep_their_wire_types() {
    let def = definition(
        24,
        "  0 datum invented_link\n  4 i16[2] invented_pair\n  8 word invented_ink\n  12 unknown 12",
    );
    let ext = plugin(
        24,
        r#"
        <tagRef withGroup="false" offset="0" name="Invented pointer"/>
        <point16 offset="4" name="Invented origin"/>
        <color32 offset="8" name="Invented dye"/>
        <rangef offset="12" name="Invented interval"/>
        <float32 offset="20" name="Invented measure"/>
        "#,
    );
    let result = merge(Some(&def), &ext, 24);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.named, 2);
    assert_eq!(result.layout.fields[0].name, "invented_link");
    assert_eq!(result.layout.fields[1].name, "invented_pair");
    assert_eq!(result.layout.fields[2].name, "invented_ink");
    assert!(matches!(
        result.layout.fields[3].kind,
        Type::Float { hint: Hint::Bounds, count: 2 }
    ));
}

#[test]
fn overruns_overlaps_and_incomplete_tiles_preserve_the_original_range() {
    let def = definition(8, "  0 unknown 4\n  4 u32 invented_anchor");
    for bad in [
        r#"<uint16 offset="0" name="Invented incomplete"/>"#,
        r#"<uint32 offset="0" name="Invented collision"/><uint16 offset="2" name="Invented overlap"/>"#,
        r#"<vector2 offset="0" name="Invented overrun"/>"#,
        r#"<inventedType offset="0" name="Invented opaque"/>"#,
    ] {
        let fields = format!(r#"{bad}<uint32 offset="4" name="Invented mate"/>"#);
        let result = merge(Some(&def), &plugin(8, &fields), 8);
        assert!(!result.conflicts.is_empty(), "{bad}");
        assert_eq!(result.conflicts[0].group, "zzzz");
        assert_eq!(result.conflicts[0].path, "+0x0");
        assert_eq!(result.named, 0);
        assert_eq!(result.layout.fields[0].name, "+0x0");
        assert!(!result.layout.fields[0].named);
        assert!(!result.rows.contains_key(&[0][..]));
    }
}

#[test]
fn conflicting_width_type_and_missing_fields_keep_our_definition() {
    let def = definition(12, "  0 u32 invented_width\n  4 f32 invented_shape\n  8 u32 invented_missing");
    let ext = plugin(12, r#"
      <uint16 offset="0" name="Invented narrower"/>
      <uint32 offset="4" name="Invented integer"/>
    "#);
    let result = merge(Some(&def), &ext, 12);
    let paths: Vec<_> = result.conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["+0x0", "+0x4", "+0x8"]);
    assert!(result.rows.is_empty());
    assert_eq!(result.layout.fields[1].name, "invented_shape");
    assert_eq!(result.conflicts[2].theirs, "missing");
}

#[test]
fn nested_merges_require_matching_block_strides() {
    let def = definition(8, "  0 block invented_coils 4 align 4\n    0 unknown 4");
    let block = |stride| plugin(8, &format!(r#"
      <tagblock offset="0" name="Invented rings" elementSize="{stride}">
        <float32 offset="0" name="Invented turn"/>
      </tagblock>
    "#));
    let result = merge(Some(&def), &block(8), 8);
    assert_eq!(result.conflicts.len(), 1);
    assert!(result.rows.is_empty());
    let Type::Block(child) = &result.layout.fields[0].kind else {
        panic!("expected block")
    };
    assert_eq!(child.size, 4);
    assert_eq!(child.fields[0].name, "+0x0");

    let result = merge(Some(&def), &block(4), 8);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.layout.fields[0].name, "invented_coils");
    let Type::Block(child) = &result.layout.fields[0].kind else {
        panic!("expected block")
    };
    assert_eq!(child.fields[0].name, "Invented turn");
    assert_eq!(result.rows.get(&[0, 0][..]).unwrap().name_source.as_str(), "assembly");
}

#[test]
fn plugin_only_fields_fit_the_tag_and_each_child_element() {
    let ext = plugin(20, r#"
      <uint32 offset="16" name="Invented root overrun"/>
      <tagblock offset="4" name="Invented rows" elementSize="2">
        <uint16 offset="0" name="Invented index"/>
        <uint16 offset="2" name="Invented child overrun"/>
      </tagblock>
      <int16 offset="0" name="Invented root"/>
    "#);
    let result = merge(None, &ext, 12);
    assert_eq!(result.layout.size, 12);
    assert_eq!(result.named, 3);
    assert_eq!(result.layout.fields[0].name, "Invented root");
    assert!(result.layout.field_at(16).is_none());
    let Type::Block(child) = &result.layout.field_at(4).unwrap().kind else {
        panic!("expected block")
    };
    assert_eq!(child.size, 2);
    assert_eq!(child.fields.len(), 1);
    assert_eq!(child.fields[0].name, "Invented index");
    let paths: Vec<_> = result.conflicts.iter().map(|c| c.path.as_str()).collect();
    assert_eq!(paths, ["+0x4.+0x2", "+0x10"]);
}

#[test]
fn hidden_fields_fill_a_tile_without_lending_it_their_name() {
    let def = definition(4, "  0 unknown 4");
    let ext = plugin(4, r#"
      <int16 offset="0" visible="false" name="Invented hidden"/>
      <int16 offset="2" name="Invented visible"/>
    "#);
    let result = merge(Some(&def), &ext, 4);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.named, 1);
    assert!(matches!(result.layout.fields[0].kind, Type::Pad { len: 2 }));
    assert_eq!(result.layout.fields[1].name, "Invented visible");
}

#[test]
fn vectors_respect_declared_spans_and_ignore_editor_word_boundaries() {
    let ext = plugin(8, r#"<vector2 offset="0" name="Invented span"/>"#);
    let split = definition(8, "  0 unknown 4\n  4 unknown 4");
    let result = merge(Some(&split), &ext, 8);
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].path, "+0x0");
    assert_eq!(*result.layout, Layout::from_definition(&split));
    assert!(result.rows.is_empty());

    let whole = definition(8, "  0 unknown 8");
    let result = merge(Some(&whole), &ext, 8);
    assert!(result.conflicts.is_empty());
    assert_eq!(result.layout.fields.len(), 1);
    assert!(matches!(result.layout.fields[0].kind, Type::Float { count: 2, .. }));
}

#[test]
fn embedded_boundaries_inside_a_block_remain_visible_to_the_audit() {
    let mut defs = Defs::default();
    defs.parse_into(
        "invented",
        "struct slices 4\n  0 unknown 2\n  2 unknown 2\n\
         group zzzz 8\n  0 block invented_bins 4 align 4\n    0 struct invented_slices slices",
    )
    .unwrap();
    let ext = plugin(
        8,
        r#"<tagblock offset="0" name="Invented bins" elementSize="4">
             <uint32 offset="0" name="Invented long span"/>
           </tagblock>"#,
    );
    let code = parse_fourcc("zzzz").unwrap();
    let registry = Registry::build(&Schema::default(), &defs);
    let result = compare(
        registry.group(code).map(Arc::as_ref),
        defs.group(code),
        &ext,
        8,
    );
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].path, "+0x0.+0x0");
    assert_eq!(result.named, 0);
    assert!(!result.rows.contains_key(&[0, 0][..]));
    assert_eq!(result.layout, *registry.group(code).unwrap());
}

#[test]
fn a_reference_in_opaque_bytes_requires_a_canonical_fixup() {
    let def = definition(16, "  0 unknown 16");
    let schema = Schema::parse("group zzzz 0x10 align 4\n  0x8 tagref\n").unwrap();
    let code = parse_fourcc("zzzz").unwrap();
    let mut defs = Defs::default();
    defs.groups.insert(code, def);
    let registry = Registry::build(&schema, &defs);
    let ext = plugin(
        16,
        r#"<tagref offset="0" name="Invented unsupported reference"/>
           <tagref offset="8" name="Invented evidence-backed reference"/>"#,
    );
    let result = compare(
        registry.group(code).map(Arc::as_ref),
        defs.group(code),
        &ext,
        16,
    );
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].path, "+0x0");
    assert!(result.conflicts[0].ours.contains("without a fixup"));
    assert_eq!(result.named, 1);
    let first = result.layout.field_at(0).unwrap();
    assert!(!first.named);
    assert!(matches!(first.kind, Type::Unknown { .. }));
    let reference = result.layout.field_at(8).unwrap();
    assert_eq!(reference.name, "Invented evidence-backed reference");
    assert!(matches!(reference.kind, Type::TagRef { .. }));
}

#[test]
fn unequal_root_sizes_are_conflicts_even_when_all_plugin_fields_fit() {
    let def = definition(8, "  0 f32 invented_span\n  4 unknown 4");
    for size in [4, 12] {
        let ext = plugin(size, r#"<float32 offset="0" name="Invented alias"/>"#);
        let result = merge(Some(&def), &ext, 8);
        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].path, "root");
        assert_eq!(result.conflicts[0].ours, "root size 0x8");
        assert_eq!(result.conflicts[0].theirs, format!("baseSize {size:#x}"));
        assert_eq!(result.layout.fields[0].name, "invented_span");
    }
}

fn hint_plugin(game: &str, size: u32, fields: &str) -> Plugin {
    let xml = format!(r#"<plugin game="{game}" baseSize="{size}">{fields}</plugin>"#);
    assembly::parse_game("zzzz", game, &xml).unwrap()
}

fn hint_names(def: &StructDef, plugin: &Plugin) -> Vec<(Vec<u32>, String)> {
    hints(&Layout::from_definition(def), plugin)
        .into_iter()
        .map(|h| {
            assert_eq!(h.game, plugin.game);
            (h.path, h.name)
        })
        .collect()
}

#[test]
fn hints_need_an_element_of_the_same_size_whose_named_fields_line_up() {
    let def = definition(0xc, "  0 f32 invented_rate\n  4 unknown 8");
    let fields = r#"<float32 offset="0" name="Invented other rate"/>
        <int32 offset="4" name="Invented count"/>
        <float32 offset="8" name="Invented scale" comment="Invented note."/>"#;
    let same = hint_plugin("Halo3", 0xc, fields);
    assert_eq!(
        hint_names(&def, &same),
        [(vec![4], "Invented count".into()), (vec![8], "Invented scale".into())]
    );
    assert_eq!(hints(&Layout::from_definition(&def), &same)[1].comment, "Invented note.");
    // Our named field no longer lines up: no hint, whatever the offsets.
    let moved = hint_plugin("Halo3", 0xc, r#"<int32 offset="0" name="Invented flags"/>
        <int32 offset="4" name="Invented count"/>"#);
    assert!(hint_names(&def, &moved).is_empty());
    // A larger element with no named field after the hint.
    let larger = hint_plugin("Halo3", 0x10, fields);
    assert!(hint_names(&def, &larger).is_empty());
}

#[test]
fn hints_in_a_different_size_element_need_both_neighbours() {
    let def = definition(
        0x10,
        "  0 f32 invented_first\n  4 unknown 4\n  8 f32 invented_last\n  0xc unknown 4",
    );
    let lined = hint_plugin("Reach", 0x40, r#"<float32 offset="0" name="Invented a"/>
        <enum16 offset="4" name="Invented half"/><int16 offset="6" name="Invented other half"/>
        <float32 offset="8" name="Invented b"/><float32 offset="0xc" name="Invented tail"/>"#);
    // +4 holds two plugin fields, which do not fill our word: no hint.
    // +0xc has no named field after it.
    assert!(hint_names(&def, &lined).is_empty());
    let word = hint_plugin("Reach", 0x40, r#"<float32 offset="0" name="Invented a"/>
        <bitfield32 offset="4" name="Invented mask"/>
        <float32 offset="8" name="Invented b"/>"#);
    assert_eq!(hint_names(&def, &word), [(vec![4], "Invented mask".into())]);
    let shifted = hint_plugin("Reach", 0x40, r#"<float32 offset="0" name="Invented a"/>
        <bitfield32 offset="4" name="Invented mask"/>
        <stringId offset="8" name="Invented b"/>"#);
    assert!(hint_names(&def, &shifted).is_empty());
}

#[test]
fn hints_enter_blocks_that_line_up_and_skip_wider_pointers() {
    let def = definition(
        0x10,
        "  0 block parts 0x8 align 4\n    0 f32 invented_weight\n    4 unknown 4\n  8 tagref tagref_8",
    );
    let narrow = hint_plugin("Halo2MCC", 0x10, r#"<tagblock offset="0" name="Invented parts" elementSize="8">
          <float32 offset="0" name="Invented mass"/><float32 offset="4" name="Invented spin"/>
        </tagblock>
        <tagRef offset="8" name="Invented link"/>"#);
    assert_eq!(
        hint_names(&def, &narrow),
        [(vec![0, 4], "Invented spin".into()), (vec![8], "Invented link".into())]
    );
    let wide = hint_plugin("Halo3", 0x14, r#"<tagblock offset="0" name="Invented parts" elementSize="8">
          <float32 offset="0" name="Invented mass"/><float32 offset="4" name="Invented spin"/>
        </tagblock>
        <tagRef offset="8" name="Invented link"/>"#);
    // The 12-byte block no longer fits our 8-byte one, and the 16-byte
    // reference no longer fits ours.
    assert!(hint_names(&def, &wide).is_empty());
}

#[test]
#[ignore = "uses the developer's local field definitions"]
fn every_plugin_merges_against_our_definitions() {
    let Some(dir) = std::env::var_os("H2_FIELD_NAMES") else {
        return;
    };
    let folder = assembly::Folder::open(Path::new(&dir)).unwrap();
    let others = assembly::Hints::open(Path::new(&dir)).unwrap();
    let registry = Registry::builtin();
    let groups = folder.groups();
    let mut total = tags::registry::Coverage::default();
    let mut added = 0;
    let mut conflicts = 0;
    for group in &groups {
        let plugin = folder.load(group).unwrap().expect("indexed plugin");
        let code = parse_fourcc(group).expect("indexed group");
        let ours = registry.group(code);
        let size = ours.map_or(plugin.base_size, |layout| layout.size);
        let result = compare(ours.map(Arc::as_ref), Defs::builtin().group(code), &plugin, size);
        let coverage = result.layout.coverage();
        println!(
            "{group} ({}): {} of {} fields named ({} external), {} conflicts",
            plugin.game,
            coverage.named_fields,
            coverage.named_fields + coverage.unnamed_fields,
            result.named,
            result.conflicts.len()
        );
        for other in others.load(group).unwrap() {
            let found = hints(&result.layout, &other);
            if !found.is_empty() {
                println!("  {} hints from {}", found.len(), other.game);
            }
        }
        for conflict in &result.conflicts {
            println!(
                "  {} {}: ours [{}], theirs [{}]",
                conflict.group, conflict.path, conflict.ours, conflict.theirs
            );
        }
        total.add(&coverage);
        added += result.named;
        conflicts += result.conflicts.len();
    }
    println!(
        "{} groups: {} of {} fields named ({added} external), {conflicts} conflicts",
        groups.len(),
        total.named_fields,
        total.named_fields + total.unnamed_fields
    );
}
