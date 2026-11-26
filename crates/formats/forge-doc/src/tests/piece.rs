//! Block-out pieces in the document (docs/game/forge-blockout.md, "In the
//! document"). Every definition here is invented.
use std::collections::BTreeMap;

use forge_shapes::{Material, Materials, PieceDef, Shape, TagKey as ShapeKey};

use super::*;

const CRATES: BlockKind = BlockKind::Crates;
const WEAPONS: BlockKind = BlockKind::Weapons;

/// Format 6: a crate with the piece tag, and its definition, a 64 unit block
/// drawn with the dev textures.
const PIECED: &str = r#"{"author":"Editor","base":{"map":"fixture","sha256":"1111111111111111111111111111111111111111111111111111111111111111"},"builder":{"bots":3,"forge_build":1,"pathfind":2},"description":"row 1\nrow 2","format":6,"imports":[],"ops":[{"edit":{"placement":{"add":{"block":"crates","fields":[{"field":"palette index","value":{"tag":{"group":"bloc","path":"forge\\blockout\\piece"}}}]}}},"id":1},{"edit":{"forge":{"piece":{"def":{"materials":{"all":"dev"},"param":0,"shape":"block","size":[64,64,64]},"target":{"added":1}}}},"id":2}],"recipes":[],"sources":[],"title":"Forge \"é\""}"#;

fn block_def() -> PieceDef {
    PieceDef::dev(Shape::Block, [64, 64, 64], 0)
}

fn ramp_def() -> PieceDef {
    PieceDef::dev(Shape::Ramp, [128, 128, 64], 0)
}

fn added(id: u32) -> Endpoint {
    Endpoint::Added(OpId(id))
}

fn add_piece(id: u32) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block: CRATES,
            like: None,
            fields: vec![FieldWrite {
                field: "palette index".into(),
                value: piece_value(),
            }],
        }),
    }
}

fn add_like(id: u32, like: u32) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block: CRATES,
            like: Some(like),
            fields: vec![],
        }),
    }
}

fn define(id: u32, target: Endpoint, def: PieceDef) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Piece { target, def }),
    }
}

fn remove(id: u32, index: u32) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Remove {
            block: CRATES,
            index,
        }),
    }
}

fn palette(id: u32, index: u32, value: Value) -> Op {
    set(id, CRATES, index, "palette index", value)
}

fn other_tag() -> Value {
    Value::Tag(Some(key("bloc")))
}

fn counts(crates: u32) -> Limits {
    Limits {
        base_counts: BTreeMap::from([(CRATES, crates)]),
        ..Limits::default()
    }
}

/// A document whose `format` is the one its ops need.
fn with(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
    doc.format = doc.required_format();
    doc
}

fn refuse(ops: Vec<Op>, limits: &Limits, expected: &str) {
    let error = validate(&with(ops), limits).unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "{error} did not mention {expected}"
    );
}

fn shaped(def: PieceDef) -> Vec<Op> {
    vec![add_piece(1), define(2, added(1), def)]
}

fn stamp_from(mut at: LogHead, ops: &[Op]) -> Vec<Entry> {
    let mut entries = Vec::new();
    for op in ops {
        let entry = at
            .stamp(
                at.tick + 1,
                Proposal {
                    author: "Editor".into(),
                    base_seq: at.seq,
                    op: op.clone(),
                    was: None,
                },
                None,
                &Limits::default(),
            )
            .unwrap();
        at = LogHead {
            seq: entry.seq,
            tick: entry.tick,
            hash: entry.hash().unwrap(),
        };
        entries.push(entry);
    }
    entries
}

fn head_of(doc: &ForgeDoc, entries: &[Entry]) -> LogHead {
    entries.last().map_or_else(
        || LogHead::genesis(doc).unwrap(),
        |e| LogHead {
            seq: e.seq,
            tick: e.tick,
            hash: e.hash().unwrap(),
        },
    )
}

