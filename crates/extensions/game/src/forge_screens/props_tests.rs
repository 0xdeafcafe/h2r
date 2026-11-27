use super::*;
use crate::forge_screens::pages::sample_thing;
use crate::forge_screens::sheet::Choice;
use crate::ui_kit::data;
use forge::{BlockKind, Frame, Value};

fn doc() -> std::sync::Arc<ScreenDoc> {
    data::require("forge.properties").unwrap_or_else(|e| panic!("{e}"))
}

fn thing(block: BlockKind, kind: ThingKind, fields: &[(&'static str, i64)]) -> Thing {
    Thing {
        fields: fields.iter().map(|(n, v)| (*n, Value::Int(*v))).collect(),
        frame: Frame::default(),
        ..sample_thing(block, kind)
    }
}

fn labels(thing: &Thing) -> Vec<String> {
    rows(&doc(), thing, 0.0)
        .into_iter()
        .map(|r| r.label)
        .collect()
}

#[test]
fn the_rows_of_a_kind_are_the_docs_in_its_order() {
    assert_eq!(
        labels(&thing(BlockKind::Scenery, ThingKind::Object, &[])),
        ["Coordinates...", "Heading", "Reset orientation", "Delete"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::StartingLocations,
            ThingKind::StartingPoint,
            &[("team", 1)]
        )),
        ["Coordinates...", "Heading", "Team", "Reset orientation", "Delete"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            &[("type", 0), ("team", 0), ("identifier", 0)]
        )),
        [
            "Coordinates...",
            "Heading",
            "Team",
            "Role",
            "Order",
            "Reset orientation",
            "Delete",
        ]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameEquipment,
            ThingKind::ItemSpawner,
            &[("respawn time", 30)]
        )),
        [
            "Coordinates...",
            "Heading",
            "Respawn time",
            "Team",
            "Reset orientation",
            "Delete",
        ]
    );
    // A teleporter has no row of its own yet.
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameFlags,
            ThingKind::Teleporter,
            &[("type", 7)]
        )),
        ["Coordinates...", "Heading", "Reset orientation", "Delete"]
    );
}

#[test]
fn a_property_the_thing_lacks_has_no_row() {
    // A starting point that holds no team, and a goal with no identifier.
    assert_eq!(
        labels(&thing(
            BlockKind::StartingLocations,
            ThingKind::StartingPoint,
            &[]
        )),
        ["Coordinates...", "Heading", "Reset orientation", "Delete"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            &[("type", 0), ("team", 0)]
        )),
        [
            "Coordinates...",
            "Heading",
            "Team",
            "Role",
            "Reset orientation",
            "Delete",
        ]
    );
}

#[test]
fn rows_show_what_the_thing_holds_and_send_their_index() {
    let goal = thing(
        BlockKind::NetgameFlags,
        ThingKind::Goal,
        &[("type", 12), ("team", 2), ("identifier", 5)],
    );
    let rows = rows(&doc(), &goal, 90.0);
    assert!(matches!(rows[0].act, Act::Open(Kind::Coordinates)));
    assert_eq!(rows[1].value.as_deref(), Some("90 degrees"));
    assert_eq!(rows[2].value.as_deref(), Some("Green"));
    assert_eq!(rows[3].value.as_deref(), Some("Hill 2"));
    assert_eq!(rows[4].value.as_deref(), Some("5"));
    let Act::Pick(p) = &rows[2].act else {
        panic!("a picker")
    };
    assert_eq!(p.choices[p.current], Choice::Set(Prop::Team, 3));
    assert_eq!(p.values.len(), 10);
    assert_eq!(
        rows[2].description.as_deref(),
        Some("Which team this goal belongs to.")
    );
    // The engine's no-team value is the last choice.
    let neutral = thing(
        BlockKind::NetgameFlags,
        ThingKind::Goal,
        &[("type", 0), ("team", 255), ("identifier", 0)],
    );
    assert_eq!(
        super::rows(&doc(), &neutral, 0.0)[2].value.as_deref(),
        Some("Neutral")
    );
    let spawner = thing(
        BlockKind::NetgameEquipment,
        ThingKind::ItemSpawner,
        &[("respawn time", 30)],
    );
    let rows = super::rows(&doc(), &spawner, 0.0);
    let Act::Type(t) = &rows[2].act else {
        panic!("the keyboard")
    };
    assert_eq!(
        (t.field, t.initial.as_str()),
        (NumberField::RespawnTime, "30")
    );
    assert_eq!(rows[2].value.as_deref(), Some("30"));
}

