//! S5: an edited WGSL override reaches the fake game's `render.reload`, and
//! a revert takes it back (docs/editor/rich-editors.md, "Shaders").
use std::sync::{Arc, Mutex};

use super::*;
use crate::app::shader_override::{Key, State};

const GOOD: &str = "@vertex fn vs_main() -> @builtin(position) vec4f { return vec4f(0.0); }\n@fragment fn fs_main() -> @location(0) vec4f { return vec4f(1.0); }\n";
const PROGRAM: u64 = 0x00ab_cdef_0123_4567;

fn key() -> Key {
    Key { tag: map::TagId(0xe123_0045), pass: 0, implementation: 0 }
}

/// A game whose `render.reload` records its params and lists the program
/// while its override file exists in the directory it was told.
fn reload_game(seen: Arc<Mutex<Vec<Value>>>) -> impl FnOnce(&Game) {
    move |game: &Game| {
        game.agent.add_method("render.reload", agent::Access::Act, move |p| {
            seen.lock().unwrap().push(p.clone());
            let file = std::path::Path::new(p["dir"].as_str().unwrap())
                .join("00abcdef01234567_studio.wgsl");
            let loaded = if file.exists() { json!(["00abcdef01234567"]) } else { json!([]) };
            Ok(json!({"dir": p["dir"], "loaded": loaded, "changed": [], "problems": [], "generation": 1}))
        });
    }
}

#[test]
fn a_push_writes_the_file_and_tells_the_game_and_a_revert_takes_it_back() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (t, _game, mut app, mut play) =
        paired_with("shader-override", "example", SessionKind::Local, reload_game(seen.clone()));
    let dir = t.0.join("overrides");
    let file = dir.join("00abcdef01234567_studio.wgsl");
    app.shader_overrides.set_draft(key(), GOOD.into());
    app.shader_overrides.queue_push(key(), PROGRAM, &dir).unwrap();
    until(&mut play, &mut app, "the override loaded", |_, a| {
        a.shader_overrides.pushed(PROGRAM).is_some_and(|p| p.state == State::Loaded)
    });
    assert_eq!(std::fs::read_to_string(&file).unwrap(), GOOD);
    assert_eq!(seen.lock().unwrap()[0]["dir"], dir.display().to_string());

    app.shader_overrides.queue_revert(PROGRAM).unwrap();
    until(&mut play, &mut app, "the override reverted", |_, a| {
        a.shader_overrides.pushed(PROGRAM).is_none()
    });
    assert!(!file.exists());
    assert_eq!(seen.lock().unwrap().len(), 2);
}

/// With no game paired the push is refused by name and no file is written.
#[test]
fn a_push_with_no_game_is_refused_and_leaves_no_file() {
    let t = Temp::new("shader-override-unpaired");
    let mut app = editor(&t);
    let mut play = Play::new();
    let dir = t.0.join("overrides");
    app.shader_overrides.set_draft(key(), GOOD.into());
    app.shader_overrides.queue_push(key(), PROGRAM, &dir).unwrap();
    play.update(&mut app);
    let state = &app.shader_overrides.pushed(PROGRAM).unwrap().state;
    assert_eq!(state.why(), Some("pair with a running game first"));
    assert!(!dir.join("00abcdef01234567_studio.wgsl").exists());
}

/// A game without `render.reload` (no render-overrides feature) is named.
#[test]
fn a_game_without_render_reload_is_named() {
    let (t, _game, mut app, mut play) = paired("shader-override-missing", "example", SessionKind::Local);
    let dir = t.0.join("overrides");
    app.shader_overrides.set_draft(key(), GOOD.into());
    app.shader_overrides.queue_push(key(), PROGRAM, &dir).unwrap();
    until(&mut play, &mut app, "the refusal", |_, a| {
        a.shader_overrides.pushed(PROGRAM).is_some_and(|p| p.state.why().is_some())
    });
    let why = app.shader_overrides.pushed(PROGRAM).unwrap().state.why().unwrap().to_owned();
    assert!(why.contains("no render.reload"), "{why}");
    assert!(!dir.exists());
}
