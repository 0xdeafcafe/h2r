//! Package writes through anim.save on the animation editor's invented graph.

use super::*;
use anim_edit::{ClipKind, Operation, Source};
use content::package::{Kind as ItemKind, Maps};
use ::tags::anim_keys::Kind;

const TAG: &str = r"jmad:invented\stickman";

fn open(test: &str) -> (Temp, App, Api) {
    let t = Temp::new(test);
    let mut app = app(&t);
    let map = t.path("timeline.map");
    std::fs::write(&map, crate::api::anim::read_tests::read_map()).unwrap();
    app.dispatch(Action::OpenSource(map));
    import_tests::package(&t.path("data/packages/mine"), "mine");
    (t, app, Api::new())
}

#[test]
fn anim_save_updates_the_same_map_scoped_item_and_round_trips_keys() {
    let (t, mut app, mut api) = open("anim-save-update");
    let made = call(
        &mut api,
        &mut app,
        m::ANIM_NEW,
        json!({"tag": TAG, "name": "wave", "frames": 6}),
    );
    let doc = made["doc"].as_u64().unwrap();
    let params = json!({"doc": doc, "action": "wave"});
    assert_eq!(
        code_of(api.call(&mut app, m::ANIM_SAVE, params.clone())),
        code::NO_TARGET
    );
    let action = Action::SaveAnimation {
        doc,
        placement: Some(crate::app::anim::DraftPlacement {
            action: "wave".into(),
            ..Default::default()
        }),
    };
    app.dispatch(action);
    assert!(app.edits.picker.is_some());
    app.dispatch(Action::ChooseEditTarget(crate::app::EditChoice::Existing("mine".into())));
    assert!(app.edits.picker.is_none());
    let dir = t.path("data/packages/mine");
    let saved = call(&mut api, &mut app, m::ANIM_SAVE, params.clone());
    let path = saved["source"].as_str().unwrap().to_owned();
    let before = std::fs::read(dir.join(&path)).unwrap();
    assert_eq!(saved["tag"], format!("tag:{TAG}"));
    assert_eq!(saved["item"], 0);
    assert!(path.ends_with(".anim.yaml"));
    call(
        &mut api,
        &mut app,
        m::ANIM_KEY,
        json!({
            "doc": doc, "node": "arm", "channel": "translation", "frame": 3,
            "value": [0.25, 0.0, 1.5], "interp": "smooth"
        }),
    );
    assert_eq!(call(&mut api, &mut app, m::ANIM_SAVE, params), saved);
    let loaded = ::workspace::Loaded::from_dir(&dir).unwrap();
    assert_eq!(loaded.package.manifest.items.len(), 1);
    assert_eq!(loaded.package.manifest.items[0].kind, ItemKind::Animation);
    assert_eq!(
        loaded.package.manifest.items[0].maps,
        Maps::Names(vec![saved["map"].as_str().unwrap().to_owned()])
    );
    let bytes = loaded.files.get(&path).unwrap();
    assert_ne!(bytes, &before);
    let source = Source::parse(bytes).unwrap();
    assert!(matches!(
        &source.operation,
        Operation::Add { mode, class, weapon_type, action, kind: ClipKind::Base }
            if mode == "any" && class == "any" && weapon_type == "any" && action == "wave"
    ));
    let saved_doc = source.doc().unwrap();
    assert_eq!(
        saved_doc.key(1, Kind::Translation, 3),
        app.anim_doc(doc).unwrap().key(1, Kind::Translation, 3)
    );
    let lock = ::workspace::read_lock(&dir).unwrap().unwrap();
    assert_eq!(lock, loaded.package.lock);
    assert!(::workspace::check_package(&loaded).is_empty());
}

#[test]
fn anim_save_replaces_a_source_clip_and_refusals_leave_the_package_alone() {
    let (t, mut app, mut api) = open("anim-save-replace");
    let read = call(
        &mut api,
        &mut app,
        m::ANIM_READ,
        json!({"tag": TAG, "animation": "combat:idle"}),
    );
    let doc = read["doc"].as_u64().unwrap();
    let params = json!({"doc": doc, "package": "mine"});
    let saved = call(&mut api, &mut app, m::ANIM_SAVE, params.clone());
    let dir = t.path("data/packages/mine");
    let source = dir.join(saved["source"].as_str().unwrap());
    let before = std::fs::read(&source).unwrap();
    assert!(matches!(
        Source::parse(&before).unwrap().operation,
        Operation::Replace { animation } if animation == "combat:idle"
    ));
    let changed_action = json!({"doc": doc, "package": "mine", "action": "other"});
    assert_eq!(
        code_of(api.call(&mut app, m::ANIM_SAVE, changed_action)),
        code::INVALID_PARAMS
    );
    assert_eq!(std::fs::read(&source).unwrap(), before);

    // The editor won't overwrite a source that has no matching item.
    import_tests::package(&dir, "mine");
    assert_eq!(
        code_of(api.call(&mut app, m::ANIM_SAVE, params)),
        code::REFUSED
    );
    assert_eq!(std::fs::read(source).unwrap(), before);
    assert!(
        ::workspace::Loaded::from_dir(&dir)
            .unwrap()
            .package
            .manifest
            .items
            .is_empty()
    );
}