#[test]
fn a_spawner_with_an_abandon_limit_has_the_return_time_row_after_respawn_time() {
    let spawner = thing(
        BlockKind::NetgameEquipment,
        ThingKind::ItemSpawner,
        &[("respawn time", 30), ("abandon limit", 120)],
    );
    assert_eq!(
        labels(&spawner),
        [
            "Coordinates...",
            "Heading",
            "Respawn time",
            "Return time",
            "Team",
            "Reset orientation",
            "Delete",
        ]
    );
    let rows = rows(&doc(), &spawner, 0.0);
    let Act::Type(t) = &rows[3].act else {
        panic!("the keyboard")
    };
    assert_eq!(
        (t.field, t.initial.as_str()),
        (NumberField::ReturnTime, "120")
    );
    assert_eq!(rows[3].value.as_deref(), Some("120"));
    // A thing whose block has no abandon limit shows no row.
    assert!(
        !labels(&thing(
            BlockKind::NetgameEquipment,
            ThingKind::ItemSpawner,
            &[("respawn time", 30)]
        ))
        .contains(&"Return time".to_string())
    );
}

#[test]
fn every_row_of_the_shipped_doc_binds() {
    // Each kind's every row of the doc is made: none is dropped for a
    // property the binder cannot bind.
    let d = doc();
    let tagged = |block, kind| Thing {
        fields: vec![(
            "palette index",
            forge::Value::Tag(Some(forge::TagKey {
                group: "bloc".into(),
                path: r"fixture\cube".into(),
            })),
        )],
        ..sample_thing(block, kind)
    };
    for (_, kind, t) in [
        (
            BlockKind::Scenery,
            ThingKind::Object,
            tagged(BlockKind::Scenery, ThingKind::Object),
        ),
        (
            BlockKind::StartingLocations,
            ThingKind::StartingPoint,
            thing(
                BlockKind::StartingLocations,
                ThingKind::StartingPoint,
                &[("team", 0)],
            ),
        ),
        (
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            thing(
                BlockKind::NetgameFlags,
                ThingKind::Goal,
                &[("type", 0), ("team", 0), ("identifier", 0)],
            ),
        ),
        (
            BlockKind::NetgameEquipment,
            ThingKind::ItemSpawner,
            thing(
                BlockKind::NetgameEquipment,
                ThingKind::ItemSpawner,
                &[
                    ("respawn time", 0),
                    ("abandon limit", 0),
                    ("game type 1", 2),
                    ("game type 2", 2),
                    ("game type 3", 2),
                    ("game type 4", 2),
                ],
            ),
        ),
    ] {
        let wanted = d.props.values().filter(|p| is_for(p, kind)).count();
        // With a label and objects to save, every row of the doc binds.
        let all = Extras {
            labellable: true,
            label: Some(("checkpoint".into(), Some(3))),
            savable: 2,
            kit: true,
            team: None,
            spare_clips: Some(None),
            weight: Some(None),
        };
        let t = Thing {
            own: true,
            changed: true,
            ..t
        };
        assert_eq!(rows_with(&d, &t, 0.0, &all).len(), wanted, "{kind:?}");
    }
}

