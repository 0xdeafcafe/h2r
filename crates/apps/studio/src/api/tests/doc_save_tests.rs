//! The shared Save action and the socket write the same package items.

use super::*;
use crate::app::{Doc, EditChoice, ToolKind};
use content::package::Kind;

#[test]
fn variant_rules_and_playlist_save_only_in_the_chosen_package() {
    let t = Temp::new("one-save");
    let mut app = app(&t);
    let mut api = Api::new();
    app.edits_to_target();
    call(&mut api, &mut app, m::DOC_OPEN, json!({"variant": {"new": "slayer"}}));
    let script = t.path("invented.rules");
    let script_text = "rules \"Invented\" 1\ntype slayer\nuses slayer\n";
    std::fs::write(&script, script_text).unwrap();
    call(&mut api, &mut app, m::RULES_OPEN, json!({"path": script}));
    let playlist = t.path("multiplayer_hopper.dat");
    let playlist_before = std::fs::read(&playlist).unwrap();
    call(&mut api, &mut app, m::DOC_OPEN, json!({"playlists": playlist}));

    for (doc, word) in [(Doc::Variant, "variant"), (Doc::Rules, "rules"), (Doc::Playlists, "playlists")] {
        assert_eq!(code_of(api.call(&mut app, m::DOC_SAVE, json!({"doc": word}))), code::NO_TARGET);
        app.dispatch(app.doc_save_action(doc).unwrap());
        assert!(app.edits.picker.is_some(), "{word}");
        app.dispatch(Action::ChooseEditTarget(EditChoice::Cancel));
        assert!(app.edits.picker.is_none(), "{word}");
    }

    app.dispatch(app.doc_save_action(Doc::Variant).unwrap());
    app.dispatch(Action::ChooseEditTarget(EditChoice::New("Save Here".into())));
    let dir = t.path("data/packages/save-here");
    assert_eq!(::workspace::Loaded::from_dir(&dir).unwrap().package.manifest.items.len(), 1);
    for word in ["variant", "rules", "playlists"] {
        let params = json!({"doc": word});
        let first = call(&mut api, &mut app, m::DOC_SAVE, params.clone());
        let second = call(&mut api, &mut app, m::DOC_SAVE, params);
        assert_eq!(first["path"], second["path"], "{word}");
        assert!(Path::new(first["path"].as_str().unwrap()).starts_with(&dir));
    }

    let loaded = ::workspace::Loaded::from_dir(&dir).unwrap();
    let kinds: Vec<_> = loaded.package.manifest.items.iter().map(|item| item.kind).collect();
    assert_eq!(kinds, [Kind::Variant, Kind::Rules, Kind::Playlist]);
    assert_eq!(::workspace::read_lock(&dir).unwrap().unwrap(), loaded.package.lock);
    assert_eq!(std::fs::read_to_string(script).unwrap(), script_text);
    assert_eq!(std::fs::read(playlist).unwrap(), playlist_before);
    assert!(!t.path("saves").exists());
    assert!(!app.rules.dirty());
    assert!(!app.tool_dirty(ToolKind::Variants));
    assert!(!app.tool_dirty(ToolKind::Playlists));
}

#[test]
fn the_unsaved_prompt_resumes_after_choosing_a_target_and_saving() {
    let t = Temp::new("save-prompt-target");
    let mut app = app(&t);
    app.edits_to_target();
    app.dispatch(Action::NewVariant(::variants::GameType::Slayer));
    app.dispatch(Action::NewVariant(::variants::GameType::Ctf));
    app.resolve(crate::app::Answer::Save);
    assert!(app.pending().is_some());
    assert!(app.edits.picker.is_some());
    app.dispatch(Action::ChooseEditTarget(EditChoice::New("Prompt Target".into())));
    assert!(app.pending().is_none());
    let loaded = ::workspace::Loaded::from_dir(&t.path("data/packages/prompt-target")).unwrap();
    assert_eq!(loaded.package.manifest.items.len(), 1);
    let editor = app.variants.as_ref().unwrap();
    assert_eq!(editor.current().unwrap().game_type(), ::variants::GameType::Ctf);
    assert!(editor.is_dirty());
}
