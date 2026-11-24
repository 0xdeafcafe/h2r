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
        ["Coordinates...", "Heading"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::StartingLocations,
            ThingKind::StartingPoint,
            &[("team", 1)]
        )),
        ["Coordinates...", "Heading", "Team"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            &[("type", 0), ("team", 0), ("identifier", 0)]
        )),
        ["Coordinates...", "Heading", "Team", "Role", "Order"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameEquipment,
            ThingKind::ItemSpawner,
            &[("respawn time", 30)]
        )),
        ["Coordinates...", "Heading", "Respawn time"]
    );
    // A teleporter has no row of its own yet.
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameFlags,
            ThingKind::Teleporter,
            &[("type", 7)]
        )),
        ["Coordinates...", "Heading"]
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
        ["Coordinates...", "Heading"]
    );
    assert_eq!(
        labels(&thing(
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            &[("type", 0), ("team", 0)]
        )),
        ["Coordinates...", "Heading", "Team", "Role"]
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
fn every_row_of_the_shipped_doc_binds() {
    // Each kind's every row of the doc is made: none is dropped for a
    // property the binder cannot bind.
    let d = doc();
    for (block, kind, fields) in [
        (BlockKind::Scenery, ThingKind::Object, &[][..]),
        (
            BlockKind::StartingLocations,
            ThingKind::StartingPoint,
            &[("team", 0)][..],
        ),
        (
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            &[("type", 0), ("team", 0), ("identifier", 0)][..],
        ),
        (
            BlockKind::NetgameEquipment,
            ThingKind::ItemSpawner,
            &[
                ("respawn time", 0),
                ("game type 1", 2),
                ("game type 2", 2),
                ("game type 3", 2),
                ("game type 4", 2),
            ][..],
        ),
    ] {
        let t = thing(block, kind, fields);
        let wanted = d.props.values().filter(|p| is_for(p, kind)).count();
        // With a label and objects to save, every row of the doc binds.
        let all = Extras {
            labellable: true,
            label: Some(("checkpoint".into(), Some(3))),
            savable: 2,
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
        ["Coordinates...", "Heading", "Respawn time", "Game types..."]
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
    assert_eq!(labels(&odd), ["Coordinates...", "Heading", "Respawn time"]);
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
        ["Coordinates...", "Heading", "Label"]
    );
    let savable = Extras {
        savable: 3,
        ..plain.clone()
    };
    assert_eq!(
        labels_with(&object, &savable),
        ["Coordinates...", "Heading", "Label", "Save as prefab"]
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
            "Save as prefab"
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
    assert_eq!(labels(&object), ["Coordinates...", "Heading"]);
}

#[test]
fn the_label_rows_show_their_values_and_open_the_keyboard_or_act() {
    let object = thing(BlockKind::Scenery, ThingKind::Object, &[]);
    let extras = Extras {
        labellable: true,
        label: Some(("checkpoint".into(), Some(3))),
        savable: 2,
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