#[test]
fn a_spawner_with_game_type_slots_has_the_game_types_row_and_it_opens_its_dialog() {
    use crate::forge_screens::games::{self, Tick};
    use forge_doc::games::GameSet;
    let slots = |v: i64| {
        [
            ("respawn time", 30),
            ("game type 1", v),
            ("game type 2", v),
            ("game type 3", v),
            ("game type 4", v),
        ]
    };
    let spawner = thing(
        BlockKind::NetgameEquipment,
        ThingKind::ItemSpawner,
        &slots(2),
    );
    assert_eq!(
        labels(&spawner),
        [
            "Coordinates...",
            "Heading",
            "Respawn time",
            "Game types...",
            "Team",
            "Reset orientation",
            "Delete",
        ]
    );
    let rows = rows(&doc(), &spawner, 0.0);
    let Act::Dialog(dialog) = &rows[3].act else {
        panic!("the row opens the dialog")
    };
    // The dialog shows the set the slots name: Slayer alone.
    assert_eq!(dialog.kind, Kind::Games);
    assert_eq!(games::set_of(dialog), GameSet::of(&[2]));
    let mut dialog = (**dialog).clone();
    games::tick(&mut dialog, Tick::Engine(3));
    assert_eq!(games::set_of(&dialog), GameSet::of(&[2, 3]));
    // A slot value the table does not know is shown as it is: no row.
    let odd = thing(
        BlockKind::NetgameEquipment,
        ThingKind::ItemSpawner,
        &slots(99),
    );
    assert_eq!(
        labels(&odd),
        [
            "Coordinates...",
            "Heading",
            "Respawn time",
            "Team",
            "Reset orientation",
            "Delete",
        ]
    );
    // Only a spawner has the row.
    assert!(
        !labels(&thing(BlockKind::Scenery, ThingKind::Object, &[]))
            .contains(&"Game types...".to_string())
    );
}

fn labels_with(thing: &Thing, extras: &Extras) -> Vec<String> {
    rows_with(&doc(), thing, 0.0, extras)
        .into_iter()
        .map(|r| r.label)
        .collect()
}

#[test]
fn label_and_prefab_rows_come_with_what_the_session_says_of_the_thing() {
    let object = thing(BlockKind::Scenery, ThingKind::Object, &[]);
    let plain = Extras {
        labellable: true,
        ..Extras::default()
    };
    assert_eq!(
        labels_with(&object, &plain),
        ["Coordinates...", "Heading", "Label", "Reset orientation", "Delete"]
    );
    let savable = Extras {
        savable: 3,
        ..plain.clone()
    };
    assert_eq!(
        labels_with(&object, &savable),
        [
            "Coordinates...",
            "Heading",
            "Label",
            "Save as prefab",
            "Reset orientation",
            "Delete",
        ]
    );
    let labelled = Extras {
        label: Some(("hill".into(), None)),
        ..savable
    };
    assert_eq!(
        labels_with(&object, &labelled),
        [
            "Coordinates...",
            "Heading",
            "Label",
            "Label number",
            "Clear label",
            "Save as prefab",
            "Reset orientation",
            "Delete",
        ]
    );
    // Only an object is saved as a prefab; any thing is labelled.
    let goal = thing(
        BlockKind::NetgameFlags,
        ThingKind::Goal,
        &[("type", 0), ("team", 0), ("identifier", 0)],
    );
    assert!(!labels_with(&goal, &labelled).contains(&"Save as prefab".to_string()));
    assert!(labels_with(&goal, &labelled).contains(&"Label".to_string()));
    // The default has none of them.
    assert_eq!(
        labels(&object),
        ["Coordinates...", "Heading", "Reset orientation", "Delete"]
    );
}

#[test]
fn the_label_rows_show_their_values_and_open_the_keyboard_or_act() {
    let object = thing(BlockKind::Scenery, ThingKind::Object, &[]);
    let extras = Extras {
        labellable: true,
        label: Some(("checkpoint".into(), Some(3))),
        savable: 2,
        kit: false,
        team: None,
        spare_clips: None,
        weight: None,
    };
    let rows = rows_with(&doc(), &object, 0.0, &extras);
    let by = |name: &str| rows.iter().find(|r| r.label == name).unwrap();
    let Act::Type(t) = &by("Label").act else {
        panic!("the keyboard")
    };
    assert_eq!(
        (t.field, t.initial.as_str()),
        (NumberField::Label, "checkpoint")
    );
    assert_eq!(by("Label").value.as_deref(), Some("checkpoint"));
    let Act::Type(t) = &by("Label number").act else {
        panic!("the keyboard")
    };
    assert_eq!(
        (t.field, t.initial.as_str()),
        (NumberField::LabelNumber, "3")
    );
    assert_eq!(
        by("Clear label").act,
        Act::Choose(Choice::ClearLabel(object.id))
    );
    let Act::Type(t) = &by("Save as prefab").act else {
        panic!("the keyboard")
    };
    assert_eq!((t.field, t.initial.as_str()), (NumberField::PrefabName, ""));
    assert_eq!(by("Save as prefab").value.as_deref(), Some("2 objects"));
    // No label: the Label row says so.
    let none = Extras {
        labellable: true,
        ..Extras::default()
    };
    let rows = rows_with(&doc(), &object, 0.0, &none);
    assert_eq!(rows[2].value.as_deref(), Some("None"));
}

