use std::collections::BTreeMap;

use super::*;

fn tag(path: &str) -> TagKey {
    TagKey {
        group: "bloc".into(),
        path: path.into(),
    }
}

fn piece(x: f32, yaw: f32) -> Captured {
    Captured {
        block: BlockKind::Crates,
        tag: tag(r"fixture\cube"),
        pose: ([x, 1., 2.], [yaw, 0.1, 0.2]),
        label: None,
    }
}

fn near(a: [f32; 3], b: [f32; 3]) {
    for i in 0..3 {
        assert!((a[i] - b[i]).abs() < 1e-4, "{a:?} is not {b:?}");
    }
}

#[test]
fn a_captured_prefab_places_back_where_it_was_taken_from() {
    let from = [piece(3., 0.5), piece(5., 1.), piece(4., 0.)];
    let prefab = Prefab::capture("wall", true, &from).unwrap();
    let pivot = from[0].pose;
    for (piece, pose) in prefab.poses(pivot).iter().zip(&from) {
        near(piece.0, pose.pose.0);
        let turn = (piece.1[0] - pose.pose.1[0]).rem_euclid(std::f32::consts::TAU);
        assert!(turn < 1e-4 || std::f32::consts::TAU - turn < 1e-4, "{turn}");
        assert_eq!(piece.1[1..], pose.pose.1[1..]);
    }
}

#[test]
fn placing_turns_the_offsets_by_the_yaw_it_is_put_down_at() {
    let prefab = Prefab::capture("wall", true, &[piece(0., 0.), piece(2., 0.)]).unwrap();
    let quarter = std::f32::consts::FRAC_PI_2;
    let poses = prefab.poses(([10., 0., 0.], [quarter, 0., 0.]));
    near(poses[0].0, [10., 0., 0.]);
    near(poses[1].0, [10., 2., 0.]);
    assert!((poses[1].1[0] - quarter).abs() < 1e-5);
}

#[test]
fn a_prefab_file_is_canonical_bounded_and_read_back_whole() {
    let mut from = piece(1., 0.);
    from.label = Some(Label::new("checkpoint", Some(2)).unwrap());
    let prefab = Prefab::capture("post", false, &[from]).unwrap();
    let json = prefab.canonical_json().unwrap();
    assert!(json.starts_with(r#"{"format":1,"name":"post","pieces":[{"block":"crates","#));
    assert_eq!(Prefab::from_bytes(json.as_bytes()).unwrap(), prefab);
    assert_eq!(prefab.canonical_json().unwrap(), json, "stable bytes");
    // Over the bound, before it is parsed.
    let big = vec![b' '; MAX_PREFAB_BYTES + 1];
    assert!(matches!(
        Prefab::from_bytes(&big),
        Err(Refusal::Limit { .. })
    ));
    // An unknown field and an unknown block are refused.
    let extra = json.replacen(r#""name""#, r#""extra":1,"name""#, 1);
    assert!(Prefab::from_bytes(extra.as_bytes()).is_err());
    let block = json.replacen("crates", "nowhere", 1);
    assert!(Prefab::from_bytes(block.as_bytes()).is_err());
}

#[test]
fn a_prefab_that_no_build_could_write_is_refused() {
    let one = piece(0., 0.);
    for (what, name, grouped, pieces) in [
        ("an empty name", "", false, vec![one.clone()]),
        ("a path as a name", "a/b", false, vec![one.clone()]),
        ("a group of one", "x", true, vec![one.clone()]),
        ("no pieces", "x", false, vec![]),
        ("too far", "x", false, vec![one.clone(), piece(9000., 0.)]),
    ] {
        assert!(
            Prefab::capture(name, grouped, &pieces).is_err(),
            "{what} was accepted"
        );
    }
    let mut bad = Prefab::capture("x", false, &[one]).unwrap();
    bad.format = 2;
    assert!(bad.check().unwrap_err().to_string().contains("format 2"));
    bad.format = PREFAB_FORMAT;
    bad.pieces[0].tag.path = String::new();
    assert!(bad.check().is_err());
    bad.pieces[0].tag.path = "a".repeat(MAX_PREFAB_TAG + 1);
    assert!(bad.check().is_err());
}

#[test]
fn the_first_tag_the_map_lacks_is_named() {
    let mut other = piece(1., 0.);
    other.tag = tag(r"fixture\sphere");
    let prefab = Prefab::capture("two", true, &[piece(0., 0.), other]).unwrap();
    let has = |_: BlockKind, t: &TagKey| t.path == r"fixture\cube";
    assert_eq!(prefab.missing(has).unwrap().path, r"fixture\sphere");
    assert_eq!(prefab.missing(|_, _| true), None);
    // The block is part of the key.
    assert!(prefab.missing(|b, _| b == BlockKind::Weapons).is_some());
}

#[test]
fn label_targets_name_the_labelled_by_identity() {
    let label = Label::new("hill", Some(1)).unwrap();
    let doc = {
        let mut doc = super::draft();
        doc.ops = vec![
            Op {
                id: OpId(1),
                edit: Edit::Forge(Forge::Label {
                    target: Endpoint::base(BlockKind::Crates, 0),
                    label: Some(label.clone()),
                }),
            },
            Op {
                id: OpId(2),
                edit: Edit::Forge(Forge::Label {
                    target: Endpoint::base(BlockKind::Crates, 1),
                    label: Some(label.clone()),
                }),
            },
            Op {
                id: OpId(3),
                edit: Edit::Forge(Forge::Label {
                    target: Endpoint::base(BlockKind::Crates, 1),
                    label: None,
                }),
            },
        ];
        doc.format = doc.required_format();
        doc
    };
    let counts = BTreeMap::from([(BlockKind::Crates, 2)]);
    let targets = doc.label_targets(&counts).unwrap();
    assert_eq!(
        targets,
        BTreeMap::from([(Endpoint::base(BlockKind::Crates, 0), label)])
    );
    assert!(super::draft().label_targets(&counts).unwrap().is_empty());
}
