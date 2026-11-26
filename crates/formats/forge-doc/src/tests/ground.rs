//! AI ground in a document: the recipe, the marks and the stamp. All
//! inputs are invented.
use std::collections::BTreeMap;

use super::*;

const FLAGS: BlockKind = BlockKind::NetgameFlags;
const SCENERY: BlockKind = BlockKind::Scenery;
const WEAPONS: BlockKind = BlockKind::Weapons;

fn counts() -> BTreeMap<BlockKind, u32> {
    BTreeMap::from([(FLAGS, 3), (SCENERY, 2), (WEAPONS, 4)])
}

fn limits() -> Limits {
    Limits {
        base_counts: counts(),
        ..Limits::default()
    }
}

fn recipe() -> GroundRecipe {
    GroundRecipe {
        kit_version: 3,
        home: 0,
        probe: false,
        classes: vec![],
    }
}

/// An invented campaign map a class is copied from.
fn donor(map: &str, byte: u8) -> MapRef {
    MapRef {
        map: map.into(),
        sha256: Sha256([byte; 32]),
    }
}

fn class(name: &str, donor: MapRef) -> GroundClass {
    GroundClass {
        name: name.into(),
        donor,
    }
}

/// The recipe with a grunt and a hunter from one invented donor and a
/// brute from another.
fn with_classes() -> GroundRecipe {
    GroundRecipe {
        classes: vec![
            class("grunt", donor("outskirts", 0x22)),
            class("brute", donor("uprising", 0x33)),
            class("hunter", donor("outskirts", 0x22)),
        ],
        ..recipe()
    }
}

fn with(ops: Vec<Op>) -> ForgeDoc {
    let mut doc = draft();
    doc.ops = ops;
    doc.format = doc.required_format();
    doc
}

fn add(id: u32, block: BlockKind) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Placement(Placement::Add {
            block,
            like: None,
            fields: vec![],
        }),
    }
}

fn label(id: u32, target: Endpoint, text: &str, number: Option<i32>) -> Op {
    Op {
        id: OpId(id),
        edit: Edit::Forge(Forge::Label {
            target,
            label: Some(Label {
                text: text.into(),
                number,
            }),
        }),
    }
}

fn move_to(id: u32, block: BlockKind, index: u32, x: f32) -> Op {
    set(
        id,
        block,
        index,
        "position",
        Value::Vec3([
            F32::new(x).unwrap(),
            F32::new(0.).unwrap(),
            F32::new(0.).unwrap(),
        ]),
    )
}

#[test]
fn the_recipe_round_trips_and_reads_back_in_any_order() {
    for r in [
        recipe(),
        GroundRecipe {
            kit_version: 9,
            home: 1,
            probe: true,
            classes: vec![],
        },
    ] {
        let recipe = r.to_recipe();
        assert_eq!(GroundRecipe::of(&recipe).unwrap(), Some(r.clone()));
        let Recipe::Generate { mut parameters, .. } = recipe else {
            panic!("a generate recipe")
        };
        parameters.reverse();
        assert_eq!(GroundRecipe::from_parameters(&parameters).unwrap(), r);
    }
    assert_eq!(GroundRecipe::of(&kit()).unwrap(), None);
}

#[test]
fn a_malformed_recipe_is_refused_by_name() {
    let Recipe::Generate { parameters, .. } = recipe().to_recipe() else {
        panic!("a generate recipe")
    };
    let cases: Vec<(Vec<FieldWrite>, &str)> = vec![
        (parameters[..2].to_vec(), "want kit_version, home and mode"),
        (
            [parameters.clone(), parameters[..1].to_vec()].concat(),
            "twice",
        ),
        (
            vec![
                parameters[0].clone(),
                FieldWrite {
                    field: "home".into(),
                    value: Value::Int(2),
                },
                parameters[2].clone(),
            ],
            "red",
        ),
        (
            vec![
                parameters[0].clone(),
                parameters[1].clone(),
                FieldWrite {
                    field: "mode".into(),
                    value: Value::Text("wide".into()),
                },
            ],
            "kit or probe",
        ),
        (
            vec![FieldWrite {
                field: "slots".into(),
                value: Value::Int(4),
            }],
            "want kit_version",
        ),
    ];
    for (parameters, expected) in cases {
        let mut doc = draft();
        doc.recipes = vec![Recipe::Generate {
            generator: GROUND_RECIPE.into(),
            parameters,
        }];
        let error = validate(&doc, &Limits::default()).unwrap_err().to_string();
        assert!(error.contains(expected), "{error} did not say {expected}");
        assert_eq!(doc.tier(), Tier::Map, "{expected}");
    }
}

