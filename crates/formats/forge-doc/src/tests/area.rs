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

#[test]
fn box_and_sphere_areas_round_trip_the_variant_wire_form() {
    let centre = target(FLAGS, 0);
    let boxed = MarkerArea::boxed([3.0, 1.0]);
    let ball = MarkerArea::sphere(2.0);
    let place = |id, area: &MarkerArea| area_op(id, centre, Some(Area::try_from(area).unwrap()));
    let doc = doc(vec![place(1, &boxed), place(2, &ball)]);
    let limits = limits(&[10]);
    let canonical = doc.canonical_json().unwrap();
    assert!(canonical.contains(
        r#""area":{"box":{"above":"3fc00000","below":"3f000000","half":["40400000","3f800000"]}}"#
    ));
    assert!(canonical.contains(r#""area":{"sphere":{"radius":"40000000"}}"#));
    let back = ForgeDoc::from_bytes(canonical.as_bytes(), &limits).unwrap();
    assert_eq!(back, doc);
    assert_eq!(back.canonical_json().unwrap(), canonical);
    // The document decodes to the area the marker and the zone use.
    assert_eq!(Area::try_from(&boxed).unwrap().marker_area(), boxed);
    assert_eq!(Area::try_from(&ball).unwrap().marker_area(), ball);
    // A sphere written with heights is not a spelling.
    let heights = serde_json::json!({"sphere": {"radius": "40000000", "below": "0", "above": "0"}});
    assert!(serde_json::from_value::<Area>(heights).is_err());
}

#[test]
fn box_and_sphere_areas_are_refused_by_the_markers_sentence() {
    let centre = target(FLAGS, 0);
    let limits = limits(&[10]);
    let tiny_box = Area::Box {
        half: [0.1, 1.0].map(|v| F32::new(v).unwrap()),
        below: F32::new(0.5).unwrap(),
        above: F32::new(1.5).unwrap(),
    };
    let huge_ball = Area::Sphere {
        radius: F32::new(100.0).unwrap(),
    };
    for (area, why) in [
        (tiny_box, AreaError::Half([0.1, 1.0])),
        (huge_ball, AreaError::Radius(100.0)),
    ] {
        let refused = validate(&doc(vec![area_op(1, centre, Some(area))]), &limits).unwrap_err();
        assert_eq!(refused.to_string(), why.to_string());
    }
}

fn show_op(id: u32, target: Endpoint, on: bool) -> Op {
    entry(id, EntryOp::Show { target, on })
}

#[test]
fn a_show_op_round_trips_the_format_3_variant_wire_form() {
    let centre = target(FLAGS, 0);
    let doc = doc(vec![show_op(1, centre, true), show_op(2, centre, false)]);
    let limits = limits(&[10]);
    let canonical = doc.canonical_json().unwrap();
    assert!(canonical.contains(r#""show":{"on":true,"target":"#), "{canonical}");
    assert!(canonical.contains(r#""show":{"on":false,"target":"#), "{canonical}");
    // The op needs both its fields.
    let no_on = serde_json::json!({"entry": {"show": {"target": centre}}});
    assert!(serde_json::from_value::<Edit>(no_on).is_err());
    let back = ForgeDoc::from_bytes(canonical.as_bytes(), &limits).unwrap();
    assert_eq!(back, doc);
    assert_eq!(back.canonical_json().unwrap(), canonical);
    // A show op is an entry op: format 3, the variant tier, no scenario bytes.
    assert_eq!(back.format, FORMAT_ENTRIES);
    assert_eq!(back.tier(), Tier::Variant);
    let resolved = resolve(&back.ops, &limits.base_counts).unwrap();
    assert!(resolved.lowered.is_empty());
    assert_eq!(resolved.prints.len(), 1);
}

#[test]
fn a_show_op_belongs_to_a_netgame_flag() {
    let limits = limits(&[10]);
    let refusal = |ops| validate(&doc(ops), &limits).unwrap_err().to_string();
    assert!(refusal(vec![show_op(1, target(BlockKind::Weapons, 0), true)])
        .contains("shown in play belongs to a netgame flag"));
    assert!(refusal(vec![
        add(1, BlockKind::Weapons),
        show_op(2, Endpoint::Added(OpId(1)), true),
    ])
    .contains("shown in play belongs to a netgame flag"));
}

#[test]
fn a_flag_the_document_added_is_shown_and_a_base_flag_is_not_until_an_op_flips_each() {
    let limits = limits(&[10, 10, 10]);
    let counts = &limits.base_counts;
    // Nothing added, nothing said: a retail map shows nothing.
    assert!(doc(vec![]).shown_flags(counts).unwrap().is_empty());
    assert!(doc(vec![]).show_targets(counts).unwrap().is_empty());
    // An added flag (index 3) is shown; the base flags are not.
    let mut ops = vec![add(1, FLAGS)];
    assert_eq!(doc(ops.clone()).shown_flags(counts).unwrap(), [3].into());
    // The op shows base flag 1 and hides the added flag.
    ops.push(show_op(2, target(FLAGS, 1), true));
    assert_eq!(doc(ops.clone()).shown_flags(counts).unwrap(), [1, 3].into());
    ops.push(show_op(3, Endpoint::Added(OpId(1)), false));
    let flipped = doc(ops.clone());
    validate(&flipped, &limits).unwrap();
    assert_eq!(flipped.shown_flags(counts).unwrap(), [1].into());
    assert_eq!(
        flipped.show_targets(counts).unwrap(),
        BTreeMap::from([(target(FLAGS, 1).identity(), true), (Endpoint::Added(OpId(1)), false)])
    );
    // The last op wins, and a base flag turned off again is as it was.
    ops.push(show_op(4, target(FLAGS, 1), false));
    ops.push(show_op(5, Endpoint::Added(OpId(1)), true));
    assert_eq!(doc(ops.clone()).shown_flags(counts).unwrap(), [3].into());
    // A reset puts a base flag back as the map has it.
    ops.push(show_op(6, target(FLAGS, 2), true));
    assert_eq!(doc(ops.clone()).shown_flags(counts).unwrap(), [2, 3].into());
    ops.push(entry(7, EntryOp::Reset { target: target(FLAGS, 2) }));
    assert_eq!(doc(ops).shown_flags(counts).unwrap(), [3].into());
}

#[test]
fn a_shown_flag_follows_its_index_through_swap_removes_and_a_removed_one_is_not_shown() {
    let limits = limits(&[10, 10, 10]);
    let counts = &limits.base_counts;
    // Removing flag 0 moves the last flag into its place: the show on flag 2
    // follows it to index 0, and the one on the removed flag goes.
    let doc = doc(vec![
        show_op(1, target(FLAGS, 0), true),
        show_op(2, target(FLAGS, 2), true),
        entry(3, EntryOp::Remove { target: target(FLAGS, 0) }),
    ]);
    validate(&doc, &limits).unwrap();
    assert_eq!(doc.shown_flags(counts).unwrap(), [0].into());
}

#[test]
fn the_census_marks_the_flags_the_document_shows() {
    let limits = limits(&[10, 11]);
    let rows = MapRows {
        counts: limits.base_counts.clone(),
        flags: [10, 11].map(|kind| FlagRow { kind, team: 0 }).to_vec(),
        ..MapRows::default()
    };
    // A retail map with no document shows nothing.
    assert!(labelled(&rows, None).unwrap().iter().all(|t| !t.show));
    let doc = doc(vec![add(1, FLAGS), show_op(2, target(FLAGS, 1), true)]);
    let things = labelled(&rows, Some(&doc)).unwrap();
    assert_eq!(things.len(), 3);
    assert_eq!(
        things.iter().map(|t| (t.index, t.show)).collect::<Vec<_>>(),
        [(0, false), (1, true), (2, true)]
    );
}
