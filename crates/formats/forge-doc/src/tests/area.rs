use std::collections::BTreeMap;

use game_markers::{AreaError, Shape};

use super::*;

const FLAGS: BlockKind = BlockKind::NetgameFlags;

fn target(block: BlockKind, index: u32) -> Endpoint {
    Endpoint::Base {
        block,
        index,
        print: Some(Print([0; 8])),
    }
}

fn entry(id: u32, edit: EntryOp) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Entry(edit),
    }
}

fn area_op(id: u32, target: Endpoint, area: Option<Area>) -> Op {
    entry(id, EntryOp::Area { target, area })
}

fn doc(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
    doc.format = doc.required_format();
    doc
}

fn limits(types: &[u16]) -> Limits {
    Limits {
        base_counts: [(FLAGS, types.len() as u32), (BlockKind::Weapons, 1)].into(),
        base_flag_types: types.into(),
        ..Limits::default()
    }
}

#[test]
fn area_ops_round_trip_the_format_3_variant_wire_form() {
    let centre = target(FLAGS, 0);
    let radius = MarkerArea::radius(2.0);
    let path = MarkerArea {
        shape: Shape::Path(vec![[-2., -2.], [2., -2.], [2., 2.]]),
        ..radius.clone()
    };
    let placement = |id, area: &MarkerArea| area_op(id, centre, Some(Area::try_from(area).unwrap()));
    let doc = doc(vec![
        placement(1, &radius),
        placement(2, &path),
        area_op(3, centre, None),
    ]);
    let limits = limits(&[10]);
    let canonical = doc.canonical_json().unwrap();
    assert!(canonical.contains(
        r#""area":{"radius":{"above":"3fc00000","below":"3f000000","radius":"40000000"}}"#
    ));
    assert!(canonical.contains(
        r#""area":{"path":{"above":"3fc00000","below":"3f000000","points":[["c0000000","c0000000"],["40000000","c0000000"],["40000000","40000000"]]}}"#
    ));
    assert!(canonical.contains(r#""area":null"#));
    let missing_area = serde_json::json!({"entry": {"area": {"target": centre}}});
    assert!(serde_json::from_value::<Edit>(missing_area).is_err());
    let back = ForgeDoc::from_bytes(canonical.as_bytes(), &limits).unwrap();
    assert_eq!(back, doc);
    assert_eq!(back.canonical_json().unwrap(), canonical);
    assert_eq!(back.format, FORMAT_ENTRIES);
    assert_eq!(back.tier(), Tier::Variant);
    assert!(back.areas(&limits.base_counts).unwrap().is_empty());
    let resolved = resolve(&back.ops, &limits.base_counts).unwrap();
    assert!(resolved.lowered.is_empty());
    assert_eq!(resolved.prints.len(), 1);
}

#[test]
fn area_validation_refuses_bad_shapes_targets_and_clears() {
    let centre = target(FLAGS, 0);
    let area = Area::try_from(&MarkerArea::default()).unwrap();
    let crossing = Area::Path {
        points: [[0., 0.], [4., 2.], [4., 0.], [0., 1.]]
            .map(|point| point.map(|v| F32::new(v).unwrap()))
            .to_vec(),
        below: F32::new(0.5).unwrap(),
        above: F32::new(1.5).unwrap(),
    };
    let limits = limits(&[10]);
    let refusal = |ops| validate(&doc(ops), &limits).unwrap_err().to_string();
    assert_eq!(
        refusal(vec![area_op(1, centre, Some(crossing))]),
        AreaError::Crosses(0, 2).to_string()
    );
    assert!(refusal(vec![area_op(1, target(BlockKind::Weapons, 0), Some(area.clone()))])
        .contains("an area belongs to a netgame flag"));
    assert!(refusal(vec![
        add(1, BlockKind::Weapons),
        area_op(2, Endpoint::Added(OpId(1)), Some(area.clone())),
    ])
    .contains("an area belongs to a netgame flag"));
    assert!(refusal(vec![area_op(1, centre, None)]).contains("the flag has no area to clear"));
    for remove in [
        entry(2, EntryOp::Remove { target: centre }),
        Op {
            id: OpId(2),
            edit: Edit::Placement(Placement::Remove { block: FLAGS, index: 0 }),
        },
    ] {
        assert!(refusal(vec![area_op(1, centre, Some(area.clone())), remove])
            .contains("clear the flag's area first"));
    }
}

#[test]
fn areas_follow_both_an_own_flag_and_an_added_flag_through_swap_removes() {
    let own = target(FLAGS, 2);
    let added = Endpoint::Added(OpId(1));
    let radius = |r| Area::try_from(&MarkerArea::radius(r)).unwrap();
    let doc = doc(vec![
        add(1, FLAGS),
        area_op(2, own, Some(radius(2.0))),
        area_op(3, added, Some(radius(3.0))),
        entry(4, EntryOp::Remove { target: target(FLAGS, 0) }),
        entry(5, EntryOp::Remove { target: target(FLAGS, 1) }),
    ]);
    let limits = limits(&[10, 0, 10]);
    validate(&doc, &limits).unwrap();
    assert_eq!(
        doc.areas(&limits.base_counts).unwrap(),
        BTreeMap::from([(0, MarkerArea::radius(3.0)), (1, MarkerArea::radius(2.0))])
    );
    let rows = MapRows {
        counts: limits.base_counts.clone(),
        flags: [10, 0, 10].map(|kind| FlagRow { kind, team: 0 }).to_vec(),
        ..MapRows::default()
    };
    let things = labelled(&rows, Some(&doc)).unwrap();
    assert_eq!(things.len(), 2);
    assert_eq!(things[0].label.text, "ctf_flag_spawn");
    assert_eq!(things[0].area, Some(MarkerArea::radius(3.0)));
    assert_eq!(things[1].label.text, "territory");
    assert_eq!(things[1].area, Some(MarkerArea::radius(2.0)));
}

#[test]
fn an_area_cannot_share_its_hill_with_corners() {
    let centre = target(FLAGS, 0);
    let mut doc = doc(vec![area_op(1, centre, Some(Area::try_from(&MarkerArea::default()).unwrap()))]);
    let same_hill = limits(&[11, 11, 11]);
    let different_hill = limits(&[11, 11, 12]);
    assert_eq!(
        validate(&doc, &same_hill).unwrap_err().to_string(),
        "a hill with corners takes no area"
    );
    validate(&doc, &different_hill).unwrap();
    doc.ops.push(entry(2, EntryOp::Set {
        target: target(FLAGS, 2),
        write: FieldWrite { field: "type".into(), value: Value::Int(11) },
    }));
    assert!(matches!(validate(&doc, &different_hill), Err(Refusal::Area(_))));
    // A document label overrides the type's built-in label.
    doc.ops.push(Op {
        id: OpId(3),
        edit: Edit::Forge(Forge::Label {
            target: Endpoint::base(FLAGS, 2),
            label: Some(Label::new("hill_corner", Some(1)).unwrap()),
        }),
    });
    validate(&doc, &same_hill).unwrap();
    doc.ops.push(entry(4, EntryOp::Reset { target: centre }));
    assert!(doc.areas(&same_hill.base_counts).unwrap().is_empty());
}