#[test]
fn a_variant_with_labels_and_a_recipe_validates_as_a_variant() {
    let mut doc = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
        label(3, Endpoint::base(FLAGS, 0), "ff_base", None),
    ]);
    doc.recipes.push(recipe().to_recipe());
    validate(&doc, &limits()).unwrap();
    assert_eq!(doc.tier(), Tier::Variant);
    doc.check_variant().unwrap();
    assert_eq!(doc.ground_recipe().unwrap(), Some(recipe()));
    // It survives the file and the play and session forms.
    let json = doc.canonical_json().unwrap();
    assert_eq!(
        ForgeDoc::from_bytes(json.as_bytes(), &Limits::default()).unwrap(),
        doc
    );
    assert_eq!(play_form(&doc), doc);
    let session = session_form(&doc);
    assert_eq!(session.recipes.last(), Some(&kit()));
    assert_eq!(session.tier(), Tier::Variant);
    assert_eq!(play_form(&session), doc);
    // A second recipe of another generator makes it a map again.
    doc.recipes.push(Recipe::Generate {
        generator: "box".into(),
        parameters: vec![],
    });
    assert_eq!(doc.tier(), Tier::Map);
}

#[test]
fn two_ground_recipes_are_refused() {
    let mut doc = draft();
    doc.recipes = vec![recipe().to_recipe(), recipe().to_recipe()];
    assert!(
        doc.ground_recipe()
            .unwrap_err()
            .to_string()
            .contains("once")
    );
}

#[test]
fn with_ground_puts_the_recipe_before_the_kit_and_drops_the_expectation() {
    let mut doc = session_form(&draft());
    doc.expect = Some(Expected {
        map_id: MapId([0x44; 32]),
        geometry: Sha256([0x55; 32]),
    });
    let built = doc.with_ground(Some(recipe()));
    assert_eq!(built.recipes, vec![recipe().to_recipe(), kit()]);
    assert_eq!(built.expect, None);
    assert_eq!(built.ground_recipe().unwrap(), Some(recipe()));
    // Asking for what the document has changes nothing.
    assert_eq!(built.with_ground(Some(recipe())), built);
    // Another recipe replaces it; none removes it.
    let probe = GroundRecipe {
        probe: true,
        ..recipe()
    };
    assert_eq!(
        built.with_ground(Some(probe.clone())).ground_recipe().unwrap(),
        Some(probe)
    );
    assert_eq!(built.with_ground(None).recipes, vec![kit()]);
    assert_eq!(draft().with_ground(None), draft());
}

#[test]
fn the_marks_are_the_ground_labels_on_flags_by_label_then_index() {
    let doc = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
        label(3, Endpoint::base(FLAGS, 2), "ff_drop", None),
        label(4, Endpoint::base(FLAGS, 1), "siege_line", Some(2)),
        label(5, Endpoint::base(FLAGS, 0), "ff_base", None),
        // Not a ground label: a script's own.
        label(6, Endpoint::base(WEAPONS, 3), "ctf_flag_spawn", None),
    ]);
    let marks = doc.ground_marks(&counts()).unwrap();
    let got: Vec<_> = marks.iter().map(|m| (m.kind, m.index, m.number)).collect();
    assert_eq!(
        got,
        [
            (GroundLabel::Drop, 2, None),
            (GroundLabel::Drop, 3, None),
            (GroundLabel::Base, 0, None),
            (GroundLabel::Line, 1, Some(2)),
        ]
    );
}