fn object_with_tag() -> Thing {
    Thing {
        fields: vec![(
            "palette index",
            Value::Tag(Some(forge::TagKey {
                group: "bloc".into(),
                path: r"fixture\cube".into(),
            })),
        )],
        ..sample_thing(BlockKind::Scenery, ThingKind::Object)
    }
}

#[test]
fn reset_orientation_is_an_act_for_every_kind_with_a_turn_field() {
    let d = doc();
    for (block, kind) in [
        (BlockKind::Scenery, ThingKind::Object),
        (BlockKind::StartingLocations, ThingKind::StartingPoint),
        (BlockKind::NetgameFlags, ThingKind::Goal),
        (BlockKind::NetgameEquipment, ThingKind::ItemSpawner),
    ] {
        let t = thing(block, kind, &[]);
        let row = rows(&d, &t, 0.0)
            .into_iter()
            .find(|r| r.label == "Reset orientation")
            .unwrap_or_else(|| panic!("{kind:?} has the row"));
        assert_eq!(row.act, Act::Choose(Choice::ResetOrientation), "{kind:?}");
    }
}

#[test]
fn duplicate_is_an_act_on_a_placed_object_only() {
    let d = doc();
    let own = object_with_tag();
    let row = rows(&d, &own, 0.0)
        .into_iter()
        .find(|r| r.label == "Duplicate")
        .expect("an own object has the row");
    assert_eq!(row.act, Act::Choose(Choice::Duplicate(own.id)));
    // An added one has it too, with its own endpoint.
    let added = Thing {
        id: forge::Endpoint::Added(forge::OpId(4)),
        own: false,
        ..own.clone()
    };
    let row = rows(&d, &added, 0.0)
        .into_iter()
        .find(|r| r.label == "Duplicate")
        .expect("an added object has the row");
    assert_eq!(row.act, Act::Choose(Choice::Duplicate(added.id)));
    // The kinds the kit adds have none without a kit session, and neither
    // has the ghost of an object being placed, which names no thing.
    for (block, kind) in [
        (BlockKind::NetgameEquipment, ThingKind::ItemSpawner),
        (BlockKind::StartingLocations, ThingKind::StartingPoint),
        (BlockKind::NetgameFlags, ThingKind::Goal),
        (BlockKind::NetgameFlags, ThingKind::Teleporter),
    ] {
        assert!(
            !labels(&thing(block, kind, &[])).contains(&"Duplicate".to_string()),
            "{kind:?}"
        );
        // In a kit session the same kinds have it, as an act on their id.
        let t = thing(block, kind, &[]);
        let kit = Extras {
            kit: true,
            ..Extras::default()
        };
        let row = rows_with(&d, &t, 0.0, &kit)
            .into_iter()
            .find(|r| r.label == "Duplicate")
            .unwrap_or_else(|| panic!("{kind:?} has the row in a kit session"));
        assert_eq!(row.act, Act::Choose(Choice::Duplicate(t.id)), "{kind:?}");
    }
    assert!(
        !labels(&thing(BlockKind::Scenery, ThingKind::Object, &[]))
            .contains(&"Duplicate".to_string()),
        "a thing with no palette tag is the ghost of one being placed"
    );
}

