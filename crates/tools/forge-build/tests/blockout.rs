//! Block-out pieces materialised on content-kit's block-out invented map
//! (docs/game/forge-blockout.md, "Materialising" and "The cut order").
//! Every definition, map and shader here is invented.
use std::io::Cursor;

use content_kit::blockout::fixture::{ROCK_SHADER, fixture};
use forge_build::{Built, Error, VERSIONS, blockout, build};
use forge_doc::{
    BlockKind, Edit, Endpoint, FORMAT, FieldWrite, Forge, ForgeDoc, MapRef, Op, OpId, Placement,
    Sha256, piece_value,
};
use forge_shapes::{Material, Materials, PieceDef, STARTER, Shape, TagKey};
use map::{FourCC, Map, TagQuery};
use scenario_edit::{MapSource, Scenario};

fn source() -> MapSource {
    let mut source = MapSource::file("invented/yard.map");
    source.bytes = Some(fixture().into());
    source
}

fn doc(source: &MapSource, ops: Vec<Op>) -> ForgeDoc {
    let mut doc = ForgeDoc {
        format: FORMAT,
        builder: VERSIONS,
        base: MapRef {
            map: "yard".into(),
            sha256: Sha256::of(source.bytes.as_deref().unwrap()),
        },
        sources: vec![],
        title: "Block-out".into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops,
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

fn materialise(doc: &ForgeDoc, source: &MapSource) -> Result<Built, Error> {
    build(doc, |name| (name == "yard").then(|| source.clone()))
}

/// A crate of the piece tag (op `id`) and its definition (op `id + 1`).
fn piece(id: u32, def: PieceDef) -> [Op; 2] {
    [
        Op {
            id: OpId(id),
            edit: Edit::Placement(Placement::Add {
                block: BlockKind::Crates,
                like: None,
                fields: vec![FieldWrite {
                    field: "palette index".into(),
                    value: piece_value(),
                }],
            }),
        },
        Op {
            id: OpId(id + 1),
            edit: Edit::Forge(Forge::Piece {
                target: Endpoint::Added(OpId(id)),
                def,
            }),
        },
    ]
}

/// A block of a size the starter set does not have.
fn block() -> PieceDef {
    PieceDef::dev(Shape::Block, [96, 96, 96], 0)
}

fn ramp() -> PieceDef {
    PieceDef::dev(Shape::Ramp, [192, 128, 64], 0)
}

fn painted(path: &str) -> PieceDef {
    PieceDef {
        materials: Materials::All(Material::Map(TagKey::shader(path))),
        ..block()
    }
}

#[test]
fn three_pieces_of_two_definitions_add_two_tag_sets_and_take_their_rows() {
    let source = source();
    let ops = [piece(1, block()), piece(3, ramp()), piece(5, block())].concat();
    let doc = doc(&source, ops);
    let first = materialise(&doc, &source).unwrap();
    let second = materialise(&doc, &source).unwrap();
    assert_eq!(first.map_bytes(), second.map_bytes());
    assert_eq!(first.manifest(), second.manifest());

    let report = first.blockout.as_ref().expect("a document with pieces");
    let mut names = [block().name(), ramp().name()];
    names.sort();
    let rows: Vec<(&str, u32)> = report
        .rows
        .iter()
        .map(|r| (r.name.as_str(), r.row))
        .collect();
    assert_eq!(rows, [(names[0].as_str(), 0), (names[1].as_str(), 1)]);
    assert!(report.rows.iter().all(|r| !r.starter));

    let mut map = Map::from_reader(Cursor::new(first.map_bytes())).unwrap();
    let crates = map
        .tags()
        .iter()
        .filter(|t| t.group == FourCC(*b"bloc"))
        .count();
    assert_eq!(crates, 2);
    let tag_of = |map: &Map<_>, def: &PieceDef| {
        map.tag(TagQuery::NameInGroup(&def.tag_path(), FourCC(*b"bloc")))
            .unwrap()
            .id
    };
    let (block_tag, ramp_tag) = (tag_of(&map, &block()), tag_of(&map, &ramp()));
    let scenario = Scenario::read(&mut map).unwrap();
    let palette = &scenario.block(BlockKind::Crates).palette;
    assert_eq!(palette.len(), 2);
    let row_of = |def: &PieceDef| report.palette_rows()[&def.name()] as usize;
    assert_eq!(palette[row_of(&block())].id, block_tag);
    assert_eq!(palette[row_of(&ramp())].id, ramp_tag);
    let placed: Vec<Option<i64>> = (0..3)
        .map(|i| {
            scenario
                .get(BlockKind::Crates, i, "palette index")
                .unwrap()
                .as_int()
        })
        .collect();
    let (b, r) = (row_of(&block()) as i64, row_of(&ramp()) as i64);
    assert_eq!(placed, [Some(b), Some(r), Some(b)]);
}

#[test]
fn a_map_shader_the_base_lacks_is_refused_by_name() {
    let source = source();
    let absent = r"invented\shaders\absent";
    let doc_absent = doc(&source, piece(1, painted(absent)).to_vec());
    match materialise(&doc_absent, &source) {
        Err(Error::Piece(reason)) => {
            assert!(reason.contains(absent), "{reason}");
            assert!(reason.contains(&painted(absent).name()), "{reason}");
        }
        Err(other) => panic!("refused otherwise: {other}"),
        Ok(_) => panic!("built a piece whose shader the base lacks"),
    }
    let doc_rock = doc(&source, piece(1, painted(ROCK_SHADER)).to_vec());
    let built = materialise(&doc_rock, &source).unwrap();
    assert_eq!(built.blockout.unwrap().rows.len(), 1);
}

#[test]
fn with_headroom_set_low_the_kit_cuts_in_the_pages_order() {
    let base = fixture();
    let needed = [block()];
    let free = blockout::install(&base, &needed, true, u64::MAX)
        .unwrap()
        .report;
    assert!(free.cut.is_empty() && !free.dev_set_cut);
    assert_eq!(free.rows.len(), 1 + STARTER.len());
    assert!(free.dev_set.tag_memory > 0);
    let starters: Vec<String> = free.rows[1..].iter().map(|r| r.name.clone()).collect();

    // Room for the dev set, the block and the first two of the starter
    // set: the rest is cut from the last.
    let keep = 2;
    let room = free.dev_set.tag_memory
        + free.rows[..1 + keep]
            .iter()
            .map(|r| r.cost.tag_memory)
            .sum::<u64>()
        + blockout::palette_bytes(free.base_rows, 1 + keep);
    let tight = blockout::install(&base, &needed, true, room)
        .unwrap()
        .report;
    let cut: Vec<String> = starters[keep..].iter().rev().cloned().collect();
    assert_eq!(tight.cut, cut);
    assert_eq!(tight.rows, free.rows[..1 + keep]);
    assert_eq!(tight.dev_set, free.dev_set);
    assert!(!tight.dev_set_cut);
    assert_eq!(tight.tag_memory, room);

    // Room for nothing: the block is refused with its cost, the dev set
    // its plates need included.
    let alone = blockout::install(&base, &needed, false, u64::MAX)
        .unwrap()
        .report;
    let cost = alone.rows[0].cost;
    match blockout::install(&base, &needed, true, 100) {
        Err(Error::PieceCost(refused)) => {
            assert_eq!(refused.name, block().name());
            assert_eq!(refused.cost, cost);
            assert_eq!(refused.headroom, 100);
            assert!(refused.dev);
            let sentence = Error::PieceCost(refused).to_string();
            assert!(sentence.contains(&block().name()), "{sentence}");
            assert!(
                sentence.contains(&format!("{} B of tag memory", cost.tag_memory)),
                "{sentence}"
            );
        }
        Err(other) => panic!("refused otherwise: {other}"),
        Ok(_) => panic!("fitted into 100 bytes"),
    }
}

#[test]
fn a_barrier_alone_lets_the_kit_cut_its_dev_set() {
    let base = fixture();
    let barrier = PieceDef {
        materials: Materials::All(Material::Clip),
        ..block()
    };
    let needed = [barrier];
    let alone = blockout::install(&base, &needed, false, u64::MAX)
        .unwrap()
        .report;
    let room = alone.tag_memory;
    let report = blockout::install(&base, &needed, true, room)
        .unwrap()
        .report;
    assert!(report.dev_set_cut);
    assert_eq!(report.cut.len(), STARTER.len());
    assert_eq!(report.rows, alone.rows);
}
