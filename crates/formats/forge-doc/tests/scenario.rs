//! F1 parity against the editor on an invented cache.
mod support;

use forge_doc::{
    BlockKind, Edit, F32, FieldWrite, ForgeDoc, Limits, Placement, TagKey, Value, replay,
};
use map::FourCC;
use scenario_edit::{Command, MapSource, TagRef, Value as ScenarioValue, fixture::id};
use support::{ScenarioApply, base_counts, open, output};

fn draft(source: &MapSource, edits: Vec<Placement>) -> ForgeDoc {
    support::draft(source, edits.into_iter().map(Edit::Placement).collect())
}

#[test]
fn placement_replay_matches_the_sessions_patched_bytes() {
    let (session, source) = open();
    let (mut editor, _) = open();
    let mut target = ScenarioApply { session, source };
    let limits = Limits {
        base_counts: base_counts(&target.session),
        ..Limits::default()
    };

    let block = BlockKind::Weapons;
    let weapon = TagKey {
        group: "weap".into(),
        path: "objects\\test\\blaster".into(),
    };
    let position = [3.0, 4.0, 5.0];
    let doc = draft(
        &target.source,
        vec![
            Placement::Set {
                block,
                index: 0,
                write: FieldWrite {
                    field: "scale".into(),
                    value: Value::Float(F32::new(2.0).unwrap()),
                },
            },
            Placement::Add {
                block,
                like: Some(1),
                fields: vec![
                    FieldWrite {
                        field: "position".into(),
                        value: Value::Vec3(position.map(|v| F32::new(v).unwrap())),
                    },
                    FieldWrite {
                        field: "palette index".into(),
                        value: Value::Tag(Some(weapon.clone())),
                    },
                ],
            },
            Placement::Remove { block, index: 1 },
            Placement::Set {
                block: BlockKind::NetgameEquipment,
                index: 0,
                write: FieldWrite {
                    field: "item".into(),
                    value: Value::Tag(Some(weapon)),
                },
            },
        ],
    );
    let doc = ForgeDoc::from_bytes(doc.canonical_json().unwrap().as_bytes(), &limits).unwrap();
    replay(&doc, &limits, &mut target).unwrap();

    editor
        .apply(Command::set(block, 0, "scale", ScenarioValue::Float(2.0)))
        .unwrap();
    let bytes = editor
        .scenario()
        .unwrap()
        .new_entry(block, Some(1), Some(position))
        .unwrap();
    editor.apply(Command::add(block, bytes)).unwrap();
    editor
        .apply(Command::set(
            block,
            2,
            "palette index",
            ScenarioValue::Int(0),
        ))
        .unwrap();
    editor.apply(Command::remove(block, 1)).unwrap();
    editor
        .apply(Command::set(
            BlockKind::NetgameEquipment,
            0,
            "item",
            ScenarioValue::Tag(TagRef {
                group: FourCC(*b"weap"),
                id: id(2),
            }),
        ))
        .unwrap();

    assert_eq!(
        target.session.scenario().unwrap().changes(),
        editor.scenario().unwrap().changes()
    );
    assert_eq!(
        output(&target.source, &target.session),
        output(&target.source, &editor)
    );
}

#[test]
fn a_portable_tag_key_must_resolve_before_a_placement_is_written() {
    let (session, source) = open();
    let mut target = ScenarioApply { session, source };
    let doc = draft(
        &target.source,
        vec![Placement::Add {
            block: BlockKind::Weapons,
            like: Some(1),
            fields: vec![FieldWrite {
                field: "palette index".into(),
                value: Value::Tag(Some(TagKey {
                    group: "weap".into(),
                    path: "missing".into(),
                })),
            }],
        }],
    );
    assert!(replay(&doc, &Limits::default(), &mut target).is_err());
    assert!(!target.session.scenario().unwrap().is_modified());
}