#[test]
fn a_ground_label_off_a_flag_or_a_line_with_no_phase_is_refused() {
    let off = with(vec![label(1, Endpoint::base(WEAPONS, 0), "ff_drop", None)]);
    let error = off.ground_marks(&counts()).unwrap_err().to_string();
    assert!(error.contains("netgame flag"), "{error}");
    let phaseless = with(vec![label(1, Endpoint::base(FLAGS, 0), "siege_line", None)]);
    let error = phaseless.ground_marks(&counts()).unwrap_err().to_string();
    assert!(error.contains("phase"), "{error}");
    let negative = with(vec![label(
        1,
        Endpoint::base(FLAGS, 0),
        "siege_line",
        Some(-1),
    )]);
    assert!(negative.ground_marks(&counts()).is_err());
}

#[test]
fn a_siege_barrier_marks_a_solid_object_numbered_by_its_phase() {
    let doc = with(vec![
        add(1, SCENERY),
        label(2, Endpoint::Added(OpId(1)), "siege_barrier", Some(2)),
        label(3, Endpoint::base(SCENERY, 0), "siege_barrier", None),
        label(4, Endpoint::base(FLAGS, 0), "ff_drop", None),
    ]);
    let marks = doc.ground_marks(&counts()).unwrap();
    let got: Vec<_> = marks.iter().map(|m| (m.kind, m.block, m.index, m.number)).collect();
    assert_eq!(
        got,
        [
            (GroundLabel::Drop, FLAGS, 0, None),
            (GroundLabel::Barrier, SCENERY, 0, None),
            (GroundLabel::Barrier, SCENERY, 2, Some(2)),
        ],
        "an unnumbered barrier is open from the start"
    );
    assert_eq!(GroundLabel::of("siege_barrier"), Some(GroundLabel::Barrier));
    assert!(GroundLabel::Barrier.has_phase() && !GroundLabel::Barrier.needs_phase());
    assert!(!GroundLabel::Barrier.on_flag() && !GroundLabel::Barrier.has_team());
    assert_eq!(GroundLabel::ALL.last(), Some(&GroundLabel::Barrier));
}

#[test]
fn a_barrier_off_a_solid_object_or_with_a_bad_phase_is_refused() {
    let flag = with(vec![label(1, Endpoint::base(FLAGS, 0), "siege_barrier", None)]);
    let error = flag.ground_marks(&counts()).unwrap_err().to_string();
    assert!(error.contains("placed object"), "{error}");
    let weapon = with(vec![label(1, Endpoint::base(WEAPONS, 0), "siege_barrier", Some(1))]);
    assert!(weapon.ground_marks(&counts()).is_err());
    let negative = with(vec![label(1, Endpoint::base(SCENERY, 0), "siege_barrier", Some(-1))]);
    let error = negative.ground_marks(&counts()).unwrap_err().to_string();
    assert!(error.contains("phase"), "{error}");
}

#[test]
fn a_barrier_is_no_input_of_the_ground() {
    let base = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
    ]);
    let stamp = |doc: &ForgeDoc| doc.ground_stamp(&counts()).unwrap();
    let mut barred = base.clone();
    barred.ops.push(label(3, Endpoint::base(SCENERY, 1), "siege_barrier", Some(1)));
    assert_eq!(stamp(&barred), stamp(&base), "labelling a barrier leaves the ground current");
}

#[test]
fn the_label_names_and_titles_are_one_table() {
    for label in GroundLabel::ALL {
        assert_eq!(GroundLabel::of(label.text()), Some(label));
        assert!(!label.title().is_empty());
        assert!(crate::Label::new(label.text(), None).is_ok());
    }
    assert_eq!(GroundLabel::of("ctf_flag_spawn"), None);
    let teamed: Vec<_> = GroundLabel::ALL
        .into_iter()
        .filter(|l| l.has_team())
        .collect();
    assert_eq!(teamed, [GroundLabel::Base, GroundLabel::Line]);
    let on_objects: Vec<_> = GroundLabel::ALL
        .into_iter()
        .filter(|l| !l.on_flag())
        .collect();
    assert_eq!(on_objects, [GroundLabel::Barrier]);
}