#[test]
fn a_piece_op_makes_the_document_format_6_and_others_keep_their_number() {
    let doc = with(shaped(block_def()));
    assert_eq!(doc.format, FORMAT_PIECES);
    assert_eq!(FORMAT_PIECES, 6);
    validate(&doc, &Limits::default()).unwrap();
    validate(&doc, &counts(0)).unwrap();

    // A crate with no piece op is a document of the number it had.
    assert_eq!(with(vec![add(1, CRATES)]).format, FORMAT);
    let mut plain = with(vec![add(1, CRATES)]);
    plain.format = FORMAT_PIECES;
    assert_eq!(
        validate(&plain, &Limits::default()),
        Err(Refusal::Format(FORMAT_PIECES))
    );
    // A document with a piece op in it is no lower number.
    let mut low = doc.clone();
    low.format = FORMAT_GROUPS;
    assert_eq!(
        validate(&low, &Limits::default()),
        Err(Refusal::Format(FORMAT_GROUPS))
    );
    // A print on the target does not lower the number either.
    let printed = Endpoint::Base {
        block: CRATES,
        index: 0,
        print: Some(Print([1; 8])),
    };
    let doc = with(vec![
        palette(1, 0, piece_value()),
        define(2, printed, block_def()),
    ]);
    assert_eq!(doc.format, FORMAT_PIECES);
    validate(&doc, &counts(1)).unwrap();
    let resolved = resolve(&doc.ops, &counts(1).base_counts).unwrap();
    assert_eq!(resolved.prints.len(), 1);
}

#[test]
fn a_piece_document_has_golden_canonical_json_and_a_stable_identity() {
    let limits = Limits::default();
    let doc = with(shaped(block_def()));
    assert_eq!(doc.canonical_json().unwrap(), PIECED);
    let read = ForgeDoc::from_bytes(PIECED.as_bytes(), &limits).unwrap();
    assert_eq!(read, doc);
    assert_eq!(read.id().unwrap(), doc.id().unwrap());
    assert_eq!(read.canonical_json().unwrap(), PIECED);

    // The identity covers the definition and its order in the log.
    let ramp = with(shaped(ramp_def()));
    assert_ne!(ramp.id().unwrap(), doc.id().unwrap());
    let mut sized = block_def();
    sized.size[2] = 128;
    assert_ne!(with(shaped(sized)).id().unwrap(), doc.id().unwrap());
    assert_ne!(
        with(vec![add(1, CRATES)]).id().unwrap(),
        doc.id().unwrap()
    );
    // A document without a piece op is the bytes it was.
    assert_eq!(draft().canonical_json().unwrap(), EMPTY);
}

#[test]
fn a_definition_with_every_material_kind_round_trips() {
    let def = PieceDef {
        shape: Shape::Stairs,
        size: [128, 64, 64],
        param: 4,
        materials: Materials::Each([
            Material::Map(ShapeKey::shader("levels\\test\\floor")),
            Material::Dev,
            Material::Nodraw,
            Material::Clip,
        ]),
    };
    let doc = with(shaped(def.clone()));
    let limits = counts(0);
    validate(&doc, &limits).unwrap();
    let json = doc.canonical_json().unwrap();
    let read = ForgeDoc::from_bytes(json.as_bytes(), &limits).unwrap();
    assert_eq!(read, doc);
    assert_eq!(read.id().unwrap(), doc.id().unwrap());
    let pieces = read.pieces(&limits.base_counts).unwrap();
    assert_eq!(pieces.active.len(), 1);
    assert_eq!(pieces.active[0].def, def);
}