#[test]
fn delete_is_on_every_kind_and_restore_original_only_on_a_changed_own_thing() {
    let d = doc();
    for (block, kind) in [
        (BlockKind::Scenery, ThingKind::Object),
        (BlockKind::NetgameEquipment, ThingKind::ItemSpawner),
        (BlockKind::StartingLocations, ThingKind::StartingPoint),
        (BlockKind::NetgameFlags, ThingKind::Goal),
        (BlockKind::NetgameFlags, ThingKind::Teleporter),
    ] {
        let t = thing(block, kind, &[]);
        let all = rows(&d, &t, 0.0);
        let delete = all.iter().find(|r| r.label == "Delete").expect("Delete");
        assert_eq!(delete.act, Act::Choose(Choice::Delete(t.id)), "{kind:?}");
        let restore = |own, changed| {
            let t = Thing {
                own,
                changed,
                ..t.clone()
            };
            rows(&d, &t, 0.0)
                .iter()
                .find(|r| r.label == "Restore original")
                .map(|r| (r.act.clone(), t.id))
        };
        assert_eq!(restore(false, true), None, "{kind:?}: not the map's own");
        assert_eq!(restore(true, false), None, "{kind:?}: not changed");
        let (act, id) = restore(true, true).expect("an own changed thing has it");
        assert_eq!(act, Act::Choose(Choice::Restore(id)), "{kind:?}");
    }
}

#[test]
fn a_spawner_has_a_team_picker_over_the_documents_property() {
    let d = doc();
    let spawner = thing(BlockKind::NetgameEquipment, ThingKind::ItemSpawner, &[]);
    let team_row = |team| {
        let extras = Extras {
            team,
            ..Extras::default()
        };
        rows_with(&d, &spawner, 0.0, &extras)
            .into_iter()
            .find(|r| r.label == "Team")
            .expect("a spawner has the Team row")
    };
    let Act::Pick(none) = team_row(None).act else {
        panic!("a picker")
    };
    assert_eq!(none.values.len(), 10);
    assert_eq!(none.values[none.current], "None");
    assert_eq!(none.choices[none.current], Choice::Set(Prop::SpawnerTeam, 0));
    // Team 1 is Blue, the third value.
    let Act::Pick(blue) = team_row(Some(1)).act else {
        panic!("a picker")
    };
    assert_eq!(blue.values[blue.current], "Blue");
}

#[test]
fn a_collection_spawner_has_a_spare_clips_picker_over_the_documents_property() {
    let d = doc();
    let spawner = thing(BlockKind::NetgameEquipment, ThingKind::ItemSpawner, &[]);
    let clips_row = |spare_clips| {
        let extras = Extras {
            spare_clips,
            ..Extras::default()
        };
        rows_with(&d, &spawner, 0.0, &extras)
            .into_iter()
            .find(|r| r.label == "Spare clips")
    };
    // A spawner the row is not for (a vehicle's) has none.
    assert!(clips_row(None).is_none());
    let Some(SheetRow { act: Act::Pick(own), .. }) = clips_row(Some(None)) else {
        panic!("a picker")
    };
    assert_eq!(own.values.len(), 12);
    assert_eq!(own.values[own.current], "Weapon's own");
    assert_eq!(own.choices[own.current], Choice::Set(Prop::SpareClips, 0));
    let Some(SheetRow { act: Act::Pick(two), .. }) = clips_row(Some(Some(2))) else {
        panic!("a picker")
    };
    assert_eq!(two.values[two.current], "2");
    assert_eq!(two.choices[two.current], Choice::Set(Prop::SpareClips, 3));
}

#[test]
fn a_starting_point_has_a_weight_picker_over_the_documents_property() {
    let d = doc();
    let start = thing(BlockKind::StartingLocations, ThingKind::StartingPoint, &[]);
    let weight_row = |weight| {
        let extras = Extras {
            weight,
            ..Extras::default()
        };
        rows_with(&d, &start, 0.0, &extras)
            .into_iter()
            .find(|r| r.label == "Weight")
    };
    assert!(weight_row(None).is_none(), "a thing the row is not for");
    let Some(SheetRow { act: Act::Pick(normal), .. }) = weight_row(Some(None)) else {
        panic!("a picker")
    };
    assert_eq!(normal.values, ["Low", "Normal", "High"]);
    assert_eq!(normal.values[normal.current], "Normal");
    assert_eq!(normal.choices[normal.current], Choice::Set(Prop::SpawnWeight, 1));
    let Some(SheetRow { act: Act::Pick(high), .. }) =
        weight_row(Some(Some(forge_doc::SpawnWeight::High)))
    else {
        panic!("a picker")
    };
    assert_eq!(high.values[high.current], "High");
}