#[test]
fn a_mark_that_moves_or_a_solid_that_changes_changes_the_stamp() {
    let base = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
    ]);
    let stamp = |doc: &ForgeDoc| doc.ground_stamp(&counts()).unwrap();
    let was = stamp(&base);
    assert_eq!(stamp(&base.clone()), was);

    let mut moved = base.clone();
    moved.ops.push(move_to(3, FLAGS, 3, 4.));
    assert_ne!(stamp(&moved), was, "a mark moved");

    let mut again = moved.clone();
    again.ops.push(move_to(4, FLAGS, 3, 5.));
    assert_ne!(stamp(&again), stamp(&moved), "moved again");

    let mut wall = base.clone();
    wall.ops.push(add(3, SCENERY));
    assert_ne!(stamp(&wall), was, "a solid added");
    let mut shifted = wall.clone();
    shifted.ops.push(move_to(4, SCENERY, 0, 1.));
    assert_ne!(stamp(&shifted), stamp(&wall), "a solid moved");

    let mut cleared = base.clone();
    cleared.ops.push(Op {
        id: OpId(3),
        edit: Edit::Forge(Forge::Label {
            target: Endpoint::Added(OpId(1)),
            label: None,
        }),
    });
    assert_ne!(stamp(&cleared), was, "a mark cleared");
    let mut relabelled = base.clone();
    relabelled
        .ops
        .push(label(3, Endpoint::Added(OpId(1)), "ff_cover", None));
    assert_ne!(stamp(&relabelled), was, "a mark became another");

    let mut home = base.clone();
    home.recipes.push(recipe().to_recipe());
    assert_ne!(stamp(&home), was, "the recipe was added");
    let other = home.with_ground(Some(GroundRecipe {
        home: 1,
        ..recipe()
    }));
    assert_ne!(stamp(&other), stamp(&home), "the recipe changed");
}

#[test]
fn a_recipe_with_classes_round_trips() {
    let r = with_classes();
    let generated = r.to_recipe();
    assert_eq!(GroundRecipe::of(&generated).unwrap(), Some(r.clone()));
    let Recipe::Generate { mut parameters, .. } = generated else {
        panic!("a generate recipe")
    };
    // Three settings, then one parameter a class, in keep order.
    let fields: Vec<&str> = parameters.iter().map(|p| p.field.as_str()).collect();
    assert_eq!(
        fields,
        ["kit_version", "home", "mode", "class_grunt", "class_brute", "class_hunter"]
    );
    // Any order reads back in keep order.
    parameters.reverse();
    assert_eq!(GroundRecipe::from_parameters(&parameters).unwrap(), r);
    assert_eq!(r.class_names(), ["grunt", "brute", "hunter"]);

    // In a document: validated, kept by a save form and read back.
    let mut doc = draft();
    doc.recipes.push(r.to_recipe());
    doc.format = doc.required_format();
    validate(&doc, &limits()).unwrap();
    let json = doc.canonical_json().unwrap();
    let back = ForgeDoc::from_bytes(json.as_bytes(), &limits()).unwrap();
    assert_eq!(back.ground_recipe().unwrap(), Some(r.clone()));
    assert_eq!(back.id().unwrap(), doc.id().unwrap());
    // A recipe without classes keeps the identity it always had.
    let plain = draft().with_ground(Some(recipe()));
    let Recipe::Generate { parameters, .. } = &plain.recipes[0] else {
        panic!("a generate recipe")
    };
    assert_eq!(parameters.len(), 3);
    assert_ne!(plain.id().unwrap(), draft().with_ground(Some(r)).id().unwrap());
}

