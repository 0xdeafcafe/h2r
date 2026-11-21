//! All sources and tag fields in these tests are invented.
use std::{
    fs,
    io::Cursor,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use content::{Library, MapId, ResolveError, Sha256};
use forge_build::{Built, Error, KIT_RECIPE, VERSIONS, build};
use forge_doc::{
    BlockKind, Edit, Expected, F32, FORMAT, FieldWrite, ForgeDoc, MapRef, Op, OpId, Placement,
    Recipe, TagKey, TagWrite, Value,
};
use map::{Map, scenario_layout};
use scenario_edit::{MapSource, Scenario, fixture};
use tags::registry::{Kind, Registry};

fn source(bytes: Vec<u8>) -> MapSource {
    let mut source = MapSource::file("invented/fixture.map");
    source.bytes = Some(bytes.into());
    source
}

fn draft(input: &MapSource) -> ForgeDoc {
    ForgeDoc {
        format: FORMAT,
        builder: VERSIONS,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(input.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "Test map".into(),
        author: "Editor".into(),
        description: "Invented inputs".into(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
    }
}

fn materialise(doc: &ForgeDoc, source: &MapSource) -> Result<Built, Error> {
    build(doc, |name| (name == "fixture").then(|| source.clone()))
}

fn edit() -> Op {
    Op {
        id: OpId(1),
        edit: Edit::Placement(Placement::Set {
            block: BlockKind::Weapons,
            index: 0,
            write: FieldWrite {
                field: "scale".into(),
                value: Value::Float(F32::new(2.0).unwrap()),
            },
        }),
    }
}

#[test]
fn two_builds_of_a_document_produce_identical_bytes() {
    let source = source(fixture::fixture_at_end());
    let mut doc = draft(&source);
    doc.ops.push(edit());
    let first = materialise(&doc, &source).unwrap();
    let second = materialise(&doc, &source).unwrap();
    assert_eq!(first.map_bytes(), second.map_bytes());
    assert_eq!(first.manifest(), second.manifest());
    assert!(first.reserves.is_empty());
    assert_eq!(
        first.manifest().map_id(),
        MapId::forge(doc.base.sha256, doc.id().unwrap())
    );
    let mut map = Map::from_reader(Cursor::new(first.map_bytes())).unwrap();
    let scenario = Scenario::read(&mut map).unwrap();
    assert_eq!(
        scenario
            .get(BlockKind::Weapons, 0, "scale")
            .unwrap()
            .as_f32(),
        Some(2.0)
    );
}

#[test]
fn stale_base_and_bad_expect_are_named_build_errors() {
    let source = source(fixture::fixture_at_end());
    let mut doc = draft(&source);
    let correct = doc.base.sha256;
    doc.base.sha256 = Sha256([0x66; 32]);
    assert!(
        matches!(materialise(&doc, &source), Err(Error::BaseHash { map, actual, .. })
        if map == "fixture" && actual == correct)
    );
    doc.base.sha256 = correct;

    let id = MapId::forge(correct, doc.id().unwrap());
    let bad_id = MapId([0xff; 32]);
    doc.expect = Some(Expected {
        map_id: bad_id,
        geometry: Sha256([0; 32]),
    });
    assert!(
        matches!(materialise(&doc, &source), Err(Error::ExpectedMapId { expected, actual })
        if expected == bad_id && actual == id)
    );
    doc.expect.as_mut().unwrap().map_id = id;
    assert!(matches!(
        materialise(&doc, &source),
        Err(Error::ExpectedGeometry { .. })
    ));
}

#[test]
fn versions_and_source_hashes_are_checked_before_materialising() {
    let base = source(fixture::fixture_at_end());
    let donor = source(b"an invented donor cache".to_vec());
    let mut doc = draft(&base);
    doc.builder.bots += 1;
    assert!(matches!(materialise(&doc, &base), Err(Error::Document(_))));
    doc.builder = VERSIONS;

    doc.sources.push(MapRef {
        map: "donor".into(),
        sha256: Sha256([9; 32]),
    });
    assert!(matches!(materialise(&doc, &base), Err(Error::MissingMap(map)) if map == "donor"));
    assert!(matches!(build(&doc, |name| match name {
        "fixture" => Some(base.clone()),
        "donor" => Some(donor.clone()),
        _ => None,
    }), Err(Error::SourceHash { map, .. }) if map == "donor"));
}

#[test]
fn zero_edits_with_a_kit_change_only_the_kit_blocks_and_header() {
    let original = fixture::fixture_at_end();
    let source = source(original.clone());
    let mut doc = draft(&source);
    let published = materialise(&doc, &source).unwrap();
    doc.recipes.push(Recipe::Generate {
        generator: KIT_RECIPE.into(),
        parameters: vec![],
    });
    let built = materialise(&doc, &source).unwrap();
    let again = materialise(&doc, &source).unwrap();
    assert_eq!(built.map_bytes(), again.map_bytes());
    assert_eq!(built.reserves, again.reserves);
    assert_ne!(built.manifest().map_id(), published.manifest().map_id());
    let output = built.map_bytes();
    assert!(output.len() > original.len());

    let mut base_map = Map::from_reader(Cursor::new(original.as_slice())).unwrap();
    let mut map = Map::from_reader(Cursor::new(output)).unwrap();
    let before = Scenario::read(&mut base_map).unwrap();
    let after = Scenario::read(&mut map).unwrap();
    for block in scenario_layout::BLOCKS {
        assert_eq!(
            before.block(block.kind).len(),
            after.block(block.kind).len()
        );
        assert_eq!(
            before.block(block.kind).palette,
            after.block(block.kind).palette
        );
        for i in 0..before.block(block.kind).len() {
            assert_eq!(
                before.block(block.kind).entry(i),
                after.block(block.kind).entry(i)
            );
        }
    }
    for i in 0..before.names.len() {
        assert_eq!(before.names.target(i), after.names.target(i));
    }

    let extras = [0x48, 0x60, 0xa0, 0xb8, 0xc8, 0x158, 0x160, 0x368];
    let mut allowed = vec![8..12, 0x18..0x20, 0x2d0..0x2d4];
    for offset in scenario_layout::BLOCKS
        .iter()
        .map(|b| b.offset as usize)
        .chain(extras)
    {
        let at = fixture::sfile(offset);
        assert_eq!(&original[at..at + 4], &output[at..at + 4]);
        allowed.push(at + 4..at + 8);
    }
    for (index, (a, b)) in original.iter().zip(output.iter()).enumerate() {
        assert!(
            a == b || allowed.iter().any(|r| r.contains(&index)),
            "unexpected write at {index:#x}"
        );
    }

    let weapons = built
        .reserves
        .iter()
        .find(|r| r.block == "weapons")
        .unwrap();
    assert_eq!(weapons.active, 2);
    assert!(weapons.spare > 0);
    let block = after.block(BlockKind::Weapons);
    let def = scenario_layout::block(BlockKind::Weapons);
    let at = map
        .file_offset(
            block.address + weapons.active * def.stride,
            weapons.spare * def.stride,
        )
        .unwrap();
    let spare = map
        .read_file_range(at, u64::from(weapons.spare * def.stride))
        .unwrap();
    assert!(
        spare
            .chunks_exact(def.stride as usize)
            .all(|row| row[..4] == [0xff; 4])
    );
}

#[test]
fn content_resolves_the_local_package_and_checks_its_expectation() {
    let source = source(fixture::fixture_at_end());
    let mut doc = draft(&source);
    doc.ops.push(edit());
    let built = materialise(&doc, &source).unwrap();
    let id = built.manifest().map_id();
    let geometry = built.manifest().result.as_ref().unwrap().geometry.unwrap();
    doc.expect = Some(Expected {
        map_id: id,
        geometry,
    });
    let built = materialise(&doc, &source).unwrap();
    let tmp = Temp::new();
    let dir = built.write_package(&tmp.0).unwrap();
    assert_eq!(built.write_package(&tmp.0).unwrap(), dir);
    let mut library = Library::open(&tmp.0);
    assert!(
        library.choose("fixture").is_none(),
        "must not activate implicitly"
    );
    let package = &library.packages()[0];
    assert_eq!(package.id, id);

    let mut base = source.bytes.as_deref().unwrap();
    let resolved = library.resolve_package(package, &mut base).unwrap();
    assert_eq!(resolved.id, id);
    assert_eq!(resolved.read_all(&mut base).unwrap(), built.map_bytes());

    doc.expect.as_mut().unwrap().map_id = MapId([3; 32]);
    fs::write(dir.join("document.h2forge"), doc.canonical_json().unwrap()).unwrap();
    assert!(matches!(library.resolve_package(package, &mut base),
        Err(ResolveError::ForgeExpectedMapId { expected, actual }) if expected == MapId([3; 32]) && actual == id));

    doc.expect.as_mut().unwrap().map_id = id;
    fs::write(dir.join("document.h2forge"), doc.canonical_json().unwrap()).unwrap();
    let mut stale = b"other map".as_slice();
    assert!(matches!(
        library.resolve_package(package, &mut stale),
        Err(ResolveError::Base { .. })
    ));
    doc.title = "A different document".into();
    fs::write(dir.join("document.h2forge"), doc.canonical_json().unwrap()).unwrap();
    assert!(matches!(
        library.resolve_package(package, &mut base),
        Err(ResolveError::ForgeDocument { .. })
    ));
    built.write_package(&tmp.0).unwrap();
    fs::write(dir.join("materialised.map"), b"not the build").unwrap();
    assert!(matches!(
        library.resolve_package(package, &mut base),
        Err(ResolveError::Hash { .. })
    ));
}

#[test]
fn field_writes_refuse_mismatches_and_unwritable_targets() {
    let mut bytes = fixture::fixture_at_end();
    let (key, path, at) = extend_weapon(&mut bytes, |kind| {
        matches!(kind, Kind::Float { count: 1, .. })
    });
    let source = source(bytes);
    let mut doc = draft(&source);
    let expected = Value::Float(F32::new(0.0).unwrap()).digest().unwrap();
    let write = TagWrite {
        tag: key,
        path,
        value: Value::Float(F32::new(2.0).unwrap()),
        was: Some(expected),
    };
    doc.ops.push(Op {
        id: OpId(1),
        edit: Edit::TagField(write.clone()),
    });
    let built = materialise(&doc, &source).unwrap();
    assert_eq!(&built.map_bytes()[at..at + 4], &2.0_f32.to_le_bytes());

    let Edit::TagField(change) = &mut doc.ops[0].edit else {
        unreachable!()
    };
    change.was = Some(Sha256([6; 32]));
    assert!(matches!(
        materialise(&doc, &source),
        Err(Error::Document(forge_doc::Refusal::Mismatch))
    ));
    let Edit::TagField(change) = &mut doc.ops[0].edit else {
        unreachable!()
    };
    change.path = "does_not_exist".into();
    assert!(materialise(&doc, &source).is_err());
}

#[test]
fn new_tag_strings_and_kit_share_one_final_metadata_growth() {
    let mut bytes = fixture::fixture_at_end();
    let (tag, path, offset) = extend_weapon(&mut bytes, |kind| matches!(kind, Kind::StringId));
    let source = source(bytes);
    let mut doc = draft(&source);
    doc.recipes.push(Recipe::Generate {
        generator: KIT_RECIPE.into(),
        parameters: vec![],
    });
    doc.ops.push(Op {
        id: OpId(1),
        edit: Edit::TagField(TagWrite {
            tag,
            path,
            value: Value::Text("invented string".into()),
            was: None,
        }),
    });
    let output = materialise(&doc, &source).unwrap();
    assert!(!output.reserves.is_empty());
    let map = Map::from_reader(Cursor::new(output.map_bytes())).unwrap();
    let id = u32::from_le_bytes(output.map_bytes()[offset..offset + 4].try_into().unwrap());
    assert_eq!(
        tags::StringId(id).resolve(map.strings()).unwrap(),
        Some("invented string")
    );
}

// Give the fixture's weapon room for its defined root fields. This test
// chooses a named scalar from the repository schema and invents its value.
fn extend_weapon(
    bytes: &mut [u8],
    matches_kind: impl Fn(&Kind) -> bool,
) -> (TagKey, String, usize) {
    let layout = Registry::builtin().group(map::FourCC(*b"weap")).unwrap();
    let field = layout
        .fields
        .iter()
        .find(|f| {
            f.named
                && matches_kind(&f.kind)
                && f.name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .unwrap();
    let tags = fixture::INDEX + 32 + 4 * 12;
    let payload = 0x1100;
    assert!(payload + layout.size < 0x1ff0);
    fixture::put(bytes, tags + 2 * 16 + 12, layout.size);
    fixture::put(bytes, tags + 3 * 16 + 8, fixture::META + 0x1ff0);
    let at = fixture::META_FILE + payload as usize + field.offset as usize;
    (
        TagKey {
            group: "weap".into(),
            path: "objects\\test\\blaster".into(),
        },
        field.name.clone(),
        at,
    )
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("forge-build-{}-{id}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