#[test]
fn each_refusal_has_a_refusing_case() {
    let none = Limits::default();
    let stairs = |param| PieceDef::dev(Shape::Stairs, [128, 64, 64], param);
    let mapped = |group: &str, path: &str| PieceDef {
        materials: Materials::All(Material::Map(ShapeKey {
            group: group.into(),
            path: path.into(),
        })),
        ..block_def()
    };
    let cases: Vec<(Vec<Op>, Limits, &str)> = vec![
        // The tag with no definition.
        (vec![add_piece(1)], none.clone(), "has no definition"),
        (
            vec![palette(1, 1, piece_value())],
            counts(2),
            "has no definition",
        ),
        // A definition on any other placement.
        (
            vec![add(1, CRATES), define(2, added(1), block_def())],
            none.clone(),
            "belongs to a placement of the piece tag",
        ),
        (
            vec![
                palette(1, 0, other_tag()),
                define(2, Endpoint::base(CRATES, 0), block_def()),
            ],
            counts(1),
            "belongs to a placement of the piece tag",
        ),
        (
            vec![
                add_piece(1),
                define(2, added(1), block_def()),
                palette(3, 0, other_tag()),
            ],
            counts(0),
            "remove the piece",
        ),
        // A target that is not there, is removed, or is not a crate.
        (
            vec![define(1, added(9), block_def())],
            none.clone(),
            "names no entry",
        ),
        (
            vec![
                add_piece(1),
                define(2, added(1), block_def()),
                remove(3, 0),
                define(4, added(1), block_def()),
            ],
            counts(0),
            "names no entry",
        ),
        (
            vec![define(1, Endpoint::base(CRATES, 5), block_def())],
            counts(2),
            "names no entry",
        ),
        (
            vec![define(1, Endpoint::base(WEAPONS, 0), block_def())],
            none.clone(),
            "a piece is a crate",
        ),
        (
            vec![define(1, Endpoint::Added(OpId(0)), block_def())],
            none.clone(),
            "want the ID of an add op",
        ),
        (
            vec![define(1, Endpoint::base(CRATES, 40_000), block_def())],
            none.clone(),
            "exceeds block limit",
        ),
        // A size outside the range.
        (
            shaped(PieceDef::dev(Shape::Block, [0, 64, 64], 0)),
            none.clone(),
            "size x of 0 units",
        ),
        (
            shaped(PieceDef::dev(Shape::Block, [64, 2049, 64], 0)),
            none.clone(),
            "size y of 2049 units",
        ),
        (
            shaped(PieceDef::dev(Shape::Wall, [64, 16, 3], 0)),
            none.clone(),
            "size z of 3 units",
        ),
        // A param outside its shape's range.
        (shaped(stairs(1)), none.clone(), "2 to 32 steps"),
        (shaped(stairs(33)), none.clone(), "2 to 32 steps"),
        (
            shaped(PieceDef::dev(Shape::Block, [64, 64, 64], 3)),
            none.clone(),
            "takes a param of 0",
        ),
        (
            shaped(PieceDef::dev(Shape::Cylinder, [64, 64, 64], 10)),
            none.clone(),
            "8, 12, 16 or 24 sides",
        ),
        // A Map material that is not a shad key.
        (
            shaped(mapped("bitm", "levels\\test\\floor")),
            none.clone(),
            "must name a shad tag",
        ),
        (shaped(mapped("shad", "")), none.clone(), "no tag path"),
        (
            shaped(mapped("shad", "levels/test/floor")),
            none.clone(),
            "relative tag key",
        ),
        (
            shaped(mapped("shad", "levels\\..\\floor")),
            none.clone(),
            "relative tag key",
        ),
    ];
    for (ops, limits, expected) in cases {
        refuse(ops, &limits, expected);
    }

    // A bad definition is also refused where a proposal is stamped.
    let doc = draft();
    let error = LogHead::genesis(&doc)
        .unwrap()
        .stamp(
            1,
            Proposal {
                author: "Editor".into(),
                base_seq: 0,
                op: define(1, added(1), PieceDef::dev(Shape::Block, [0, 4, 4], 0)),
                was: None,
            },
            None,
            &Limits::default(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("size x of 0 units"), "{error}");
}

#[test]
fn a_piece_tag_written_by_a_set_is_a_piece() {
    let limits = counts(2);
    let doc = with(vec![
        palette(1, 1, piece_value()),
        define(2, Endpoint::base(CRATES, 1), block_def()),
    ]);
    validate(&doc, &limits).unwrap();
    let pieces = doc.pieces(&limits.base_counts).unwrap();
    assert_eq!(pieces.active.len(), 1);
    assert_eq!((pieces.active[0].index, pieces.active[0].id), (1, OpId(2)));
    // Writing another tag back is refused while the piece has a definition,
    // and allowed once the placement has none.
    refuse(
        vec![
            palette(1, 1, piece_value()),
            define(2, Endpoint::base(CRATES, 1), block_def()),
            palette(3, 1, other_tag()),
        ],
        &limits,
        "remove the piece",
    );
    validate(
        &with(vec![
            palette(1, 1, piece_value()),
            palette(2, 1, other_tag()),
        ]),
        &limits,
    )
    .unwrap();
}

#[test]
fn a_changed_definition_replaces_the_one_before() {
    let limits = counts(0);
    let doc = with(vec![
        add_piece(1),
        define(2, added(1), block_def()),
        define(3, added(1), ramp_def()),
    ]);
    validate(&doc, &limits).unwrap();
    let pieces = doc.pieces(&limits.base_counts).unwrap();
    assert_eq!(pieces.active.len(), 1);
    assert_eq!(pieces.active[0].def, ramp_def());
    assert_eq!(pieces.active[0].id, OpId(3));
    let targets = doc.piece_targets(&limits.base_counts).unwrap();
    assert_eq!(targets, BTreeMap::from([(added(1), ramp_def())]));
}

#[test]
fn a_removal_clears_the_definition_with_the_entry() {
    let limits = counts(0);
    let doc = with(vec![
        add_piece(1),
        define(2, added(1), block_def()),
        remove(3, 0),
    ]);
    validate(&doc, &limits).unwrap();
    assert!(doc.pieces(&limits.base_counts).unwrap().active.is_empty());
    assert!(doc.piece_targets(&limits.base_counts).unwrap().is_empty());
    // The piece op is still in the document.
    assert_eq!(doc.format, FORMAT_PIECES);
}

#[test]
fn a_definition_follows_its_entry_through_a_swap_remove() {
    // Base crates 0..=2; the piece is index 3. Crate 0 is removed, so the
    // piece, the last entry, takes index 0.
    let limits = counts(3);
    let doc = with(vec![
        add_piece(1),
        define(2, added(1), ramp_def()),
        remove(3, 0),
    ]);
    validate(&doc, &limits).unwrap();
    let pieces = doc.pieces(&limits.base_counts).unwrap();
    assert_eq!(pieces.active.len(), 1);
    assert_eq!((pieces.active[0].index, &pieces.active[0].def), (0, &ramp_def()));
}

#[test]
fn a_duplicate_takes_its_sources_definition() {
    // The piece is index 3 of four; the copy is made `like` it.
    let limits = counts(3);
    let doc = with(vec![
        add_piece(1),
        define(2, added(1), ramp_def()),
        add_like(3, 3),
    ]);
    validate(&doc, &limits).unwrap();
    let pieces = doc.pieces(&limits.base_counts).unwrap();
    let seen: Vec<_> = pieces
        .active
        .iter()
        .map(|p| (p.index, p.id, p.def.clone()))
        .collect();
    assert_eq!(
        seen,
        vec![(3, OpId(2), ramp_def()), (4, OpId(3), ramp_def())]
    );
    // The copy can be given a definition of its own.
    let mut ops = doc.ops.clone();
    ops.push(define(4, added(3), block_def()));
    let doc = with(ops);
    validate(&doc, &limits).unwrap();
    let pieces = doc.pieces(&limits.base_counts).unwrap();
    assert_eq!(pieces.active[0].def, ramp_def());
    assert_eq!(pieces.active[1].def, block_def());
    // A copy of a crate that is no piece is no piece.
    let plain = with(vec![add_like(1, 0)]);
    validate(&plain, &limits).unwrap();
    assert!(plain.pieces(&limits.base_counts).unwrap().active.is_empty());
    assert_eq!(plain.format, FORMAT);
}

#[test]
fn pieces_need_the_crate_count_to_resolve_and_none_when_there_are_none() {
    let doc = with(shaped(block_def()));
    let error = doc.pieces(&BTreeMap::new()).unwrap_err();
    assert!(error.to_string().contains("crates count is needed"), "{error}");
    let plain = with(vec![add(1, CRATES)]);
    assert!(plain.pieces(&BTreeMap::new()).unwrap().active.is_empty());
    assert!(plain.piece_targets(&BTreeMap::new()).unwrap().is_empty());
}

#[test]
fn pieces_go_through_the_edit_log_and_undo() {
    let doc = draft();
    let ops = [
        add_piece(1),
        define(2, added(1), block_def()),
        define(3, added(1), ramp_def()),
    ];
    let start = LogHead::genesis(&doc).unwrap();
    let entries = stamp_from(start, &ops);
    let folded = fold(&doc, &entries).unwrap();
    assert_eq!(folded.format, FORMAT_PIECES);
    assert_eq!(folded.ops, ops.to_vec());
    folded.canonical_json().unwrap();

    // The inverses, latest first, give the start and its format.
    let undos: Vec<Op> = ops
        .iter()
        .rev()
        .map(|op| inverse(op, &Before::capture(&doc)).unwrap())
        .collect();
    assert!(undos.iter().all(|op| op.edit == Edit::Undo));
    let mut log = entries.clone();
    log.extend(stamp_from(head_of(&doc, &entries), &undos));
    let restored = fold(&doc, &log).unwrap();
    assert_eq!(restored, doc);
    assert_eq!(restored.format, FORMAT);
    assert_eq!(restored.id().unwrap(), doc.id().unwrap());

    // Redo of the last op restores the same op.
    let redo = inverse(&undos[0], &Before::capture(&folded)).unwrap();
    assert_eq!(redo, ops[2]);

    // A log whose active ops leave a tagged crate with no definition does
    // not fold.
    let partial = stamp_from(start, &ops[..1]);
    let error = fold(&doc, &partial).unwrap_err();
    assert!(error.to_string().contains("has no definition"), "{error}");
    // Retracting the second definition leaves the first.
    let mut log = entries.clone();
    log.extend(stamp_from(head_of(&doc, &entries), &undos[..1]));
    let one = fold(&doc, &log).unwrap();
    assert_eq!(one.ops, ops[..2].to_vec());
}

/// An adapter with no `pieces`: it refuses a document that has one.
struct Plain;

impl Apply for Plain {
    fn placement(&mut self, _: &Placement) -> Result<()> {
        Ok(())
    }

    fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Takes(Vec<Pieces>);

impl Apply for Takes {
    fn placement(&mut self, _: &Placement) -> Result<()> {
        Ok(())
    }

    fn tag_field(&mut self, _: &TagWrite) -> Result<()> {
        Ok(())
    }

    fn pieces(&mut self, pieces: &Pieces) -> Result<()> {
        self.0.push(pieces.clone());
        Ok(())
    }
}

#[test]
fn replay_hands_the_pieces_over_and_an_adapter_without_them_refuses() {
    let limits = counts(1);
    let doc = with(shaped(block_def()));
    assert_eq!(
        replay(&doc, &limits, &mut Plain),
        Err(Refusal::Unsupported("piece".into()))
    );
    let mut takes = Takes::default();
    replay(&doc, &limits, &mut takes).unwrap();
    assert_eq!(takes.0.len(), 1);
    assert_eq!(takes.0[0].active.len(), 1);
    assert_eq!(takes.0[0].active[0].index, 1);
    assert_eq!(takes.0[0].active[0].def, block_def());

    // A document with no piece never calls it.
    let mut takes = Takes::default();
    replay(&with(vec![add(1, CRATES)]), &limits, &mut takes).unwrap();
    assert!(takes.0.is_empty());
    replay(&with(vec![add(1, CRATES)]), &limits, &mut Plain).unwrap();
}

#[test]
fn a_document_with_a_piece_is_a_map_and_not_a_variant() {
    let doc = with(shaped(block_def()));
    assert_eq!(doc.tier(), Tier::Map);
    assert_eq!(doc.check_variant(), Err(Refusal::Tier));
    assert_eq!(with(vec![add(1, CRATES)]).tier(), Tier::Variant);
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn seeded_fuzz_of_piece_documents() {
    let limits = counts(3);
    let mut rng = Rng(0x5eed_b0c5_0005_0001);
    let defs = [
        block_def(),
        ramp_def(),
        PieceDef::dev(Shape::Stairs, [128, 64, 64], 4),
        PieceDef::dev(Shape::Block, [0, 64, 64], 0),
        PieceDef::dev(Shape::Stairs, [128, 64, 64], 1),
    ];
    let (mut accepted, mut piece_docs) = (0, 0);
    for case in 0..4_000 {
        let mut doc = draft();
        let mut id = 1;
        for _ in 0..rng.below(6) + 1 {
            let target = match rng.below(3) {
                0 => Endpoint::base(CRATES, rng.below(4) as u32),
                _ => added(rng.below(id as usize + 1) as u32),
            };
            match rng.below(7) {
                0 | 1 => {
                    // A piece: the placement and its definition.
                    doc.ops.push(add_piece(id));
                    doc.ops
                        .push(define(id + 1, added(id), defs[rng.below(defs.len())].clone()));
                    id += 1;
                }
                2 => doc.ops.push(add(id, CRATES)),
                3 => doc.ops.push(add_like(id, rng.below(5) as u32)),
                4 => doc.ops.push(remove(id, rng.below(5) as u32)),
                5 => {
                    let value = if rng.below(2) == 0 {
                        piece_value()
                    } else {
                        other_tag()
                    };
                    doc.ops.push(palette(id, rng.below(4) as u32, value));
                }
                _ => doc
                    .ops
                    .push(define(id, target, defs[rng.below(defs.len())].clone())),
            }
            id += 1;
        }
        doc.format = match rng.below(4) {
            0 => rng.below(7) as u32,
            _ => doc.required_format(),
        };
        let bytes = serde_json::to_vec(&doc).unwrap();
        let Ok(doc) = ForgeDoc::from_bytes(&bytes, &limits) else {
            continue;
        };
        accepted += 1;
        assert_eq!(doc.format, doc.required_format(), "seeded case {case}");
        let has_piece = doc
            .ops
            .iter()
            .any(|op| matches!(op.edit, Edit::Forge(Forge::Piece { .. })));
        assert_eq!(has_piece, doc.format == FORMAT_PIECES, "seeded case {case}");
        if has_piece {
            piece_docs += 1;
        }
        let json = doc.canonical_json().unwrap();
        let again = ForgeDoc::from_bytes(json.as_bytes(), &limits).unwrap();
        assert_eq!(again, doc, "seeded case {case}");
        assert_eq!(again.id().unwrap(), doc.id().unwrap(), "seeded case {case}");
        let pieces = doc.pieces(&limits.base_counts).unwrap();
        let targets = doc.piece_targets(&limits.base_counts).unwrap();
        assert_eq!(pieces.active.len(), targets.len(), "seeded case {case}");
        for piece in &pieces.active {
            piece.def.validate().unwrap();
        }
    }
    assert!((1..4_000).contains(&accepted), "{accepted}");
    assert!(piece_docs > 0);
}

#[test]
fn a_document_is_no_stricter_without_the_crate_count() {
    // Each of these is valid with the count, and the rules that need to
    // know which crates hold the tag wait for it without one: the file form
    // and the identity are checked with no map.
    let ops = [
        vec![
            palette(1, 1, piece_value()),
            define(2, Endpoint::base(CRATES, 1), block_def()),
        ],
        vec![add_piece(1), remove(2, 3)],
        vec![
            add_piece(1),
            define(2, added(1), block_def()),
            add_like(3, 3),
        ],
    ];
    for ops in ops {
        let doc = with(ops);
        validate(&doc, &counts(3)).unwrap();
        doc.canonical_json().unwrap();
        doc.id().unwrap();
        validate(&doc, &Limits::default()).unwrap();
    }
}