#[test]
fn a_malformed_class_is_refused_by_name() {
    let good = with_classes().to_recipe();
    let Recipe::Generate { parameters, .. } = good else {
        panic!("a generate recipe")
    };
    let hash = Sha256([0x22; 32]).to_string();
    let with = |field: &str, text: &str| {
        let mut p = parameters[..3].to_vec();
        p.push(FieldWrite {
            field: field.into(),
            value: Value::Text(text.into()),
        });
        p
    };
    let cases: Vec<(Vec<FieldWrite>, &str)> = vec![
        // The Elite is the kit's own, never copied.
        (with("class_elite", &format!("outskirts:{hash}")), "no enemy class"),
        (with("class_marine", &format!("outskirts:{hash}")), "no enemy class"),
        (with("class_grunt", "outskirts"), "donor:hash"),
        (with("class_grunt", "out skirts:00"), "plain map name"),
        (with("class_grunt", "outskirts:00"), "SHA-256"),
        (
            with("class_grunt", &format!("outskirts:{}", hash.to_uppercase())),
            "lowercase",
        ),
        (
            [
                with("class_grunt", &format!("outskirts:{hash}")),
                with("class_grunt", &format!("outskirts:{hash}"))[3..].to_vec(),
            ]
            .concat(),
            "twice",
        ),
        (
            [
                with("class_grunt", &format!("outskirts:{hash}")),
                with("class_jackal", &format!("outskirts:{}", Sha256([0x44; 32])))[3..].to_vec(),
            ]
            .concat(),
            "two hashes",
        ),
    ];
    for (parameters, want) in cases {
        let error = GroundRecipe::from_parameters(&parameters).unwrap_err();
        assert!(error.to_string().contains(want), "{want}: {error}");
        // The document's validation refuses it the same way.
        let mut doc = draft();
        doc.recipes.push(Recipe::Generate {
            generator: GROUND_RECIPE.into(),
            parameters,
        });
        assert!(validate(&doc, &limits()).is_err(), "{want}");
    }
}

#[test]
fn a_variant_may_name_class_donors_and_no_other_source() {
    let mut doc = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
    ]);
    doc.title = "Yard".into();
    doc.recipes.push(with_classes().to_recipe());
    validate(&doc, &limits()).unwrap();
    // The donors the recipe names do not make it a Forge map.
    assert_eq!(doc.tier(), Tier::Variant);
    doc.check_variant().unwrap();
    // Any other source map does.
    let mut sourced = doc.clone();
    sourced.sources.push(donor("outskirts", 0x22));
    assert_eq!(sourced.tier(), Tier::Map);
    assert_eq!(sourced.check_variant(), Err(Refusal::Tier));
}

#[test]
fn a_class_change_changes_the_stamp() {
    let base = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
    ]);
    let stamp = |r: GroundRecipe| {
        base.with_ground(Some(r))
            .ground_stamp(&counts())
            .unwrap()
    };
    let none = stamp(recipe());
    let all = stamp(with_classes());
    assert_ne!(none, all, "classes added");
    let mut cut = with_classes();
    cut.classes.pop();
    assert_ne!(stamp(cut), all, "a class cut");
    let mut moved = with_classes();
    moved.classes[1].donor = donor("uprising", 0x55);
    assert_ne!(stamp(moved), all, "a donor of another hash");
    assert_eq!(stamp(with_classes()), all, "the same list");
}

#[test]
fn an_edit_that_touches_neither_leaves_the_stamp() {
    let base = with(vec![
        add(1, FLAGS),
        label(2, Endpoint::Added(OpId(1)), "ff_drop", None),
    ]);
    let was = base.ground_stamp(&counts()).unwrap();
    let mut doc = base.clone();
    doc.ops.push(add(3, WEAPONS));
    doc.ops.push(move_to(4, WEAPONS, 0, 9.));
    doc.ops
        .push(label(5, Endpoint::Added(OpId(3)), "loot", Some(1)));
    doc.title = "Renamed".into();
    doc.description = "a note".into();
    assert_eq!(doc.ground_stamp(&counts()).unwrap(), was);
}
