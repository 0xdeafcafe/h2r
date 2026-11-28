//! The agent API over the scenario crate's invented map, the playlist
//! crate's sample file and the fallback variant schema (no game data).
//! Outputs go to the system temp directory, never the checkout.

use std::path::{Path, PathBuf};

use ::variants::save::{CONTENT_TYPES, SaveStore, XbeNames};
use scenario_edit::{BlockKind, fixture};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::*;
use crate::app::{Action, PlaylistEdit, Selection};

mod anim_save_tests;
mod bitmaps_tests;
mod blockout_tests;
mod game_tests;
mod playtest_tests;
mod image_gen_tests;
mod markers_tests;
mod script_variants_tests;
mod assets_tests;
mod links_tests;
mod forge_tests;
mod mods_tests;
mod packages_tests;
mod edit_target_tests;
mod doc_save_tests;
mod problems_tests;
mod import_tests;
mod sound_edit_tests;
mod shell_tests;
mod ui_export_tests;
mod shader_tests;
mod siege_tests;
mod new_map_tests;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app with the invented map open, a variant store, and the sample
/// playlist file at hand (not open).
fn app(t: &Temp) -> App {
    let map = t.path("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    std::fs::write(
        t.path("multiplayer_hopper.dat"),
        hopper_edit::fixture::sample().build(),
    )
    .unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let names = XbeNames {
        file_names: (0..CONTENT_TYPES).map(|i| format!("t{i}")).collect(),
        labels: (0..CONTENT_TYPES).map(|i| format!("L{i}")).collect(),
    };
    // The fallback schema (no disc) has no built-in variants: give it one.
    let mut schema = ::variants::Schema::fallback();
    schema.builtins.push(::variants::schema::Builtin {
        name: "Slayer".into(),
        game_type: ::variants::GameType::Slayer,
        description: 0,
        pairs: Vec::new(),
    });
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    // After the source: opening one replaces a variant editor made for
    // another (App::ensure_variants).
    app.set_variant_editor(::variants::VariantEditor::new(
        schema,
        Some(SaveStore::new(t.path("saves"), Some(names))),
    ));
    app
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

fn flag_position(app: &App) -> [f32; 3] {
    app.session
        .scenario()
        .unwrap()
        .get(BlockKind::NetgameFlags, 0, "position")
        .unwrap()
        .as_vec3()
        .unwrap()
}

/// The example params of `d` with the test's paths and values in.
fn example(d: &MethodDef, t: &Temp, app: &mut App) -> Value {
    let mut text = d.example.to_owned();
    let out = t.path("out");
    std::fs::create_dir_all(&out).unwrap();
    text = text.replace("$MAP", &t.path("example.map").display().to_string());
    if text.contains("$DISC") {
        let disc = t.path("disc");
        std::fs::create_dir_all(&disc).unwrap();
        std::fs::write(
            disc.join("example.map"),
            crate::app::workspace::tests::invented_map("example"),
        )
        .unwrap();
        text = text.replace("$DISC", &disc.display().to_string());
    }
    text = text.replace("$OUT", &out.display().to_string());
    if text.contains("$SCREEN") {
        let first = crate::app::UiScreens::tree()[0].0;
        text = text.replace("$SCREEN", first);
    }
    if text.contains("$SETTING") {
        // A setting of the open variant and another value for it.
        app.ensure_variants();
        let e = app.variants.as_ref().unwrap();
        let v = e.current().unwrap();
        // The fallback schema (no disc) knows no choices: any value goes,
        // so flip one between 0 and 1.
        let known: Vec<_> = v
            .known_settings()
            .into_iter()
            .filter(|(id, _)| ::variants::schema::applies(*id, v.game_type()))
            .collect();
        let (id, other) = known
            .iter()
            .find_map(|&(id, now)| {
                let d = e.schema().settings.get(&id)?;
                let other = d.choices.iter().find(|c| c.value != now)?.value;
                Some((id, other))
            })
            .or_else(|| {
                known
                    .iter()
                    .find(|(id, _)| {
                        e.schema()
                            .settings
                            .get(id)
                            .is_some_and(|d| d.choices.is_empty())
                    })
                    .map(|&(id, now)| (id, i32::from(now == 0)))
            })
            .expect("a setting the variant can change");
        text = text
            .replace("\"$SETTING\"", &id.0.to_string())
            .replace("\"$VALUE\"", &other.to_string());
    }
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: example {text}: {e}", d.name))
}

/// What a method's example needs before it runs.
fn setup(api: &mut Api, app: &mut App, t: &Temp, name: &str) {
    let mut c = |m: &str, p: Value| {
        call(api, app, m, p);
    };
    let new_variant = json!({"variant": {"new": "slayer"}, "discard": true});
    match name {
        m::DOC_SAVE => c(
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": t.path("out/first.map").display().to_string()}),
        ),
        // The layout runs between the two teams' CTF flag stands, which the
        // invented map has none of: the line map has them.
        m::SIEGE_LAYOUT => siege_tests::line_map(api, app, t),
        m::SCENARIO_ADD | m::SCENARIO_DUPLICATE | m::TAG_BLOCK_ADD | m::TAG_BLOCK_DUPLICATE => {
            // Every block of the invented map is full (the editor does
            // not grow blocks): make room in the starting locations, which
            // each example adds to.
            c(
                m::SCENARIO_DELETE,
                json!({"block": "starting_locations", "index": 1}),
            );
        }
        m::ASSET_LIST
        | m::ASSET_THUMB
        | m::ASSET_COPY
        | m::ASSET_LINKS
        | m::ASSET_WHERE_USED
        | m::ASSET_EDITS => {
            // An invented tags tree of two maps and a package `kit-test`.
            assets_tests::tree(&app.tags_root());
            assets_tests::package(&t.path("data/packages/kit-test"));
        }
        m::FORGE_VARIANT_LIST
        | m::FORGE_VARIANT_READ
        | m::FORGE_VARIANT_SET
        | m::FORGE_VARIANT_SAVE => {
            // The invented map variant `Castle` of the map `fixture`, open.
            forge_tests::open_castle(api, app);
        }
        m::FORGE_ITEMS => {
            // The invented tree and the package `kit-test`, which adds a weapon.
            assets_tests::tree(&app.tags_root());
            assets_tests::package(&t.path("data/packages/kit-test"));
        }
        m::ASSET_FORK | m::ASSET_REVERT => {
            // The invented tree and package `kit-test`, whose edit item
            // sets the rifle for every map; a revert has a fork to take out.
            assets_tests::tree(&app.tags_root());
            assets_tests::package(&t.path("data/packages/kit-test"));
            if name == m::ASSET_REVERT {
                call(
                    api,
                    app,
                    m::ASSET_FORK,
                    json!({"tag": "weap:objects\\weapons\\rifle", "map": "example", "package": "kit-test"}),
                );
            }
        }
        m::PACKAGE_EXPORT => c(m::PACKAGE_NEW, json!({"id": "export-example", "name": "Export example"})),
        n if n.starts_with("package.") => {
            // The invented package `kit-test`, and the file its example adds.
            let dir = t.path("data/packages/kit-test");
            assets_tests::package(&dir);
            std::fs::create_dir_all(dir.join("data")).unwrap();
            std::fs::write(dir.join("data/note.txt"), "a note\n").unwrap();
        }
        m::IMPORT_SOUND | m::ASSET_DROP => {
            // An empty local package `mine` and a WAV of an invented tone.
            import_tests::package(&t.path("data/packages/mine"), "mine");
            std::fs::create_dir_all(t.path("out")).unwrap();
            std::fs::write(t.path("out/clip.wav"), import_tests::tone(1, 22_050, 640)).unwrap();
        }
        m::SOUND_EDIT | m::SOUND_RENDER | m::SOUND_PERMUTATION => {
            // A package `mine` with the invented tone imported as a sound.
            import_tests::package(&t.path("data/packages/mine"), "mine");
            std::fs::create_dir_all(t.path("out")).unwrap();
            std::fs::write(t.path("out/clip.wav"), import_tests::tone(1, 22_050, 640)).unwrap();
            c(
                m::IMPORT_SOUND,
                json!({"file": t.path("out/clip.wav").display().to_string(), "package": "mine", "tag": "sounds\\clip"}),
            );
        }
        m::IMPORT_MODEL => {
            import_tests::package(&t.path("data/packages/mine"), "mine");
            import_tests::model_files(&t.path("out"));
        }
        m::IMPORT_BITMAP => {
            // An empty local package `mine` and a PNG of invented pixels.
            import_tests::package(&t.path("data/packages/mine"), "mine");
            std::fs::create_dir_all(t.path("out")).unwrap();
            std::fs::write(t.path("out/pic.png"), import_tests::png(3, 2)).unwrap();
        }
        m::SOUND_READ | m::SOUND_PLAY | m::SOUND_STOP | m::SOUND_MEASURE => {
            // A map with a sound, played to the headless output.
            let map = t.path("sound.map");
            std::fs::write(&map, super::sound::tests::sound_map()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            app.audition.headless = true;
        }
        m::SOUND_MOUTH => {
            // The same sound with a mouth curve on permutation 0.
            let map = t.path("mouth.map");
            std::fs::write(&map, super::sound::tests::mouth_map()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::FUNCTION_READ | m::FUNCTION_SET | m::FUNCTION_SAMPLE | m::FUNCTION_COLOURS => {
            // A map whose weapon tag has invented function fields.
            let map = t.path("curve.map");
            std::fs::write(&map, super::curves::tests::curve_map()).unwrap();
            app.tags = crate::app::TagDoc::new(super::curves::tests::registry());
            call(api, app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::SHADER_READ
        | m::SHADER_SET
        | m::SHADER_PREVIEW
        | m::SHADER_CODE
        | m::SHADER_OVERRIDE
        | m::SHADER_PUSH
        | m::SHADER_REVERT => {
            // The invented material: the floor shader with a colour its
            // template's one pass in layer 5 reads.
            let map = t.path("shader.map");
            std::fs::write(&map, shader_tests::material_map(fixture::LAYERED_LAYER)).unwrap();
            call(
                api,
                app,
                m::SOURCE_OPEN,
                json!({"path": map.display().to_string()}),
            );
            // A push sends a draft and a revert removes what one sent: the
            // linked program is the draft, pushed to a directory of the test's.
            if matches!(name, m::SHADER_PUSH | m::SHADER_REVERT) {
                let r = call(
                    api,
                    app,
                    m::SHADER_OVERRIDE,
                    json!({"tag": shader_tests::SHADER}),
                );
                let base = r["base"].as_str().expect("the pass links").to_owned();
                call(
                    api,
                    app,
                    m::SHADER_OVERRIDE,
                    json!({"tag": shader_tests::SHADER, "text": base}),
                );
            }
            if name == m::SHADER_REVERT {
                call(
                    api,
                    app,
                    m::SHADER_PUSH,
                    json!({
                        "tag": shader_tests::SHADER,
                        "program": "00abcdef01234567",
                        "dir": t.path("out/overrides").display().to_string(),
                    }),
                );
            }
        }
        m::TAG_COLOURS => {
            // A map whose weapon tag has an invented colour field of
            // every form.
            let map = t.path("colour.map");
            std::fs::write(&map, super::colours::tests::colour_map()).unwrap();
            app.tags = crate::app::TagDoc::new(super::colours::tests::registry());
            call(api, app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::VOICE_CHARACTERS | m::VOICE_DATASET_BUILD | m::VOICE_DATASET_STATUS => {
            // Invented maps and the fake sidecar (never Python).
            super::voice_clone::test_setup(app, false, false);
        }
        m::VOICE_CLONE_TRAIN
        | m::VOICE_CLONE_SAY
        | m::VOICE_SAY_BATCH
        | m::VOICE_BATCH_STATUS
        | m::VOICE_BATCH_RUN
        | m::VOICE_BATCH_SAVE => {
            // The fake sidecar installed and Cortana's dataset built.
            super::voice_clone::test_setup(app, true, true);
        }
        m::VOICE_SAY | m::VOICE_LIST => {
            // The fake voice (never the service), a headless output.
            app.audition.headless = true;
        }
        m::VOICE_COMMIT => {
            // A line to copy, and a checkout to copy it into.
            c(m::VOICE_SAY, json!({"text": "Superbounce", "play": false}));
            std::fs::create_dir_all(t.path("out/art/announcer")).unwrap();
        }
        m::VOLUME_LIST | m::VOLUME_SELECT | m::VOLUME_SET | m::VOLUME_REMOVE | m::VOLUME_ADD => {
            let map = t.path("volumes.map");
            std::fs::write(&map, fixture::fixture_with_volumes()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            if name == m::VOLUME_ADD {
                // Both blocks are full: make room for a kill volume.
                c(m::VOLUME_REMOVE, json!({"kind": "kill", "index": 0}));
                c(m::VOLUME_REMOVE, json!({"index": 1}));
            }
        }
        m::SCRIPT_LIST
        | m::SCRIPT_SOURCE
        | m::SCRIPT_FOLLOW
        | m::SCRIPT_USES
        | m::SCRIPT_BUILTINS
        | m::SCRIPT_DOC
        | m::SCRIPT_EXTERNAL
        | m::SCRIPT_COMPILE
        | m::SCRIPT_UNDO
        | m::SCRIPT_EVAL
        | m::RECORDING_LIST
        | m::RECORDING_READ
        | m::RECORDING_PLAY
        | m::RECORDING_WRITE
        | m::RECORDING_REMOVE
        | m::RECORDING_UNDO
        | m::SCRIPT_WORKING => {
            // The invented map with scripts and the invented executable.
            let dir = t.path("scripts");
            std::fs::create_dir_all(&dir).unwrap();
            let map = crate::app::scripts::tests::scripted_source(&dir, true);
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            if name == m::SCRIPT_WORKING {
                c(m::SCRIPT_EXTERNAL, json!({"launch": false}));
            }
        }
        m::COLLISION_SURFACES | m::COLLISION_AT | m::COLLISION_SELECT | m::COLLISION_OBJECTS => {
            let map = t.path("collision.map");
            let bytes = if name == m::COLLISION_OBJECTS {
                fixture::fixture_with_object_collision()
            } else {
                fixture::fixture_with_instance_collision()
            };
            std::fs::write(&map, bytes).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::GEOMETRY_COLLISION_ADD_BOX | m::GEOMETRY_COLLISION_CLEAR => {
            let map = t.path("level.map");
            std::fs::write(&map, level_collision_map()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::MODEL_PHYSICS_SHAPES | m::MODEL_PHYSICS_SET => {
            let map = t.path("physics.map");
            std::fs::write(&map, fixture::fixture_with_object_collision()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map}));
        }
        m::MODEL_COLLISION_FROM_RENDER => {
            let map = t.path("render-and-collision.map");
            std::fs::write(&map, fixture::fixture_with_render_and_collision()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            c(m::GEOMETRY_OPEN, json!({"target": "cluster:e1000004:0"}));
            c(m::GEOMETRY_OPEN, json!({"target": "coll_bsp:e100000a:0"}));
        }
        m::GEOMETRY_LIST
        | m::GEOMETRY_OPEN
        | m::GEOMETRY_CLOSE
        | m::GEOMETRY_READ
        | m::GEOMETRY_CHECK
        | m::GEOMETRY_LOWER_LODS
        | m::MESH_MODE
        | m::MESH_SELECT
        | m::MESH_TRANSFORM
        | m::MESH_OPS
        | m::MESH_APPLY
        | m::MESH_SCULPT => {
            // The invented textured map: one BSP (e1000004) whose one
            // cluster is a 4 by 4 floor quad in a local raw block. A stroke
            // rebuilds the structure's level, so `mesh.sculpt` gets the map
            // that has one.
            let map = t.path("textured.map");
            let bytes = if name == m::MESH_SCULPT {
                fixture::fixture_with_textured_level()
            } else {
                fixture::fixture_with_textured_bsp()
            };
            std::fs::write(&map, bytes).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            if !matches!(name, m::GEOMETRY_LIST | m::GEOMETRY_OPEN) {
                c(m::GEOMETRY_OPEN, json!({"target": "cluster:e1000004:0"}));
            }
            if name == m::MESH_TRANSFORM {
                c(m::MESH_SELECT, json!({"all": true}));
            }
        }
        m::BITMAP_LIST
        | m::BITMAP_READ
        | m::BITMAP_EXPORT
        | m::BITMAP_IMPORT
        | m::BITMAP_PAINT
        | m::BITMAP_FILL
        | m::BITMAP_ADJUST
        | m::BITMAP_PIXELS
        | m::BITMAP_RESHAPE
        | m::BITMAP_SPANS
        | m::BITMAP_IMAGE_ADD
        | m::BITMAP_IMAGE_RESIZE
        | m::BITMAP_SEQUENCE_ADD
        | m::BITMAP_SEQUENCE_SET
        | m::BITMAP_SPRITES
        | m::BITMAP_SPRITE_ADD
        | m::BITMAP_SPRITE_SET
        | m::BITMAP_SPRITE_REMOVE
        | m::BITMAP_CHANNEL_COPY
        | m::BITMAP_TURN => {
            // The invented textured map: its floor bitmap is 4 by 4,
            // one `A8R8G8B8` colour.
            let map = t.path("textured.map");
            std::fs::write(&map, fixture::fixture_with_textured_bsp()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            if name == m::BITMAP_IMPORT {
                let png = bitmap_edit::encode_png(2, 2, &[10, 20, 30, 255].repeat(4)).unwrap();
                std::fs::create_dir_all(t.path("out")).unwrap();
                std::fs::write(t.path("out/in.png"), png).unwrap();
            }
            if matches!(
                name,
                m::BITMAP_SEQUENCE_SET
                    | m::BITMAP_SPRITES
                    | m::BITMAP_SPRITE_ADD
                    | m::BITMAP_SPRITE_SET
                    | m::BITMAP_SPRITE_REMOVE
            ) {
                c(m::BITMAP_SEQUENCE_ADD, json!({"bitmap": "floor", "first": 0, "label": "icons"}));
            }
            if matches!(name, m::BITMAP_SPRITE_SET | m::BITMAP_SPRITE_REMOVE) {
                c(m::BITMAP_SPRITE_ADD, json!({"bitmap": "floor", "sequence": 0}));
            }
        }
        m::IMAGE_GENERATE | m::IMAGE_RESULT | m::IMAGE_APPLY | m::IMAGE_ACCEPT | m::IMAGE_DISCARD => {
            // The invented textured map's floor is the slot; the fake
            // source draws the picture (never the real generator).
            let map = t.path("textured.map");
            std::fs::write(&map, fixture::fixture_with_textured_bsp()).unwrap();
            call(api, app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            image_gen_tests::fake(app, t);
            if name != m::IMAGE_GENERATE {
                call(
                    api,
                    app,
                    m::IMAGE_GENERATE,
                    json!({"prompt": "a round blue badge", "references": [], "bitmap": "floor"}),
                );
                assert!(app.imagegen.wait(1, std::time::Duration::from_secs(20)));
            }
        }
        m::DOC_CLOSE | m::PLAYLIST_LIST | m::PLAYLIST_GET | m::PLAYLIST_SET | m::PLAYLIST_ENTRY => {
            c(
                m::DOC_OPEN,
                json!({"playlists": t.path("multiplayer_hopper.dat").display().to_string()}),
            )
        }
        m::PLAYLIST_ENTRY_VARIANT => {
            c(
                m::DOC_OPEN,
                json!({"playlists": t.path("multiplayer_hopper.dat").display().to_string()}),
            );
            c(m::DOC_OPEN, new_variant);
        }
        m::VARIANT_GET | m::VARIANT_SET | m::VARIANT_RENAME | m::VARIANT_EXPORT | m::VARIANT_MAPS => {
            c(m::DOC_OPEN, new_variant)
        }
        m::VARIANT_COPY => c(m::DOC_OPEN, json!({"variant": {"builtin": 0}})),
        m::VARIANT_DELETE => {
            c(m::DOC_OPEN, new_variant);
            app.dispatch(Action::SaveVariant);
        }
        m::VARIANT_IMPORT => {
            c(m::DOC_OPEN, new_variant);
            c(
                m::VARIANT_EXPORT,
                json!({"path": t.path("out/v.h2variant").display().to_string()}),
            );
        }
        m::UI_PRESS | m::UI_WIDGETS | m::UI_RENDER => {
            let first = crate::app::UiScreens::tree()[0].0;
            c(m::UI_PICK, json!({"screen": first}));
        }
        m::DOC_UNDO => c(
            m::SCENARIO_MOVE,
            json!({"block": "netgame_flags", "index": 0, "position": [1, 2, 3]}),
        ),
        m::DOC_REDO => {
            c(
                m::SCENARIO_MOVE,
                json!({"block": "netgame_flags", "index": 0, "position": [1, 2, 3]}),
            );
            c(m::DOC_UNDO, json!({"doc": "map"}));
        }
        // A sibling of the loose map stands in for the disc's shared.map.
        m::TAG_BROWSE => std::fs::write(t.path("shared.map"), fixture::fixture()).unwrap(),
        // An invented disc, its tree already extracted, so the example's
        // own run is the plan's no-op.
        m::WORKSPACE_EXTRACT => {
            let disc = t.path("disc");
            std::fs::create_dir_all(&disc).unwrap();
            std::fs::write(
                disc.join("example.map"),
                crate::app::workspace::tests::invented_map("example"),
            )
            .unwrap();
            c(m::WORKSPACE_EXTRACT, json!({"disc": disc.display().to_string()}));
            app.wait_workspace();
        }
        m::TAG_IMPORT => c(
            m::TAG_EXPORT,
            json!({"tag": "itmc", "out": t.path("out/kit.h2tag").display().to_string()}),
        ),
        m::UI_SELECT
        | m::UI_MOVE
        | m::UI_SET_RECT
        | m::UI_SET_TEXT
        | m::UI_REVERT
        | m::UI_UNDO
        | m::UI_REDO
        | m::UI_EDITS
        | m::UI_SAVE
        | m::UI_ANIMATE => {
            // Our example screen, its files saved into a temporary checkout.
            ui_edit_checkout(app, t);
            call(api, app, m::UI_PICK, json!({"screen": UI_EDIT_SCREEN}));
            if matches!(name, m::UI_UNDO | m::UI_REDO | m::UI_REVERT | m::UI_SAVE) {
                call(api, app, m::UI_SET_TEXT, json!({"id": "title", "text": "Edited"}));
            }
            if name == m::UI_REDO {
                call(api, app, m::UI_UNDO, json!({}));
            }
        }
        // A retail screen's tag edits: with no disc in a test there is no
        // retail screen, so their examples meet our example screen and
        // are refused with the owner.
        m::UI_HIDE | m::UI_DUPLICATE | m::UI_SET_STRING => {
            call(api, app, m::UI_PICK, json!({"screen": UI_EDIT_SCREEN}));
        }
        // The window's pairing, as its first frame publishes it.
        n if n.starts_with("pair.") => {
            crate::play::Play::new().update(app);
        }
        // The Pogo kit's invented map with the kit in it, as court.map.
        m::MODEL_LIST | m::MODEL_READ | m::MODEL_RENDER | m::MODEL_CHECK => {
            super::models::tests::open_court(app, &t.path("court.map"), &content_kit::fixture::kitted().bytes);
        }
        // The invented two-bone model and its graph, as pose.map; the
        // face preview's example poses its biped.
        m::ANIM_LIST | m::ANIM_POSE | m::SOUND_FACE => {
            let map = t.path("pose.map");
            std::fs::write(&map, super::anim::tests::anim_map(true)).unwrap();
            app.dispatch(Action::OpenSource(map));
        }
        // The invented two-bone model and graph, with a document to
        // retarget or a glTF file to import.
        m::ANIM_RETARGET | m::ANIM_IMPORT => {
            let map = t.path("pose.map");
            std::fs::write(&map, super::anim::tests::anim_map(true)).unwrap();
            app.dispatch(Action::OpenSource(map));
            if name == m::ANIM_IMPORT {
                let out = t.path("out");
                std::fs::create_dir_all(&out).unwrap();
                super::anim::transfer_tests::gltf(&out, ["body", "arm"], 1);
            } else {
                let opened = call(
                    api,
                    app,
                    m::ANIM_NEW,
                    json!({"tag": r"jmad:invented\stickman", "name": "wave", "frames": 30}),
                );
                assert_eq!(opened["doc"], 1, "the example's document");
            }
        }
        // The invented two-bone graph for the animation editor.
        m::ANIM_READ
        | m::ANIM_NEW
        | m::ANIM_DUPLICATE
        | m::ANIM_KEY
        | m::ANIM_CHECK
        | m::ANIM_SAVE => {
            let map = t.path("timeline.map");
            std::fs::write(&map, super::anim::read_tests::read_map()).unwrap();
            app.dispatch(Action::OpenSource(map));
            if name == m::ANIM_SAVE {
                import_tests::package(&t.path("data/packages/mine"), "mine");
            }
            if matches!(name, m::ANIM_KEY | m::ANIM_CHECK | m::ANIM_SAVE) {
                let opened = call(
                    api,
                    app,
                    m::ANIM_NEW,
                    json!({"tag": r"jmad:invented\stickman", "name": "wave", "frames": 30}),
                );
                assert_eq!(opened["doc"], 1, "the example's document");
            }
        }
        n if n.starts_with("hud.") => {
            // The HUD fixture's map as the source (it has no scenario).
            let map = t.path("hudtest.map");
            std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
            app.dispatch(Action::OpenSource(map));
            if n != m::HUD_LIST {
                call(api, app, m::HUD_LIST, json!({"map": "hudtest"}));
            }
        }
        _ => {}
    }
}

#[test]
fn ui_palette_lists_and_runs_fixture_maps_tags_and_placements() {
    let t = Temp::new("palette");
    let mut app = app(&t);
    let map = t.path("blaster.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    app.dispatch(Action::OpenSource(map));
    // The tag rows are asset.list's: a tree whose one tag is the blaster
    // weapon the fixture map also holds, in a manifest under the open
    // map's name, so the row opens on the map that is open.
    use ::tags::repo::store::Manifest;
    use ::tags::repo::{TagKey, TagRepo};
    let key = TagKey::new(map::FourCC(*b"weap"), r"objects\test\blaster");
    TagRepo::create(app.tags_root())
        .unwrap()
        .put_manifest(
            &Manifest {
                map: "blaster".into(),
                name: "blaster".into(),
                tags: vec![(key, None)],
                ..Default::default()
            },
            b"header",
        )
        .unwrap();
    let mut api = Api::new();

    let list = call(&mut api, &mut app, m::UI_PALETTE, json!({"query":"blast"}));
    let entries = list["entries"].as_array().unwrap();
    let kinds: Vec<_> = entries.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["map", "tag", "placement", "placement"]);
    assert_eq!(
        list["totals"],
        json!({"map":1, "tag":1, "package":0, "item":0, "voice":0, "placement":2, "command":0})
    );
    assert_eq!(entries[0]["label"], "blaster");
    // The tag row: the file name as its label, the path in its detail.
    assert_eq!(entries[1]["label"], "blaster");
    assert_eq!(entries[1]["detail"], "weap · objects\\test\\blaster");
    assert_eq!(entries[2]["label"], "0: blaster");

    let filtered = call(&mut api, &mut app, m::UI_PALETTE, json!({"query":"WEAP bla"}));
    assert_eq!(
        filtered["totals"],
        json!({"map":0, "tag":1, "package":0, "item":0, "voice":0, "placement":2, "command":0})
    );
    let result = call(
        &mut api, &mut app, m::UI_PALETTE, json!({"query":"blast", "run":1}),
    );
    assert_eq!(result["ran"], 1);
    assert_eq!(app.selection(), Some(Selection::Tag(fixture::id(2))));
    call(&mut api, &mut app, m::UI_PALETTE, json!({"query":"blast", "run":3}));
    assert_eq!(
        app.selection(),
        Some(Selection::Placement(BlockKind::Weapons, 1))
    );
    let bad_index = api.call(&mut app, m::UI_PALETTE, json!({"query":"blast", "run":4}));
    assert_eq!(code_of(bad_index), code::INVALID_PARAMS);
    assert_eq!(
        app.selection(),
        Some(Selection::Placement(BlockKind::Weapons, 1))
    );

    let result = call(
        &mut api,
        &mut app,
        m::UI_PALETTE,
        json!({"query":"growing full blocks", "run":0}),
    );
    assert_eq!(result["entries"][0]["kind"], "command");
    assert_eq!(result["ran"], 0);
    assert!(app.session.allow_growth());
}

#[test]
fn doc_save_rewrites_the_open_package() {
    let t = Temp::new("doc-save-package");
    let mut app = app(&t);
    let mut api = Api::new();
    let form = app.session.form_defaults();
    app.save_map_package(&form);
    call(
        &mut api, &mut app, m::SCENARIO_MOVE,
        json!({"block":"netgame_flags", "index":0, "position":[7, 3, 3]}),
    );
    let path = app.session.maps_dir().join("example-edit");
    let result = call(&mut api, &mut app, m::DOC_SAVE, json!({"doc":"map"}));
    assert_eq!(result["saved"], true);
    assert_eq!(result["path"], path.display().to_string());
    assert!(!app.is_dirty());
    app.dispatch(Action::SelectMap("example".into()));
    app.dispatch(Action::OpenPackageEntry("example-edit".into()));
    assert_eq!(flag_position(&app), [7.0, 3.0, 3.0]);
}

#[test]
fn tag_fields_describe_root_and_nested_registry_fields() {
    let t = Temp::new("field-rows");
    let mut bytes = fixture::fixture();
    // One collection entry in the fixture's ordinary metadata.
    let root = fixture::META_FILE + 0x1200;
    fixture::put(&mut bytes, root, 1);
    fixture::put(&mut bytes, root + 4, fixture::META + 0x1240);
    fixture::putf(&mut bytes, fixture::META_FILE + 0x1240, 1.5);
    let source = t.path("rows.map");
    std::fs::write(&source, bytes).unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    app.session.build_scenes = false;
    app.dispatch(Action::OpenSource(source));
    let mut api = Api::new();

    let root = call(&mut api, &mut app, m::TAG_FIELDS, json!({"tag":"itmc"}));
    let rows = root["fields"].as_array().unwrap();
    let respawn = rows.iter().find(|f| f["name"] == "respawn_time").unwrap();
    assert_eq!(respawn["offset"], 8);
    assert_eq!(respawn["size"], 2);
    assert_eq!(respawn["kind"], "i16");
    assert_eq!(
        respawn["description"],
        "Seconds before the item respawns when its placement has no override."
    );
    // The panel's row model: words for people, the path for tag.set, and
    // no effect chip with no game connected.
    assert_eq!(respawn["label"], "Respawn time");
    assert_eq!(respawn["type"], "Whole number");
    assert_eq!(respawn["path"], "respawn_time");
    assert!(respawn["view"].is_null(), "a field with no view= word");
    assert_eq!(respawn["named"], true);
    assert_eq!(respawn["edited"], false);
    assert!(respawn["file_value"].is_null());
    assert!(respawn["chip"].is_null());
    assert!(respawn["live"].is_null());
    assert_eq!(respawn["effect"]["id"], "not_known");
    assert_eq!(respawn["effect"]["text"], "Not known; reload to be sure.");
    let entries = rows.iter().find(|f| f["name"] == "entries").unwrap();
    assert_eq!(entries["value"], 1);
    assert_eq!((entries["count"].as_u64(), entries["type"].as_str()), (Some(1), Some("List")));
    assert!(entries["summary"].as_str().unwrap().starts_with("1 item"));
    assert!(entries.get("description").unwrap().is_null());

    let nested = call(
        &mut api,
        &mut app,
        m::TAG_FIELDS,
        json!({"tag":"itmc", "path":"entries[0]"}),
    );
    let weight = &nested["fields"][0];
    assert_eq!(weight["name"], "weight");
    assert_eq!(weight["value"], 1.5);
    assert_eq!(weight["path"], "entries[0].weight");
    let via_offset = call(
        &mut api,
        &mut app,
        m::TAG_FIELDS,
        json!({"tag":"itmc", "path":"+0x0[0]"}),
    );
    assert_eq!(nested, via_offset);
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::TAG_FIELDS,
            json!({"tag":"itmc", "path":"entries[1]"}),
        )),
        code::REFUSED
    );
}

/// `tag.choices` lists what the pick dialog lists: an enum's named
/// values, every bit of switches with what is on, a reference's tags (no
/// cap unless asked, `kind` and `query` narrowing them) and an unnamed
/// value's views. A field with nothing to pick is refused.
#[test]
fn tag_choices_lists_the_pick_dialogs_choices() {
    use crate::app::tag_tests;
    let t = Temp::new("tag-choices");
    let (mut app, ..) = tag_tests::app(&t.0);
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);
    let mut choices = |p: Value| call(&mut api, &mut app, m::TAG_CHOICES, p);

    let mode = choices(json!({"tag": weap, "path": "mode"}));
    assert_eq!(
        mode["values"],
        json!([{"value": 0, "name": "Off"}, {"value": 1, "name": "On"}, {"value": 2, "name": "Auto"}])
    );
    let narrowed = choices(json!({"tag": weap, "path": "mode", "query": "au"}));
    assert_eq!(narrowed["values"], json!([{"value": 2, "name": "Auto"}]));

    let flags = choices(json!({"tag": weap, "path": "flags"}));
    let bits = flags["bits"].as_array().unwrap();
    assert_eq!(bits.len(), 16, "every bit of the word");
    assert_eq!(bits[0], json!({"bit": 0, "name": "loud", "label": "Loud", "on": true}));
    assert_eq!(bits[1]["on"], false);
    assert_eq!(bits[5], json!({"bit": 5, "name": null, "label": "Bit 5", "on": false}));

    let item = choices(json!({"tag": weap, "path": "parts[0].item"}));
    assert_eq!(item["kinds"], json!([{"group": "itmc", "name": "itmc"}]));
    assert_eq!(item["tags"][0]["name"], "objects\\test\\kit");
    assert_eq!(item["tags"].as_array().unwrap().len(), 1);
    assert_eq!(item["truncated"], false);
    let none = choices(json!({"tag": weap, "path": "parts[0].item", "limit": 0}));
    assert_eq!((none["tags"].as_array().unwrap().len(), none["truncated"].as_bool()), (0, Some(true)));
    let missed = choices(json!({"tag": weap, "path": "parts[0].item", "query": "blaster"}));
    assert!(missed["tags"].as_array().unwrap().is_empty());
    let other = choices(json!({"tag": weap, "path": "parts[0].item", "kind": "weap"}));
    assert!(other["tags"].as_array().unwrap().is_empty());

    assert_eq!(
        code_of(api.call(&mut app, m::TAG_CHOICES, json!({"tag": weap, "path": "speed"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_CHOICES, json!({"tag": weap, "path": "parts[0].item", "kind": "toolong"}))),
        code::INVALID_PARAMS
    );

    let t = Temp::new("tag-choices-partial");
    let (mut app, ..) = tag_tests::app_partial(&t.0);
    let gap = call(&mut api, &mut app, m::TAG_CHOICES, json!({"tag": weap, "path": "+0x6"}));
    assert_eq!(
        gap["views"],
        json!([{"id": "whole_number", "name": "Whole number"}, {"id": "hex", "name": "Hex"}])
    );
    assert_eq!(gap["shown"], "whole_number");
}

/// `tag.bytes` gives an element's bytes with spans that tile it, or one
/// field's; `tag.reset` puts a field back to the file's value as one undo
/// step and clears `edited`.
#[test]
fn tag_bytes_tile_the_element_and_tag_reset_is_one_undo_step() {
    use crate::app::tag_tests;
    let t = Temp::new("tag-bytes");
    let (mut app, ..) = tag_tests::app(&t.0);
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);
    let tiles = |v: &Value| {
        let mut next = 0;
        for s in v["spans"].as_array().unwrap() {
            assert_eq!(s["offset"], next, "{v}");
            next += s["size"].as_u64().unwrap();
        }
        assert_eq!(json!(next), v["size"], "{v}");
        assert_eq!(v["hex"].as_str().unwrap().len() as u64, 2 * next);
    };

    let root = call(&mut api, &mut app, m::TAG_BYTES, json!({ "tag": weap }));
    assert_eq!(root["address"], tag_tests::WEAP);
    assert_eq!(root["size"], 0x10);
    tiles(&root);
    assert_eq!(root["hex"], root["file_hex"], "nothing changed");
    assert!(root["hex"].as_str().unwrap().starts_with("00002040"), "speed 2.5");
    let part = call(&mut api, &mut app, m::TAG_BYTES, json!({"tag": weap, "path": "parts[1]"}));
    assert_eq!(part["address"], tag_tests::PARTS + 0xc);
    tiles(&part);
    let mode = call(&mut api, &mut app, m::TAG_BYTES, json!({"tag": weap, "path": "mode"}));
    assert_eq!(mode["hex"], "0200");
    assert_eq!(
        mode["spans"],
        json!([{"offset": 0, "size": 2, "name": "mode", "named": true}])
    );
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_BYTES, json!({"tag": weap, "path": "parts[5]"}))),
        code::INVALID_PARAMS
    );

    let speed = |api: &mut Api, app: &mut App| {
        let rows = call(api, app, m::TAG_FIELDS, json!({ "tag": weap }));
        rows["fields"][0].clone()
    };
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": weap, "path": "speed", "value": 4.0}));
    assert_eq!(speed(&mut api, &mut app)["edited"], true);
    let changed = call(&mut api, &mut app, m::TAG_BYTES, json!({"tag": weap, "path": "speed"}));
    assert_ne!(changed["hex"], changed["file_hex"]);
    let back = call(&mut api, &mut app, m::TAG_RESET, json!({"tag": weap, "path": "speed"}));
    assert_eq!(back["value"], 2.5);
    let row = speed(&mut api, &mut app);
    assert_eq!((row["edited"].as_bool(), row["value"].as_f64()), (Some(false), Some(2.5)));
    app.dispatch(Action::Undo);
    assert_eq!(speed(&mut api, &mut app)["value"], 4.0, "the reset is one undo step");
    app.dispatch(Action::Undo);
    assert_eq!(speed(&mut api, &mut app)["value"], 2.5);
    // Already the file's value: no step.
    call(&mut api, &mut app, m::TAG_RESET, json!({"tag": weap, "path": "speed"}));
    app.dispatch(Action::Redo);
    assert_eq!(speed(&mut api, &mut app)["value"], 4.0, "the redo stack is intact");
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_RESET, json!({"tag": weap, "path": "parts"}))),
        code::INVALID_PARAMS
    );
}

/// `tag.elements` titles a list's elements by the block's `title=` field,
/// else by the first named name or tag reference, else not at all; the
/// filter and paging follow the list's rows. `tag.view path` shows an
/// element alone with its breadcrumbs, and the navigation carries it.
#[test]
fn tag_elements_are_titled_and_an_element_opens_alone() {
    use crate::app::tag_tests;
    let t = Temp::new("tag-elements");
    let (mut app, ..) = tag_tests::app(&t.0);
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);
    let titles = |v: &Value| -> Vec<Value> {
        v["elements"].as_array().unwrap().iter().map(|e| e["title"].clone()).collect()
    };

    let parts = call(&mut api, &mut app, m::TAG_ELEMENTS, json!({"tag": weap, "path": "parts"}));
    assert_eq!((parts["count"].as_u64(), parts["truncated"].as_bool()), (Some(2), Some(false)));
    // The first named tag reference titles an element; an empty one leaves
    // the index alone.
    assert_eq!(titles(&parts), [json!("kit"), Value::Null]);
    assert_eq!(parts["elements"][1]["path"], "parts[1]");
    let kit = call(
        &mut api,
        &mut app,
        m::TAG_ELEMENTS,
        json!({"tag": weap, "path": "parts", "filter": "KIT"}),
    );
    assert_eq!(kit["elements"].as_array().unwrap().len(), 1);
    let paged = call(
        &mut api,
        &mut app,
        m::TAG_ELEMENTS,
        json!({"tag": weap, "path": "+0x8", "offset": 1, "limit": 1}),
    );
    assert_eq!(paged["elements"][0]["index"], 1);
    assert_eq!(paged["truncated"], false);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_ELEMENTS, json!({"tag": weap, "path": "speed"}))),
        code::INVALID_PARAMS
    );

    // Open alone, as the panel's button does.
    let alone = call(&mut api, &mut app, m::TAG_VIEW, json!({"tag": weap, "path": "parts[1]"}));
    assert_eq!(alone["path"], "parts[1]");
    // No tag view is registered for a weapon: Fields is its only tab, and
    // another tab is refused by name.
    assert_eq!(alone["tabs"], json!([{"id": "fields", "label": "Fields"}]));
    assert_eq!(alone["tab"], "fields");
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_VIEW, json!({"tab": "sound"}))),
        code::REFUSED
    );
    assert_eq!(alone["breadcrumbs"], json!(["weap: objects\\test\\blaster", "Parts 1"]));
    let paths: Vec<&str> = alone["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(paths, ["parts[1].item", "parts[1].value"]);
    let status = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    assert_eq!(status["element"], "parts[1]");
    let root = call(&mut api, &mut app, m::TAG_VIEW, json!({"path": ""}));
    assert_eq!(root["path"], "");
    call(&mut api, &mut app, m::NAV_BACK, json!({}));
    let back = call(&mut api, &mut app, m::TAG_VIEW, json!({}));
    assert_eq!(back["path"], "parts[1]", "Back restores it");
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_VIEW, json!({"path": "parts[2]"}))),
        code::INVALID_PARAMS
    );
    let via_nav = call(
        &mut api,
        &mut app,
        m::NAV_GO,
        json!({"tag": weap, "element": "+0x8[0]"}),
    );
    assert_eq!(via_nav["element"], "parts[0]", "an offset path is kept by names");
    assert_eq!(app.tool, crate::ToolKind::Tags);

    // A block's title= field titles its elements.
    let t = Temp::new("tag-elements-titled");
    let (mut app, ..) = tag_tests::app_titled(&t.0);
    let titled = call(&mut api, &mut app, m::TAG_ELEMENTS, json!({"tag": weap, "path": "parts"}));
    assert_eq!(titles(&titled), [json!("7"), json!("9")]);
    let view = call(&mut api, &mut app, m::TAG_VIEW, json!({"tag": weap, "path": "parts[0]"}));
    assert_eq!(view["breadcrumbs"][1], "Parts 0  7");
    assert_eq!(view["header"][0], "Weapon  blaster");
}

#[test]
fn tag_view_folds_unnamed_fields_and_opens_them_on_request() {
    use crate::app::tag_tests;
    let t = Temp::new("tag-view");
    let (mut app, ..) = tag_tests::app_partial(&t.0);
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);

    let folded = call(&mut api, &mut app, m::TAG_VIEW, json!({ "tag": weap }));
    assert_eq!(app.tool, crate::ToolKind::Tags);
    assert_eq!(app.shown_tags().selected, Some(tag_tests::weap_id()));
    assert_eq!(folded["unnamed"], false);
    assert_eq!(folded["unnamed_hidden"], 1);
    let paths = |v: &Value| -> Vec<String> {
        v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["path"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(paths(&folded), ["speed", "flags", "parts"]);
    assert!(folded["header"][0].as_str().unwrap().starts_with("weap"));

    let open = call(&mut api, &mut app, m::TAG_VIEW, json!({"unnamed": true}));
    assert!(app.tag_fields.unnamed);
    assert_eq!(open["unnamed_hidden"], 0);
    assert_eq!(
        open["rows"].as_array().unwrap().len(),
        folded["rows"].as_array().unwrap().len() + 1,
        "the toggle shows the hidden rows"
    );
    let gap = &open["rows"][3];
    assert_eq!((gap["label"].as_str(), gap["path"].as_str()), (Some("Value at +0x6"), Some("+0x6")));
    assert_eq!(gap["show_as"], "whole_number");
    // tag.fields gives every row in the panel's order.
    let every = call(&mut api, &mut app, m::TAG_FIELDS, json!({ "tag": weap }));
    assert_eq!(every["fields"], open["rows"]);
    let named = call(&mut api, &mut app, m::TAG_FIELDS, json!({ "tag": weap, "unnamed": false }));
    assert_eq!(named["fields"].as_array().unwrap().len(), 3);

    let hex = call(
        &mut api,
        &mut app,
        m::TAG_VIEW,
        json!({"show_as": {"path": "+0x6", "as": "hex"}}),
    );
    assert_eq!(hex["rows"][3]["show_as"], "hex");
    assert_eq!(app.tag_fields.show_as.get("weap:+0x6"), Some(&crate::fields::View::Hex));
    let guessed = call(
        &mut api,
        &mut app,
        m::TAG_VIEW,
        json!({"show_as": {"path": "+0x6", "as": null}}),
    );
    assert_eq!(guessed["rows"][3]["show_as"], "whole_number");
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_VIEW, json!({"show_as": {"path": "speed", "as": "hex"}}))),
        code::INVALID_PARAMS
    );

    // Focus on an unnamed field opens its group with the toggle off.
    call(&mut api, &mut app, m::TAG_VIEW, json!({"unnamed": false}));
    let focused = call(&mut api, &mut app, m::TAG_VIEW, json!({"field": "+0x6"}));
    assert_eq!(focused["field"], "+0x6");
    assert_eq!(focused["unnamed_hidden"], 0);
    // A changed unnamed field opens it too.
    call(&mut api, &mut app, m::TAG_VIEW, json!({"field": "speed"}));
    assert_eq!(call(&mut api, &mut app, m::TAG_VIEW, json!({}))["unnamed_hidden"], 1);
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": weap, "path": "+0x6", "value": 5}));
    let changed = call(&mut api, &mut app, m::TAG_VIEW, json!({}));
    assert_eq!(changed["unnamed_hidden"], 0);
    assert_eq!(changed["rows"][3]["edited"], true);
    // The filter row's search drives the hit list; an empty one clears it.
    let searched = call(&mut api, &mut app, m::TAG_VIEW, json!({"search": "speed"}));
    assert_eq!(searched["search"], "speed");
    let hit_paths: Vec<&str> = searched["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(hit_paths, ["speed"]);
    let back = call(&mut api, &mut app, m::TAG_VIEW, json!({"search": ""}));
    assert_eq!(back["rows"].as_array().unwrap().len(), 4, "the changed unnamed row is shown");

    // A tag nothing names shows its group open: every field is reachable.
    let scenario = format!("{:#010x}", fixture::id(1).0);
    let unnamed = call(&mut api, &mut app, m::TAG_VIEW, json!({ "tag": scenario }));
    assert_eq!(unnamed["unnamed_hidden"], 0);
    let rows = unnamed["rows"].as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r["named"] == false && r["editable"] == false));
}

/// `tag.find` matches the label, the name, the description, the type
/// words, choice and switch names and the referenced tag's row text, and
/// `changed` lists the fields that differ from the file — gone after the
/// edit's undo. `tag.view`'s `search` and `changed` drive the same hit
/// list, and `field` clears it.
#[test]
fn tag_find_matches_words_and_changed_follows_edits() {
    use crate::app::tag_tests;
    let t = Temp::new("tag-find");
    let (mut app, ..) = tag_tests::app_described(&t.0);
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);
    let find = |api: &mut Api, app: &mut App, query: Option<&str>, path: Option<&str>, changed: bool| {
        let mut p = json!({ "tag": weap, "changed": changed });
        if let Some(q) = query {
            p["query"] = json!(q);
        }
        if let Some(e) = path {
            p["path"] = json!(e);
        }
        call(api, app, m::TAG_FIND, p)
    };
    let paths = |v: &Value| -> Vec<String> {
        v["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["path"].as_str().unwrap().to_owned())
            .collect()
    };

    // The label (and the name); the description; a choice name; a switch
    // name; the referenced tag's row text; the type words. Words may sit
    // in different places, but every word must match somewhere.
    assert_eq!(paths(&find(&mut api, &mut app, Some("speed"), None, false)), ["speed"]);
    assert_eq!(
        find(&mut api, &mut app, Some("speed"), None, false)["hits"][0]["matched"],
        "Speed"
    );
    assert_eq!(paths(&find(&mut api, &mut app, Some("repeats"), None, false)), ["speed"]);
    assert_eq!(
        find(&mut api, &mut app, Some("repeats"), None, false)["hits"][0]["matched"],
        "How fast the trigger repeats."
    );
    assert_eq!(paths(&find(&mut api, &mut app, Some("auto"), None, false)), ["mode"]);
    assert_eq!(paths(&find(&mut api, &mut app, Some("QUIET"), None, false)), ["flags"]);
    assert_eq!(paths(&find(&mut api, &mut app, Some("kit"), None, false)), ["parts[0].item"]);
    assert_eq!(
        paths(&find(&mut api, &mut app, Some("decimal number"), None, false)),
        ["speed"]
    );
    assert_eq!(paths(&find(&mut api, &mut app, Some("fast trigger"), None, false)), ["speed"]);
    assert!(paths(&find(&mut api, &mut app, Some("speed kit"), None, false)).is_empty());
    // A hit carries its crumbs and its row.
    let hit = &find(&mut api, &mut app, Some("kit"), None, false)["hits"][0];
    assert_eq!(hit["breadcrumbs"], json!(["weap: objects\\test\\blaster", "Parts 0  kit"]));
    assert_eq!(hit["label"], "Item");
    assert_eq!(hit["type"], "Tag reference");
    assert_eq!(hit["matched"], "itmc  kit");
    // Searching within an element.
    let inside = find(&mut api, &mut app, Some("value"), Some("parts[0]"), false);
    assert_eq!(paths(&inside), ["parts[0].value"]);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_FIND, json!({"tag": weap, "path": "parts[2]"}))),
        code::INVALID_PARAMS
    );

    // Changed only: nothing, the edit, then nothing again after its undo.
    assert!(paths(&find(&mut api, &mut app, None, None, true)).is_empty());
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": weap, "path": "speed", "value": 4.0}));
    assert_eq!(paths(&find(&mut api, &mut app, None, None, true)), ["speed"]);
    // Both filters together intersect.
    assert_eq!(paths(&find(&mut api, &mut app, Some("speed"), None, true)), ["speed"]);
    assert!(paths(&find(&mut api, &mut app, Some("flags"), None, true)).is_empty());
    // The panel shows the same hit list.
    let view = call(&mut api, &mut app, m::TAG_VIEW, json!({"tag": weap, "changed": true}));
    assert_eq!(view["changed"], true);
    let rows: Vec<&str> = view["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["path"].as_str().unwrap())
        .collect();
    assert_eq!(rows, ["speed"]);
    // Focusing a field clears the filters: its rows are shown.
    let focused = call(&mut api, &mut app, m::TAG_VIEW, json!({"field": "speed"}));
    assert_eq!((focused["search"].as_str(), focused["changed"].as_bool()), (Some(""), Some(false)));
    assert!(focused["rows"].as_array().unwrap().iter().any(|r| r["path"] == "flags"));
    app.dispatch(Action::Undo);
    assert!(paths(&find(&mut api, &mut app, None, None, true)).is_empty());
}

/// The search walk stops at 5,000 list items and says so: `tag.find`
/// answers `truncated` with the sentence, and `tag.view`'s `header`
/// carries it over the cut rows.
#[test]
fn the_field_search_stops_at_its_budget() {    use crate::app::tag_tests;
    use scenario_edit::fixture::{META, META_FILE, put};
    let t = Temp::new("tag-find-budget");
    // The weapon's parts grown to 6,000 elements of invented data: the
    // metadata region grows with them.
    let mut src = tag_tests::map();
    let w = META_FILE + 0x1100;
    let size = 0x2000 + 0x20000;
    src.resize(META_FILE + size, 0);
    put(&mut src, 0x8, (META_FILE + size) as u32);
    put(&mut src, 0x18, size as u32);
    put(&mut src, w + 8, 6_000);
    put(&mut src, w + 0xc, META + 0x2000);
    let (mut app, ..) = tag_tests::app_with_registry(&t.0, src, tag_tests::registry());
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);

    let found = call(
        &mut api,
        &mut app,
        m::TAG_FIND,
        json!({"tag": weap, "query": "value", "limit": 10}),
    );
    assert_eq!(found["hits"].as_array().unwrap().len(), 10);
    assert_eq!(found["hits"][0]["path"], "parts[0].value");
    assert_eq!(found["truncated"], true);
    assert_eq!(
        found["why"],
        "Search stopped after 5,000 list items. Open a list and search there."
    );
    // The panel's rows are the walk's hits, and the header carries the
    // budget's sentence.
    let view = call(&mut api, &mut app, m::TAG_VIEW, json!({"tag": weap, "search": "value"}));
    assert_eq!(view["rows"].as_array().unwrap().len(), 5_000);
    assert_eq!(
        view["header"].as_array().unwrap().last(),
        Some(&json!("Search stopped after 5,000 list items. Open a list and search there."))
    );
}

/// `tag.uses` answers from the reverse index: the two placements on one
/// palette entry are both listed (and `highlight` outlines them), the
/// item collection is reached through the weapon and placed by the
/// netgame item, and the index rebuilds after a block edit and after a
/// reference edit. `tag.chain`'s nodes count their uses.
#[test]
fn tag_uses_lists_the_placements_and_tags_that_reach_a_tag() {
    use crate::app::tag_tests;
    let t = Temp::new("tag-uses");
    let (mut app, ..) = tag_tests::app(&t.0);
    let mut api = Api::new();
    let weap = format!("{:#010x}", tag_tests::weap_id().0);
    let itmc = format!("{:#010x}", fixture::id(3).0);
    let blocks = |v: &Value| -> Vec<(String, usize)> {
        v["placements"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["block"].as_str().unwrap().to_owned(), p["index"].as_u64().unwrap() as usize))
            .collect()
    };
    let tags = |v: &Value| -> Vec<String> {
        v["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u["tag"].as_str().unwrap().to_owned())
            .collect()
    };

    // Two placements share one palette entry: both listed, and nothing
    // refers to the weapon itself (the scenario's reference is the
    // placements', not a tag's).
    let weapon = call(&mut api, &mut app, m::TAG_USES, json!({ "tag": weap }));
    assert_eq!(blocks(&weapon), [("weapons".to_owned(), 0), ("weapons".to_owned(), 1)]);
    assert!(tags(&weapon).is_empty());
    assert_eq!(weapon["objects"], json!([]));

    // The item collection is referred to by the weapon's first part and
    // placed by the netgame item; the placements whose palette chain
    // runs through the weapon are named too.
    let kit = call(&mut api, &mut app, m::TAG_USES, json!({ "tag": itmc }));
    assert_eq!(tags(&kit), std::slice::from_ref(&weap));
    assert_eq!(kit["tags"][0]["via"], "parts[0].item");
    assert_eq!(
        blocks(&kit),
        [
            ("netgame_equipment".to_owned(), 0),
            ("weapons".to_owned(), 0),
            ("weapons".to_owned(), 1),
        ]
    );

    // highlight outlines the placements; the next highlight clears them.
    call(&mut api, &mut app, m::TAG_USES, json!({ "tag": weap, "highlight": true }));
    assert_eq!(app.highlighted.len(), 2);
    call(&mut api, &mut app, m::TAG_USES, json!({ "tag": weap, "highlight": false }));
    assert!(app.highlighted.is_empty());

    // The chain's nodes carry the count.
    let chain = call(&mut api, &mut app, m::TAG_CHAIN, json!({ "block": "weapons", "index": 0 }));
    assert_eq!(
        chain["root"]["uses"],
        json!({ "placements": 2, "tags": 0 }),
        "the weapon node counts its two placements"
    );

    // The index rebuilds after a block edit (a placement removed) and
    // after a reference edit (the part no longer names the collection);
    // undo restores both.
    call(&mut api, &mut app, m::SCENARIO_DELETE, json!({ "block": "weapons", "index": 1 }));
    let after = call(&mut api, &mut app, m::TAG_USES, json!({ "tag": weap }));
    assert_eq!(blocks(&after), [("weapons".to_owned(), 0)]);
    call(&mut api, &mut app, m::TAG_SET, json!({ "tag": weap, "path": "parts[0].item", "value": null }));
    let cut = call(&mut api, &mut app, m::TAG_USES, json!({ "tag": itmc }));
    assert!(tags(&cut).is_empty());
    assert_eq!(blocks(&cut), [("netgame_equipment".to_owned(), 0)]);
    app.dispatch(Action::Undo);
    let back = call(&mut api, &mut app, m::TAG_USES, json!({ "tag": itmc }));
    assert_eq!(tags(&back), std::slice::from_ref(&weap));
}

#[test]
fn the_camera_api_reads_and_sets_fly_speed() {
    let temp = Temp::new("fly-speed");
    let mut app = app(&temp);
    let mut api = Api::new();
    let initial = api.call(&mut app, m::VIEW_CAMERA, json!({})).unwrap();
    assert_eq!(initial["fly_speed"], json!(crate::camera::DEFAULT_SPEED));
    let response = api.call(&mut app, m::VIEW_CAMERA, json!({"fly_speed": 32})).unwrap();
    assert_eq!(response["fly_speed"], json!(32.0));
    let error = api.call(&mut app, m::VIEW_CAMERA, json!({"fly_speed": 0})).unwrap_err();
    assert_eq!(error.code, code::INVALID_PARAMS);
    assert_eq!(app.fly_speed(), 32.0);
}

#[test]
fn every_method_in_methods_has_a_test() {
    // Each method runs its own example from the table on a fresh app:
    // built ones answer, planned ones answer NOT_YET with their lane.
    let mut ran = Vec::new();
    for d in Api::new().methods() {
        let t = Temp::new("each");
        let mut app = app(&t);
        let mut api = Api::new();
        setup(&mut api, &mut app, &t, d.name);
        let params = example(&d, &t, &mut app);
        let r = api.call(&mut app, d.name, params.clone());
        match (d.planned, r) {
            (Some(lane), Err(e)) => {
                assert_eq!(e.code, code::NOT_YET, "{}: {e}", d.name);
                assert_eq!(e.data.unwrap()["lane"], lane);
            }
            (Some(_), Ok(v)) => panic!("{} is planned but answered {v}", d.name),
            // No renderer on this machine: the snapshot says so.
            (None, Err(e))
                if matches!(d.name, m::VIEW_SNAPSHOT | m::MODEL_RENDER | m::MARKERS_RENDER | m::BLOCKOUT_RENDER) && e.code == code::NO_GPU => {}
            // These reads need an active watchdog, recorded samples or sampling.
            (None, Err(e))
                if matches!(d.name, mem::HISTORY | mem::HEAP | mem::DIFF | mem::POSTMORTEM)
                    && e.code == agent_proto::code::UNAVAILABLE => {}
            // The first frame has no paired or editor-launched game.
            (None, Err(e))
                if matches!(
                    d.name,
                    m::PAIR_STOP
                        | m::PAIR_EDIT
                        | m::PAIR_CONTROL
                        | m::MIRROR_APPLY
                        | m::MIRROR_PLAYER
                        | m::MIRROR_CONTROL
                        | m::PAIR_PREVIEW
                        | m::PAIR_AUDIO
                        | m::PAIR_RESULT
                        // No game, so no picture to save.
                        | m::GAME_SNAPSHOT
                        | m::GAME_START
                        | m::GAME_STOP
                        | m::GAME_MODE
                        | m::PACKAGE_PLAY
                        | m::RECORDING_PLAY
                        | m::SCRIPT_EVAL
                        // No sim runner is wired to Studio's own API.
                        | m::PLAYTEST_RUN
                ) && e.code == code::REFUSED => {}
            // No edit target is set here; app/ground_tests.rs sets one.
            (None, Err(e)) if d.name == m::GROUND_SAVE && e.code == code::NO_TARGET => {}
            // No tag tree under the test's data root: the build and the
            // edit check name it (api/projects.rs's own tests make one).
            (None, Err(e))
                if matches!(d.name, m::PROJECT_BUILD | m::EDITS_CHECK)
                    && e.code == code::FAILED
                    && e.message.contains("no tag tree") => {}
            // The invented map has no hill with corners to convert
            // (markers_tests.rs makes one).
            (None, Err(e)) if d.name == m::MARKERS_AREA_CONVERT && e.code == code::REFUSED => {}
            // The invented map has no render model: its example names a
            // model section that is not open (the refusal of any other
            // target is `use_for_lower_lods_is_for_model_sections`).
            (None, Err(e)) if d.name == m::GEOMETRY_LOWER_LODS && e.code == code::NO_DOCUMENT => {}
            // Retail tag edits need the disc's screens: ours are refused.
            (None, Err(e))
                if matches!(d.name, m::UI_HIDE | m::UI_DUPLICATE | m::UI_SET_STRING)
                    && e.code == code::REFUSED => {}
            // A new multiplayer map is built over the disc's coagulation,
            // which the invented source lacks (the refusal is
            // `map_new_names_why_it_cannot_start`).
            (None, Err(e))
                if d.name == m::MAP_NEW && e.code == code::REFUSED && e.message.contains("coagulation") => {}
            (None, Err(e)) => panic!("{} {params}: {e}", d.name),
            (None, Ok(v)) => {
                assert!(
                    !v.is_null() || matches!(d.name, m::METHODS | m::MIRROR_PICK),
                    "{}",
                    d.name
                );
                if d.name.starts_with("anim.") {
                    let fields = shape_keys(d.result);
                    let result = v.as_object().expect("an animation result object");
                    for (name, required) in &fields {
                        assert!(
                            !*required || result.contains_key(name),
                            "{}: missing {name}",
                            d.name
                        );
                    }
                    for name in result.keys() {
                        assert!(
                            fields.iter().any(|(field, _)| field == name),
                            "{}: undeclared {name}",
                            d.name
                        );
                    }
                }
            }
        }
        ran.push(d.name);
    }
    // Every method name the protocol crate has is in the table.
    let names: Vec<&str> = METHODS.iter().map(|d| d.name).collect();
    for n in &names {
        assert_eq!(
            names.iter().filter(|x| *x == n).count(),
            1,
            "{n} listed twice"
        );
    }
    assert_eq!(ran.len(), Api::new().methods().len());
}

#[test]
fn pair_playtest_and_settings_share_the_window_controls() {
    let t = Temp::new("playtest-api");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(code_of(api.call(&mut app, m::PAIR_PLAYTEST, Value::Null)), code::REFUSED);
    crate::play::Play::new().update(&mut app);
    assert_eq!(call(&mut api, &mut app, m::PAIR_PLAYTEST, Value::Null), json!({"step":"launch"}));
    assert_eq!(app.pairing.requests, [crate::play::pair::Request::Play]);
    assert_eq!(
        code_of(api.call(&mut app, m::PAIR_PLAYTEST, json!({"confirm":"send"}))),
        code::REFUSED,
    );
    let settings = call(&mut api, &mut app, m::PAIR_SETTINGS, json!({
        "pause_while_editing": true, "spawn": "resume"
    }));
    assert_eq!(settings, json!({"pause_while_editing":true, "spawn":"resume"}));
    assert!(app.pairing.pause_while_editing);
    assert_eq!(app.pairing.spawn, Some(crate::play::Spawn::Resume));
}

#[cfg(unix)]
#[test]
fn pair_playtest_describes_manual_reload_and_prompt_answers() {
    use crate::play::{Play, playtest, testing::fake};
    use agent_proto::sim::SessionKind;

    let t = Temp::new("playtest-result");
    let mut app = app(&t);
    let mut api = Api::new();
    let definition = api.methods().into_iter().find(|d| d.name == m::PAIR_PLAYTEST).unwrap();
    let result: Value = serde_json::from_str(definition.result).unwrap();
    assert_eq!(result["why?"], "string");
    assert_eq!(result["answer?"], "cancel|open");

    let game = fake::Game::start("playtest-result", "other", SessionKind::Local);
    let mut play = Play::new();
    play.pair(&mut app, agent_proto::Endpoint::Unix(game.path.clone()));
    fake::until(&mut play, &mut app, "other game map", |_, a| {
        a.pairing.playtest.as_ref().is_some_and(|offer| offer.step
            == (playtest::Step::Ask { ask: playtest::Ask::Load }))
    });
    let mut previous = None;
    for choice in ["cancel", "open"] {
        let ask = call(&mut api, &mut app, m::PAIR_PLAYTEST, Value::Null);
        assert_eq!(ask["ask"], "load");
        let prompt = ask["prompt"].as_u64().unwrap();
        if let Some(old) = previous.replace(prompt) {
            assert_ne!(old, prompt);
            let stale = api.call(&mut app, m::PAIR_PLAYTEST, json!({"confirm":choice,"prompt":old}));
            assert_eq!(code_of(stale), code::REFUSED);
        }
        let reply = call(&mut api, &mut app, m::PAIR_PLAYTEST, json!({"confirm": choice}));
        assert_eq!(reply, json!({"step":"ask", "answer":choice}));
        play.update(&mut app);
    }
    let manual = playtest::Step::Reload(playtest::Reload::Manual).report(None).unwrap();
    assert_eq!(manual["step"], "reload");
    assert!(manual["why"].as_str().unwrap().contains("load it in the game"));
}

#[cfg(unix)]
#[test]
fn pair_status_carries_link_view() {
    use crate::play::Play;
    use crate::play::testing::fake;
    use agent_proto::sim::SessionKind;

    let t = Temp::new("pair-link");
    let mut app = app(&t);
    let mut play = Play::new();
    let mut api = Api::new();
    play.update(&mut app);
    let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
    assert_eq!(status["state"], "off");
    assert_eq!(
        status["link"],
        json!({"state":"no_game", "text":"No game", "why":null, "issues":[]})
    );
    let game = fake::Game::start("api-link", "example", SessionKind::Local);
    fake::pair_for_view(&mut play, &mut app, &game);
    let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
    assert_eq!(status["state"], "connected");
    assert_eq!(status["link"]["state"], "viewing");
    assert_eq!(status["link"]["text"], play.link_view(&app).text);
}

#[cfg(unix)]
mod paired_socket {
    use super::*;
    use crate::app::tag_tests::{self, WEAP};
    use crate::play::testing::fake::{self, Game, batch_pending, until};
    use crate::play::{Conn, Play, sync};
    use agent_proto::sim::SessionKind;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    // The editor socket is pumped by the same thread that advances Play,
    // just as it is between frames of the window.
    pub(super) struct Editor {
        client: agent_proto::Client,
        _listener: server::Listener,
        rx: Receiver<server::Incoming>,
        app: App,
        api: Api,
        play: Play,
    }

    impl Editor {
        pub(super) fn paired(t: &Temp, tag: &str) -> (Game, Self) {
            let game = Game::start(tag, "example", SessionKind::Local);
            let (mut app, ..) = tag_tests::app(&t.0);
            let mut play = Play::new();
            play.request(crate::play::pair::Request::PairSocket(agent_proto::Endpoint::Unix(
                game.path.clone(),
            )));
            until(&mut play, &mut app, "the named game and its journal", |p, a| {
                p.edits_blocked(a).as_deref() == Some("View mode")
            });
            let sock = t.path("w.sock");
            let (listener, rx) = server::listen(&sock).unwrap();
            let client = studio_proto::cli::connect(&sock).unwrap();
            client.set_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut api = Api::new();
            api.window = true;
            (
                game,
                Self {
                    client,
                    _listener: listener,
                    rx,
                    app,
                    api,
                    play,
                },
            )
        }

        pub(super) fn call(&mut self, method: &str, params: Value) -> Value {
            let client = &mut self.client;
            std::thread::scope(|scope| {
                let request = scope.spawn(move || client.call(method, params));
                while !request.is_finished() {
                    self.api.pump(&mut self.app, &self.rx);
                    self.play.update(&mut self.app);
                    std::thread::sleep(Duration::from_millis(2));
                }
                request.join().unwrap().unwrap()
            })
        }

        pub(super) fn wait(&mut self, what: &str, done: impl FnMut(&Play, &App) -> bool) {
            until(&mut self.play, &mut self.app, what, done);
        }

        fn send_speed(&mut self) {
            self.call(
                m::TAG_SET,
                json!({"tag":"weap", "path":"speed", "value":4.5}),
            );
            self.wait("the pending field", |_, a| !a.pairing.pending.is_empty());
            let pending = self.call(m::MIRROR_PENDING, Value::Null);
            let rows = pending["pending"].as_array().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["doc"], "tags");
            assert_eq!(rows[0]["at"], WEAP);
            assert!(rows[0]["path"].as_str().unwrap().contains("speed"));
            assert_eq!(rows[0]["effect"], "not_known");
            assert_eq!(rows[0]["text"], "Not known; reload to be sure.");
            assert_eq!(rows[0]["class"], "field");
            assert_eq!(
                self.call(m::MIRROR_APPLY, json!({"what":"send"})),
                json!({"queued":"send"})
            );
            self.wait("the outgoing batch", |p, _| batch_pending(p));
        }
    }

    #[test]
    fn mirror_apply_clears_the_pending_field_only_after_ack() {
        let t = Temp::new("mir-ack");
        let (game, mut editor) = Editor::paired(&t, "mir-ack");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        editor.call(m::MIRROR_APPLY, json!({"what":"send"}));
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"][0]["at"],
            WEAP
        );

        // A diff while the first write waits still compares to the file.
        editor.call(m::DOC_UNDO, json!({"doc":"tags"}));
        editor.wait("undo compared to the file", |_, a| {
            a.pairing.pending.is_empty()
        });
        editor.call(m::DOC_REDO, json!({"doc":"tags"}));
        editor.wait("redo compared to the file", |_, a| {
            !a.pairing.pending.is_empty()
        });
        assert!(batch_pending(&editor.play));
        drop(boundary);

        editor.wait("the acknowledged field", |p, a| {
            !batch_pending(p) && a.pairing.pending.is_empty()
        });
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"],
            json!([])
        );

        // The inverse is a fence on the ordered game connection. Any
        // duplicate batch queued before the ack would arrive first.
        editor.call(m::DOC_UNDO, json!({"doc":"tags"}));
        editor.wait("the inverse write", |p, a| {
            !batch_pending(p)
                && a.pairing.pending.is_empty()
                && game.calls("edit.batch").len() >= 2
        });
        let calls = game.calls("edit.batch");
        assert_eq!(calls.len(), 2);
        let (first, undo) = (&calls[0]["writes"][0], &calls[1]["writes"][0]);
        assert_eq!(first["at"], WEAP);
        assert_eq!(first["value"], sync::hex(&4.5f32.to_le_bytes()));
        assert_eq!(first["was"], sync::hex(&2.5f32.to_le_bytes()));
        assert_eq!(undo["was"], first["value"]);
        assert_eq!(undo["value"], first["was"]);
    }

    #[test]
    fn pair_disconnect_restores_after_the_outstanding_batch() {
        let t = Temp::new("mir-leave");
        let (game, mut editor) = Editor::paired(&t, "mir-leave");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        editor.call(m::PAIR_DISCONNECT, json!({"leave":"restore"}));
        assert_eq!(editor.call(m::MIRROR_PENDING, Value::Null)["leaving"], true);
        assert_eq!(
            editor.call(m::PAIR_STATUS, Value::Null)["state"],
            "connected"
        );
        drop(boundary);

        editor.wait("restore and disconnect", |p, _| p.conn == Conn::Off);
        assert_eq!(game.calls("edit.batch").len(), 1);
        assert_eq!(game.calls("edit.revert").len(), 1);
        let state = game.edits.lock().unwrap();
        assert!(state.journal.is_empty());
        assert_eq!(state.bytes[&WEAP.to_string()], sync::hex(&2.5f32.to_le_bytes()));
    }

    #[test]
    fn losing_the_socket_mid_batch_keeps_the_field_pending() {
        let t = Temp::new("mir-lost");
        let (game, mut editor) = Editor::paired(&t, "mir-lost");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        game.agent.shutdown();
        drop(boundary);
        drop(game);
        editor.wait("the interrupted connection", |p, a| {
            matches!(p.conn, Conn::Reconnecting { .. }) && !a.pairing.pending.is_empty()
        });
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"][0]["at"],
            WEAP
        );

        let replacement = Game::start("mir-new", "example", SessionKind::Local);
        fake::pair_for_view(&mut editor.play, &mut editor.app, &replacement);
        editor.call(m::MIRROR_APPLY, json!({"what":"send"}));
        editor.wait("the fresh game to acknowledge the edit", |p, a| {
            !batch_pending(p)
                && a.pairing.pending.is_empty()
                && !replacement.calls("edit.batch").is_empty()
        });
        let calls = replacement.calls("edit.batch");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["writes"][0]["was"], sync::hex(&2.5f32.to_le_bytes()));
    }

    #[test]
    fn an_epoch_change_mid_batch_requires_a_fresh_send() {
        let t = Temp::new("mir-cut");
        let (game, mut editor) = Editor::paired(&t, "mir-cut");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        game.bsp.store(1, std::sync::atomic::Ordering::Relaxed);
        drop(boundary);

        editor.wait("the new epoch and its journal", |p, a| {
            a.pairing.shown.as_ref().is_some_and(|s| s.epoch == 2)
                && p.link_view(a).state.name() == "viewing"
                && !a.pairing.pending.is_empty()
        });
        assert_eq!(game.calls("edit.batch").len(), 1);
        assert!(game.edits.lock().unwrap().bytes.is_empty());
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"][0]["at"],
            WEAP
        );

        editor.call(m::MIRROR_APPLY, json!({"what":"send"}));
        editor.wait("the new epoch to acknowledge", |p, a| {
            !batch_pending(p) && a.pairing.pending.is_empty()
        });
        let calls = game.calls("edit.batch");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["epoch"], 1);
        assert_eq!(calls[1]["epoch"], 2);
        assert_eq!(calls[1]["writes"], calls[0]["writes"]);
    }
}

/// P14: `view.lighting` reads and sets the Lighting mode, refuses an
/// unknown one, and `view.stats` reports it under `native`, beside the
/// level's BSPs on the native renderer (none here: the test app builds
/// no art).
#[test]
fn view_lighting_sets_the_mode_and_stats_report_it() {
    let t = Temp::new("view-lighting");
    let mut app = app(&t);
    let mut api = Api::new();
    let read = call(&mut api, &mut app, m::VIEW_LIGHTING, Value::Null);
    assert_eq!(read["mode"], "game");
    assert_eq!(read["modes"], json!(["game", "flat", "lightmap", "collision"]));
    let set = call(&mut api, &mut app, m::VIEW_LIGHTING, json!({"mode": "lightmap"}));
    assert_eq!(set["mode"], "lightmap");
    assert_eq!(app.lighting, scenario_edit::native_view::Lighting::LightmapOnly);
    for invalid in [json!({"mode": "bright"}), json!({"mode": 3})] {
        let reply = api.call(&mut app, m::VIEW_LIGHTING, invalid);
        assert_eq!(code_of(reply), code::INVALID_PARAMS);
    }
    assert_eq!(app.lighting, scenario_edit::native_view::Lighting::LightmapOnly);
    let stats = call(&mut api, &mut app, m::VIEW_STATS, Value::Null);
    assert_eq!(stats["native"]["lighting"], "lightmap");
    assert!(stats["native"]["bsps"].as_array().is_some());
    assert!(stats["native"]["errors"].as_array().is_some());
    // Each BSP's base look: translated, or the named fallback with its
    // layers off.
    for b in stats["native"]["bsps"].as_array().unwrap() {
        let look = &b["base_look"];
        let translated = look["translated"].as_bool().expect("base_look.translated");
        assert_eq!(look["layers_off"], json!(!translated), "{look}");
        assert_eq!(look["fallback"].is_string(), !translated, "{look}");
    }
    // The placed objects: null with no art built, else the counts.
    let objects = &stats["native"]["objects"];
    assert!(stats["native"].get("objects").is_some());
    assert!(
        objects.is_null()
            || (objects["models"].is_u64()
                && objects["placed"].is_u64()
                && objects["errors"].is_array()),
        "{objects}"
    );
}

#[test]
fn view_layers_applies_overrides_atomically_and_shares_the_halo1_menu_switch() {
    let t = Temp::new("view-layers");
    let mut app = app(&t);
    let mut api = Api::new();
    let result = call(&mut api, &mut app, m::VIEW_LAYERS, json!({
        "preset": "everything", "set": {"fog": false}, "xray": true
    }));
    assert_eq!(result["xray"], true);
    let rows = result["layers"].as_array().unwrap();
    assert_eq!(rows.iter().find(|r| r["name"] == "fog").unwrap()["on"], false);
    let scenery = rows.iter().find(|r| r["label"] == "Scenery").unwrap();
    assert_eq!(scenery["count"], 1);
    assert_eq!(scenery["on"], true);
    assert_eq!(scenery["color"].as_array().unwrap().len(), 3);

    for invalid in [
        json!({"preset": "missing"}),
        json!({"preset": "art", "set": []}),
        json!({"preset": "art", "set": {"fog": true, "unknown": false}}),
        json!({"set": {"fog": "on"}}),
        json!({"set": {"fog": true}, "xray": 1}),
    ] {
        let reply = api.call(&mut app, m::VIEW_LAYERS, invalid);
        assert_eq!(code_of(reply), code::INVALID_PARAMS);
        assert_eq!(call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null), result);
    }

    assert!(app.overlay_shown());
    app.dispatch(Action::SetOverlay(false));
    let hidden = call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null);
    let halo = hidden["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "halo1")
        .unwrap();
    assert_eq!(halo["on"], false);
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"halo1": true}}));
    assert!(app.overlay_shown());
}

/// P8: the invented map's two volumes, kill trigger and BSP switch
/// through `volume.list`; `volume.select` replaces the placements, frames
/// the box and shows in `nav.status`; the layers count and draw them.
#[test]
fn volume_list_and_select_read_the_invented_volumes() {
    let t = Temp::new("volumes");
    let mut app = app(&t);
    let mut api = Api::new();
    // The plain invented map has none.
    let none = call(&mut api, &mut app, m::VOLUME_LIST, Value::Null);
    assert_eq!(none["volumes"], json!([]));
    assert_eq!(none["error"], Value::Null);
    let map = t.path("volumes.map");
    std::fs::write(&map, fixture::fixture_with_volumes()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );

    let all = call(&mut api, &mut app, m::VOLUME_LIST, Value::Null);
    let rows = all["volumes"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (all["kill_triggers"].clone(), all["bsp_switches"].clone()),
        (json!(1), json!(1))
    );
    for (row, (origin, forward, size)) in rows.iter().zip(fixture::VOLUMES) {
        assert_eq!(row["origin"], json!(origin));
        assert_eq!(row["forward"], json!(forward));
        assert_eq!(row["up"], json!([0.0, 0.0, 1.0]));
        assert_eq!(row["size"], json!(size));
        assert_eq!(row["world"], true);
    }
    assert_eq!(rows[0]["kinds"], json!(["bsp_switch", "trigger"]));
    assert_eq!(
        rows[0]["named_by"],
        json!([{"kind": "bsp_switch", "index": 0, "from": 0, "to": 1}])
    );
    assert_eq!(rows[1]["kinds"], json!(["kill", "trigger"]));
    assert_eq!(rows[1]["label"], "trigger volume 1");
    // The turned box: forward +y, so it runs 6 along y and 2 along -x.
    assert_eq!(rows[1]["center"], json!([-41.0, -37.0, -8.5]));
    let kills = call(&mut api, &mut app, m::VOLUME_LIST, json!({"kind": "kill"}));
    let kill_rows = kills["volumes"].as_array().unwrap();
    assert_eq!(kill_rows.len(), 1);
    assert_eq!(kill_rows[0]["index"], 1);
    assert_eq!(
        code_of(api.call(&mut app, m::VOLUME_LIST, json!({"kind": "soft"}))),
        code::INVALID_PARAMS
    );

    let layers = call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null);
    let row = |name: &str| {
        layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    for (name, count) in [
        ("trigger_volumes", 2),
        ("kill_volumes", 1),
        ("bsp_switches", 1),
    ] {
        assert_eq!(row(name)["group"], "Volumes", "{name}");
        assert_eq!(row(name)["count"], count, "{name}");
    }

    call(
        &mut api,
        &mut app,
        m::SCENARIO_SELECT,
        json!({"block": "weapons", "index": 0}),
    );
    let selected = call(
        &mut api,
        &mut app,
        m::VOLUME_SELECT,
        json!({"index": 1, "focus": true}),
    );
    assert_eq!(selected, json!({"selected": 1}));
    assert_eq!(app.session.selected(), None);
    let nav = call(&mut api, &mut app, m::NAV_STATUS, Value::Null);
    assert_eq!(
        (nav["volume"].clone(), nav["selected"].clone()),
        (json!(1), Value::Null)
    );
    assert_eq!(app.session.camera().target, [-41.0, -37.0, -8.5]);
    assert_eq!(
        call(&mut api, &mut app, m::VOLUME_LIST, Value::Null)["selected"],
        1
    );
    assert_eq!(
        code_of(api.call(&mut app, m::VOLUME_SELECT, json!({"index": 2}))),
        code::INVALID_PARAMS
    );
    assert_eq!(app.session.selected_volume(), Some(1));
    call(
        &mut api,
        &mut app,
        m::SCENARIO_SELECT,
        json!({"block": "weapons", "index": 0}),
    );
    assert_eq!(app.session.selected_volume(), None);
    call(&mut api, &mut app, m::VOLUME_SELECT, json!({"index": 0}));
    assert_eq!(
        call(&mut api, &mut app, m::VOLUME_SELECT, Value::Null),
        json!({"selected": null})
    );
    assert_eq!(app.session.selected_volume(), None);

    // Each volume layer draws its boxes in the snapshot.
    let out = t.path("volumes.png");
    let params = json!({
        "out": out, "size": [128, 96],
        "camera": {"target": [-41, -37, -8.5], "distance": 20, "yaw": 30, "pitch": 30}
    });
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"preset": "everything"}),
    );
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Ok(_) => {}
        Err(e) => panic!("view.snapshot: {e}"),
    }
    let shown = png_pixels(&out);
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"set": {"trigger_volumes": false, "kill_volumes": false}}),
    );
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
    let hidden = png_pixels(&out);
    assert!(
        hidden != shown,
        "hiding the volume layers must remove the box"
    );
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"set": {"kill_volumes": true}}),
    );
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(
        png_pixels(&out) != hidden,
        "the Kill layer alone draws the kill box"
    );
}

/// P10: the invented structure BSP's room through `collision.*`. It has
/// no render geometry, so all three surfaces are collision only;
/// `collision.at` finds the far wall by ray and by point whatever layers
/// are shown, and selecting it replaces the placements. The collision
/// layers start hidden and count the surfaces.
#[test]
fn collision_surfaces_and_at_report_the_invented_rooms_walls() {
    let t = Temp::new("collision");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("collision.map");
    std::fs::write(&map, fixture::fixture_with_collision()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    let all = call(&mut api, &mut app, m::COLLISION_SURFACES, Value::Null);
    assert_eq!(all["bsps"].as_array().unwrap().len(), 1);
    assert_eq!(all["counts"], json!({"drawn": 0, "collision_only": 3}));
    assert_eq!(all["errors"], json!([]));
    let rows = all["surfaces"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["class"], "collision_only");
    assert_eq!(rows[2]["breakable"], true);
    assert_eq!(rows[2]["ring"].as_array().unwrap().len(), 4);
    assert_eq!(rows[0]["breakable"], false);
    let drawn = call(
        &mut api,
        &mut app,
        m::COLLISION_SURFACES,
        json!({"class": "drawn"}),
    );
    assert_eq!(drawn["matching"], 0);
    let paged = call(
        &mut api,
        &mut app,
        m::COLLISION_SURFACES,
        json!({"offset": 1, "limit": 1}),
    );
    assert_eq!(paged["matching"], 3);
    assert_eq!(paged["surfaces"][0]["index"], 1);
    for bad in [
        (m::COLLISION_SURFACES, json!({"class": "invisible"})),
        (
            m::COLLISION_AT,
            json!({"origin": [0, 0, 0], "dir": [1, 0, 0], "point": [0, 0, 0]}),
        ),
        (m::COLLISION_AT, json!({"origin": [0, 0, 0]})),
        (
            m::COLLISION_AT,
            json!({"origin": [0, 0, 0], "dir": [0, 0, 0]}),
        ),
        (m::COLLISION_SELECT, json!({"index": 3})),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, bad.0, bad.1.clone())),
            code::INVALID_PARAMS,
            "{}",
            bad.1
        );
    }

    // The far wall stands at x 210; the layers are hidden by default.
    let ray = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({
            "origin": [205, 205, 1], "dir": [2, 0, 0]
        }),
    );
    assert_eq!(ray["surface"]["index"], 2);
    assert_eq!(ray["surface"]["class"], "collision_only");
    assert!((ray["distance"].as_f64().unwrap() - 5.0).abs() < 1e-4);
    let point = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({"point": [209.98, 205, 1]}),
    );
    assert_eq!(point["surface"]["index"], 2);
    let miss = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({"point": [205, 205, 2]}),
    );
    assert_eq!(miss["surface"], Value::Null);

    call(
        &mut api,
        &mut app,
        m::SCENARIO_SELECT,
        json!({"block": "netgame_flags", "index": 0}),
    );
    let picked = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({
            "origin": [205, 205, 1], "dir": [1, 0, 0], "select": true
        }),
    );
    assert_eq!(picked["selected"], 2);
    assert_eq!(app.session.selected(), None);
    let cleared = call(&mut api, &mut app, m::COLLISION_SELECT, Value::Null);
    assert_eq!(cleared["selected"], Value::Null);
    let framed = call(
        &mut api,
        &mut app,
        m::COLLISION_SELECT,
        json!({"index": 0, "focus": true}),
    );
    assert_eq!(framed["selected"], 0);
    let target = app.session.camera().target;
    assert!(
        (target[0] - 205.0).abs() < 1e-3 && (target[1] - 205.0).abs() < 1e-3,
        "{target:?}"
    );

    let layers = call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null);
    let row = |name: &str| {
        layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    for (name, count) in [("level_collision", 3), ("collision_only", 3)] {
        assert_eq!(row(name)["group"], "Collision", "{name}");
        assert_eq!(row(name)["count"], count, "{name}");
        assert_eq!(row(name)["on"], false, "{name}");
    }
    let shown = call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"preset": "collision"}),
    );
    assert!(
        shown["layers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["group"] == "Collision")
            .all(|r| r["on"] == true)
    );
}

/// P11: the invented instance's quad through `collision.surfaces`, and
/// the invented scenery's `coll` cube and `phmo` sphere through
/// `collision.objects`, each counted by its layer.
#[test]
fn collision_objects_and_instances_report_the_invented_shapes() {
    let t = Temp::new("collision-objects");
    let mut app = app(&t);
    let mut api = Api::new();
    let open = |api: &mut Api, app: &mut App, name: &str, bytes: Vec<u8>| {
        let map = t.path(name);
        std::fs::write(&map, bytes).unwrap();
        call(
            api,
            app,
            m::SOURCE_OPEN,
            json!({"path": map.display().to_string()}),
        );
    };
    let layer = |api: &mut Api, app: &mut App, name: &str| {
        let layers = call(api, app, m::VIEW_LAYERS, Value::Null);
        layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["count"]
            .clone()
    };

    open(
        &mut api,
        &mut app,
        "instance.map",
        fixture::fixture_with_instance_collision(),
    );
    let all = call(&mut api, &mut app, m::COLLISION_SURFACES, Value::Null);
    let rows = all["surfaces"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["instance"], Value::Null);
    assert_eq!(rows[3]["instance"], 0);
    assert_eq!(rows[3]["plane"], json!([0.0, 0.0, 1.0, 3.0]));
    assert_eq!(layer(&mut api, &mut app, "level_collision"), 3);
    assert_eq!(layer(&mut api, &mut app, "instance_collision"), 1);
    let down = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({"origin": [202.25, 202.5, 10], "dir": [0, 0, -1]}),
    );
    assert_eq!(down["surface"]["index"], 3);

    open(
        &mut api,
        &mut app,
        "object.map",
        fixture::fixture_with_object_collision(),
    );
    let objects = call(&mut api, &mut app, m::COLLISION_OBJECTS, Value::Null);
    assert_eq!(objects["error"], Value::Null);
    let models = objects["models"].as_array().unwrap();
    let crate_model = models.iter().find(|m| m["shapes"] == 2).unwrap();
    assert_eq!(crate_model["nodes"], 2);
    assert_eq!(crate_model["errors"], json!([]));
    let shapes = objects["shapes"].as_array().unwrap();
    assert_eq!(shapes.len(), 2);
    let kinds: Vec<&str> = shapes.iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["bsp", "sphere"]);
    for s in shapes {
        assert_eq!(s["block"], "scenery");
        assert_eq!(s["index"], 0);
    }
    assert_eq!(shapes[0]["node"], 1);
    assert_eq!(shapes[0]["center"], json!([5.0, 5.0, 6.0]));
    assert_eq!(shapes[1]["center"], json!([5.0, 5.0, 5.5]));
    assert_eq!(shapes[1]["radius"], 0.5);
    let none = call(
        &mut api,
        &mut app,
        m::COLLISION_OBJECTS,
        json!({"block": "weapons"}),
    );
    assert_eq!(none["shapes"], json!([]));
    assert_eq!(layer(&mut api, &mut app, "object_collision"), 2);
}

/// P9: `volume.set` moves, turns and resizes a box as one undo step each;
/// `volume.remove` refuses a volume a kill trigger names, naming it, and
/// takes the kill trigger first; `volume.add` with `kill` adds both
/// entries, refused past the source size while growth is off.
#[test]
fn volume_set_add_and_remove_edit_through_the_command_layer() {
    let t = Temp::new("volume-edits");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("volumes.map");
    std::fs::write(&map, fixture::fixture_with_volumes()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    let undo = |api: &mut Api, app: &mut App| {
        call(api, app, m::DOC_UNDO, json!({"doc": "map"}));
    };

    // Move by the centre, then resize keeping it, then turn.
    let moved = call(
        &mut api,
        &mut app,
        m::VOLUME_SET,
        json!({"index": 0, "center": [110, 100, 2]}),
    );
    assert_eq!(moved["volume"]["center"], json!([110.0, 100.0, 2.0]));
    assert_eq!(moved["volume"]["size"], json!([4.0, 4.0, 4.0]));
    let sized = call(
        &mut api,
        &mut app,
        m::VOLUME_SET,
        json!({"index": 0, "size": [8, 2, 4]}),
    );
    assert_eq!(sized["volume"]["center"], json!([110.0, 100.0, 2.0]));
    assert_eq!(sized["volume"]["origin"], json!([106.0, 99.0, 0.0]));
    let turned = call(
        &mut api,
        &mut app,
        m::VOLUME_SET,
        json!({"index": 0, "forward": [0, 2, 0]}),
    );
    assert_eq!(turned["volume"]["forward"], json!([0.0, 1.0, 0.0]));
    assert_eq!(turned["volume"]["center"], json!([110.0, 100.0, 2.0]));
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VOLUME_SET,
            json!({"index": 0, "size": [0, 1, 1]})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::VOLUME_SET, json!({"index": 5}))),
        code::INVALID_PARAMS
    );
    for _ in 0..3 {
        undo(&mut api, &mut app);
    }
    let row = &call(&mut api, &mut app, m::VOLUME_LIST, Value::Null)["volumes"][0];
    assert_eq!(row["origin"], json!(fixture::VOLUMES[0].0));
    assert_eq!(row["size"], json!(fixture::VOLUMES[0].2));
    assert!(!app.session.is_dirty());

    // Volume 1 is the kill box: the refusal names its kill trigger.
    let e = api
        .call(&mut app, m::VOLUME_REMOVE, json!({"index": 1}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("kill trigger 0"), "{}", e.message);
    assert_eq!(
        e.data.unwrap()["named_by"],
        json!([{"kind": "kill", "index": 0}])
    );
    let gone = call(
        &mut api,
        &mut app,
        m::VOLUME_REMOVE,
        json!({"kind": "kill", "index": 0}),
    );
    assert_eq!(gone["kill_triggers"], 0);
    let gone = call(&mut api, &mut app, m::VOLUME_REMOVE, json!({"index": 1}));
    assert_eq!(gone["triggers"], 1);

    // Room for one more of each: Make a kill volume fills it.
    let made = call(
        &mut api,
        &mut app,
        m::VOLUME_ADD,
        json!({"kill": true, "center": [0, 0, 2], "size": [10, 10, 1]}),
    );
    assert_eq!(
        (made["index"].clone(), made["kill_trigger"].clone()),
        (json!(1), json!(0))
    );
    assert_eq!(made["volume"]["kinds"], json!(["kill", "trigger"]));
    assert_eq!(made["volume"]["center"], json!([0.0, 0.0, 2.0]));
    assert_eq!(app.session.selected_volume(), Some(1));
    let e = api
        .call(&mut app, m::VOLUME_ADD, json!({"kill": true}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert_eq!(e.data.unwrap()["capacity"], 2);
    // One undo takes both entries away.
    undo(&mut api, &mut app);
    let list = call(&mut api, &mut app, m::VOLUME_LIST, Value::Null);
    assert_eq!(list["volumes"].as_array().unwrap().len(), 1);
    assert_eq!(list["kill_triggers"], 0);
    // BSP switches are read only.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VOLUME_REMOVE,
            json!({"kind": "bsp_switch", "index": 0})
        )),
        code::INVALID_PARAMS
    );
}

fn png_pixels(path: &Path) -> Vec<u8> {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let mut png = png::Decoder::new(file).read_info().unwrap();
    let mut rgba = vec![0; png.output_buffer_size().unwrap()];
    png.next_frame(&mut rgba).unwrap();
    rgba
}

#[test]
fn view_layers_changes_snapshot_pixels_on_the_fixture() {
    let t = Temp::new("view-layer-pixels");
    let mut app = app(&t);
    let mut api = Api::new();
    let out = t.path("view.png");
    let params = json!({
        "out": out, "size": [128, 96],
        "camera": {"target": [5, 5, 5], "distance": 2, "yaw": 15, "pitch": 15}
    });
    // The fixture has one scenery glyph at [5, 5, 5].
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": "everything"}));
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Ok(_) => {}
        Err(e) => panic!("view.snapshot: {e}"),
    }
    let shown = png_pixels(&out);
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"scenery": false}}));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
    assert!(png_pixels(&out) != shown, "hiding scenery must remove its glyph from the snapshot");
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"scenery": true}}));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(png_pixels(&out) == shown, "showing scenery must restore the original pixels");
}

#[test]
fn view_layers_gameplay_legend_counts_survive_changes_of_preset() {
    let t = Temp::new("view-gameplay-legend");
    let mut app = app(&t);
    let mut api = Api::new();
    for (preset, enabled) in [
        ("gameplay", [true, true, true]),
        ("art", [false, false, true]),
        ("collision", [false, false, false]),
    ] {
        let state = call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": preset}));
        let rows = state["layers"].as_array().unwrap();
        for ((name, count), on) in [("spawn_points", 2), ("flags", 1), ("netgame_items", 1)]
            .into_iter()
            .zip(enabled)
        {
            let row = rows.iter().find(|r| r["name"] == name).unwrap();
            assert_eq!(row["group"], "Gameplay", "{name}");
            assert_eq!(row["count"], count, "{name}");
            assert_eq!(row["on"], on, "{preset} / {name}");
        }
    }
}

#[test]
fn view_layers_gameplay_switches_and_respawn_labels_change_snapshot_pixels() {
    let t = Temp::new("view-gameplay-pixels");
    let mut app = app(&t);
    let mut api = Api::new();
    let path = t.path("gameplay.png");
    // All three kinds of placement are in frame. Keep the camera fixed so
    // only the changed layer (or the edited respawn caption) affects pixels.
    let params = json!({
        "out": path, "size": [320, 240],
        "camera": {"target": [1, 1, 1], "distance": 14, "yaw": 80, "pitch": 15}
    });
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": "gameplay"}));
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Err(e) => panic!("view.snapshot: {e}"),
        Ok(_) => {}
    }
    let shown = png_pixels(&path);
    for name in ["spawn_points", "flags", "netgame_items"] {
        call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {(name): false}}));
        call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
        assert!(png_pixels(&path) != shown, "hiding {name} must change the picture");
        call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {(name): true}}));
    }
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
    assert!(png_pixels(&path) == shown, "showing all three layers must restore the picture");

    call(&mut api, &mut app, m::SCENARIO_SET, json!({
        "block": "netgame_equipment", "index": 0, "field": "respawn time", "value": 77
    }));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(png_pixels(&path) != shown, "the equipment's new respawn time must appear in its caption");
}

#[test]
fn methods_describe_their_params_results_and_examples() {
    let t = Temp::new("methods");
    let mut app = app(&t);
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::METHODS, Value::Null);
    let list = list.as_array().unwrap();
    // The table with the providers' methods over and after it.
    let defs = api.methods();
    assert_eq!(list.len(), defs.len());
    for (d, v) in defs.iter().zip(list) {
        assert_eq!(v["name"], d.name);
        assert!(
            v["params"].is_object(),
            "{}: params shape {}",
            d.name,
            d.params
        );
        assert!(
            !v["result"].is_string(),
            "{}: result shape {}",
            d.name,
            d.result
        );
        assert!(!d.summary.is_empty());
        assert_eq!(
            v["status"],
            if d.planned.is_some() {
                "planned"
            } else {
                "ready"
            }
        );
        // The example names only params the shape has.
        if let Value::Object(o) = &v["example"] {
            let keys = shape_keys(d.params);
            for k in o.keys() {
                assert!(
                    keys.iter().any(|(n, _)| n == k),
                    "{}: example param {k}",
                    d.name
                );
            }
        }
    }
    let planned: Vec<&str> = list
        .iter()
        .filter(|v| v["status"] == "planned")
        .map(|v| v["name"].as_str().unwrap().split('.').next().unwrap())
        .collect();
    for ns in ["model", "custom"] {
        assert!(planned.contains(&ns), "{ns}.* is a planned namespace");
    }
    // The HUD tool's provider answers all of hud.*.
    assert!(!planned.contains(&"hud"));
    for name in [m::HUD_LIST, m::HUD_READ, m::HUD_SET, m::HUD_MOVE, m::HUD_RENDER, m::HUD_CHECK, m::HUD_SAVE_AS] {
        assert!(list.iter().any(|v| v["name"] == name && v["status"] == "ready"), "{name}");
    }
    // The models provider answers its four; colours and emblem stay planned.
    for name in [m::MODEL_LIST, m::MODEL_READ, m::MODEL_RENDER, m::MODEL_CHECK] {
        assert!(list.iter().any(|v| v["name"] == name && v["status"] == "ready"), "{name}");
    }
}

#[test]
fn params_are_checked_before_anything_runs() {
    let t = Temp::new("params");
    let mut app = app(&t);
    let mut api = Api::new();
    let before = flag_position(&app);
    let set = |p: Value| (m::SCENARIO_SET, p);
    for (method, p) in [
        set(
            json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [1, 2, 3], "typo": 1}),
        ),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position"})),
        set(json!({"block": "nope", "index": 0, "field": "position", "value": [1, 2, 3]})),
        set(json!({"block": "netgame_flags", "index": 9, "field": "position", "value": [1, 2, 3]})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "nope", "value": 1})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [1, 2]})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position", "value": "up"})),
        (m::SCENARIO_GET, json!([1, 2])),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, method, p.clone())),
            code::INVALID_PARAMS,
            "{p}"
        );
    }
    assert_eq!(
        code_of(api.call(&mut app, "scenario.nope", Value::Null)),
        code::METHOD_NOT_FOUND
    );
    assert_eq!(flag_position(&app), before, "nothing was applied");
    assert!(!app.is_dirty());
}

#[test]
fn growth_where_the_map_editor_cannot_grow_safely_is_refused_with_a_reason() {
    let t = Temp::new("growth");
    let mut app = app(&t);
    let mut api = Api::new();
    let e = api
        .call(&mut app, m::SCENARIO_ADD, json!({"block": "netgame_flags"}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("full"), "{}", e.message);
    assert_eq!(e.data.unwrap(), json!({"count": 1, "capacity": 1}));
    let e = api
        .call(
            &mut app,
            m::TAG_BLOCK_ADD,
            json!({"tag": "scnr", "path": "netgame_flags"}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(!app.is_dirty());
    // With room made, the same add goes in.
    call(
        &mut api,
        &mut app,
        m::SCENARIO_DELETE,
        json!({"block": "netgame_flags", "index": 0}),
    );
    let v = call(
        &mut api,
        &mut app,
        m::SCENARIO_ADD,
        json!({"block": "netgame_flags", "position": [9, 9, 9]}),
    );
    assert_eq!(v["index"], 0);
    assert_eq!(flag_position(&app), [9.0, 9.0, 9.0]);
}

#[test]
fn source_maps_reports_the_picker_groups_in_order() {
    use crate::app::map_cards_tests::{maps_folder, package_with_picture};

    let t = Temp::new("source-maps-groups");
    let maps = maps_folder(&t.0);
    package_with_picture(&t.path("data"));
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let mut api = Api::new();
    call(&mut api, &mut app, m::SOURCE_OPEN, json!({ "path": maps }));
    app.wait_cards();

    let result = call(&mut api, &mut app, m::SOURCE_MAPS, Value::Null);
    assert_eq!(result["maps"], json!(["02_pier", "mainmenu", "shared", "yard"]));
    assert_eq!(
        result["map_entries"],
        json!([
            {"map": "yard", "label": "Yard", "group": "multiplayer"},
            {"map": "02_pier", "label": "The Pier", "group": "campaign"},
            {"map": "mainmenu", "label": "mainmenu", "group": "shared_and_menu"},
            {"map": "shared", "label": "shared", "group": "shared_and_menu"},
        ]),
    );
    assert_eq!(result["packages"][0]["group"], "packages");
}

#[test]
fn unsaved_edits_are_never_dropped_without_discard() {
    let t = Temp::new("unsaved");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [1, 1, 1]}),
    );
    let path = t.path("example.map").display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::SOURCE_OPEN, json!({"path": path}))),
        code::UNSAVED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_CLOSE, json!({"doc": "map"}))),
        code::UNSAVED
    );
    assert!(
        app.is_dirty() && app.pending().is_none(),
        "refused, not held for a prompt"
    );
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": path, "discard": true}),
    );
    assert!(!app.is_dirty());
}

#[test]
fn outputs_stay_out_of_git_and_off_the_source() {
    let t = Temp::new("git");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [1, 1, 1]}),
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("never.map");
    let e = api
        .call(
            &mut app,
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": repo.display().to_string()}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert!(!repo.exists());
    let src = t.path("example.map").display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_SAVE_AS, json!({"doc": "map", "path": src}))),
        code::REFUSED
    );
    assert_eq!(
        std::fs::read(t.path("example.map")).unwrap(),
        fixture::fixture()
    );
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_SAVE, json!({"doc": "map"}))),
        code::NO_TARGET
    );
    assert_eq!(std::fs::read(t.path("example.map")).unwrap(), fixture::fixture());
    let first = crate::app::UiScreens::tree()[0].0;
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": first}));
    let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("never.png");
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::UI_RENDER,
            json!({"out": png.display().to_string()})
        )),
        code::REFUSED
    );
    assert!(!png.exists());
}

#[test]
fn undo_and_redo_through_the_api_for_each_document() {
    let t = Temp::new("undo");
    let mut app = app(&t);
    let mut api = Api::new();
    let before = flag_position(&app);
    call(
        &mut api,
        &mut app,
        m::SCENARIO_SET,
        json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4, 5, 6]}),
    );
    // The variant tool is shown now: undo still names the map.
    call(
        &mut api,
        &mut app,
        m::DOC_OPEN,
        json!({"variant": {"new": "slayer"}}),
    );
    let v = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(v["done"], true);
    assert_eq!(flag_position(&app), before);
    let v = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(v["done"], false, "nothing left");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&app), [4.0, 5.0, 6.0]);

    call(
        &mut api,
        &mut app,
        m::VARIANT_RENAME,
        json!({"name": "Renamed"}),
    );
    let name = |app: &App| app.variants.as_ref().unwrap().current().unwrap().name();
    assert_eq!(name(&app), "Renamed");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "variant"}));
    assert_ne!(name(&app), "Renamed");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "variant"}));
    assert_eq!(name(&app), "Renamed");

    let hop = t.path("multiplayer_hopper.dat").display().to_string();
    call(&mut api, &mut app, m::DOC_OPEN, json!({"playlists": hop}));
    call(
        &mut api,
        &mut app,
        m::PLAYLIST_SET,
        json!({"playlist": 0, "title": "Agent"}),
    );
    let title = |app: &App| app.hoppers.hoppers().unwrap()[0].title.clone();
    assert_eq!(title(&app), "Agent");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "playlists"}));
    assert_ne!(title(&app), "Agent");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "playlists"}));
    assert_eq!(title(&app), "Agent");
    let docs = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let dirty: Vec<&str> = docs
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["dirty"] == true)
        .map(|d| d["doc"].as_str().unwrap())
        .collect();
    assert_eq!(dirty, ["map", "variant", "playlists"]);
}

#[test]
fn the_window_and_the_api_make_identical_documents() {
    // The inspector's path: a dragged field (merged under one key), then
    // the end of the drag, as dispatched actions.
    let t = Temp::new("same");
    let mut ui = app(&t);
    ui.dispatch(Action::SetField {
        block: BlockKind::NetgameFlags,
        index: 0,
        field: "position".into(),
        value: scenario_edit::Value::Vec3([2.0, 2.0, 2.0]),
        merge: Some(7),
    });
    ui.dispatch(Action::SetField {
        block: BlockKind::NetgameFlags,
        index: 0,
        field: "position".into(),
        value: scenario_edit::Value::Vec3([4.0, 5.0, 6.0]),
        merge: Some(7),
    });
    ui.dispatch(Action::SealEdit);
    // The API's path.
    let t2 = Temp::new("same-api");
    let mut agent = app(&t2);
    let mut api = Api::new();
    call(
        &mut api,
        &mut agent,
        m::SCENARIO_SET,
        json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4, 5, 6]}),
    );
    let (a, b) = (
        ui.session.scenario().unwrap(),
        agent.session.scenario().unwrap(),
    );
    assert_eq!(a.changes(), b.changes());
    assert!(!a.changes().is_empty());
    // One undo step each.
    ui.dispatch(Action::Undo);
    call(&mut api, &mut agent, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&ui), flag_position(&agent));
    ui.dispatch(Action::Redo);
    call(&mut api, &mut agent, m::DOC_REDO, json!({"doc": "map"}));
    let (pa, pb) = (t.path("ui.map"), t2.path("api.map"));
    ui.dispatch(Action::SaveMapAs(pa.clone()));
    call(
        &mut api,
        &mut agent,
        m::DOC_SAVE_AS,
        json!({"doc": "map", "path": pb.display().to_string()}),
    );
    assert_eq!(std::fs::read(pa).unwrap(), std::fs::read(pb).unwrap());

    // Playlists: the table's row buttons apply the same edits.
    let hop = |t: &Temp| t.path("multiplayer_hopper.dat");
    ui.dispatch(Action::OpenHopper(hop(&t)));
    let edits = [
        PlaylistEdit::Weight {
            playlist: 0,
            entry: 0,
            weight: 9,
        },
        PlaylistEdit::Duplicate {
            playlist: 0,
            entry: 0,
        },
        PlaylistEdit::Move {
            playlist: 0,
            entry: 1,
            to: 0,
        },
    ];
    for e in &edits {
        // As entries_table does inside its frame's edit.
        ui.hoppers.edit(|hs| e.apply(hs).unwrap());
    }
    call(
        &mut api,
        &mut agent,
        m::DOC_OPEN,
        json!({"playlists": hop(&t2).display().to_string()}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "weight", "entry": 0, "weight": 9}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "duplicate", "entry": 0}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "move", "entry": 1, "to": 0}),
    );
    assert_eq!(ui.hoppers.hoppers(), agent.hoppers.hoppers());
}

#[test]
fn tags_list_read_and_follow_references() {
    let t = Temp::new("tags");
    let mut app = app(&t);
    let mut api = Api::new();
    let all = call(&mut api, &mut app, m::TAG_LIST, Value::Null);
    assert_eq!(all["total"], 4);
    let scnr = call(&mut api, &mut app, m::TAG_LIST, json!({"group": "scnr"}));
    assert_eq!(scnr["tags"][0]["name"], "levels\\test\\fixture");
    let raw = |t: Value| json!({"tag": t, "raw": true});
    let byname = call(&mut api, &mut app, m::TAG_READ, raw(json!("levels\\test\\fixture")));
    let bygroup = call(&mut api, &mut app, m::TAG_READ, raw(json!("scnr")));
    let byid = call(&mut api, &mut app, m::TAG_READ, raw(scnr["tags"][0]["id"].clone()));
    assert_eq!(byname, bygroup);
    assert_eq!(byname, byid);
    assert_eq!(byname["layout"], "schema");
    assert!(byname["root"]["fields"].as_array().unwrap().len() > 3);
    // The weapon palette names the blaster.
    let refs = call(&mut api, &mut app, m::TAG_REFS, json!({"tag": "scnr"}));
    let blaster = refs
        .as_array()
        .unwrap()
        .iter()
        .position(|r| r["name"] == "objects\\test\\blaster")
        .expect("the palette's reference");
    let to = call(&mut api, &mut app, m::TAG_FOLLOW, json!({"tag": "scnr", "ref": blaster}));
    assert_eq!(to["group"], "weap");
    let path = refs[blaster]["path"].clone();
    assert_eq!(call(&mut api, &mut app, m::TAG_FOLLOW, json!({"tag": "scnr", "ref": path})), to);

    // Any tag's fields through the Tags tool's registry and commands.
    let kit = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc:objects\\test\\kit"}));
    assert_eq!(kit["access"], "edit", "{kit}");
    assert_eq!(kit, call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"})));
    let v = call(&mut api, &mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}));
    assert_eq!(v["value"], 7);
    assert!(app.is_dirty(), "a tag edit is the map's to save");
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "tags"}));
    assert_eq!(diff["patches"][0]["tag"], "itmc:objects\\test\\kit", "{diff}");
    assert_eq!(diff["patches"][0]["offset"], "0x8");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "tags"}));
    assert!(!app.is_dirty());
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "tags"}));
    assert_eq!(call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}))["edited"], true);
    // A tag too short for its group's layout is refused with the reason.
    let e = api
        .call(&mut app, m::TAG_SET, json!({"tag": "weap", "path": "+0x4", "value": 1}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert!(e.message.contains("shorter than"), "{}", e.message);
    // The scenario's placements still edit as scenario.set.
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": "scnr", "path": "netgame_flags[0].position", "value": [7, 7, 7]}));
    assert_eq!(flag_position(&app), [7.0, 7.0, 7.0]);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_READ, json!({"tag": "nope"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn tags_browse_a_shared_cache_and_round_trip_through_a_bundle() {
    let t = Temp::new("tag-io");
    let mut app = app(&t);
    let mut api = Api::new();
    std::fs::write(t.path("shared.map"), fixture::fixture()).unwrap();
    // The open map's tags, by group and by folder, with where they live.
    let all = call(&mut api, &mut app, m::TAG_LIST, json!({"homes": true}));
    assert_eq!(all["map"], "example");
    assert_eq!(all["browsing"], false);
    assert_eq!(all["groups"]["weap"], 1);
    assert!(all["tags"].as_array().unwrap().iter().any(|t| t["raw"]["local"].is_number()));
    let objects = call(&mut api, &mut app, m::TAG_LIST, json!({"path": "objects\\test"}));
    assert_eq!(objects["total"], 2, "{objects}");
    let q = call(&mut api, &mut app, m::TAG_LIST, json!({"query": "itmc kit"}));
    assert_eq!(q["total"], 1);

    // A shared cache, read-only.
    let b = call(&mut api, &mut app, m::TAG_BROWSE, json!({"map": "shared"}));
    assert_eq!((b["map"].clone(), b["browsing"].clone(), b["total"].clone()), (json!("shared"), json!(true), json!(4)));
    let kit = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    assert!(kit["access"]["read_only"].is_string(), "{kit}");
    let e = api
        .call(&mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    let res = call(&mut api, &mut app, m::TAG_RESOURCES, json!({"tag": "itmc"}));
    assert_eq!(res["raw"], json!([]));
    assert_eq!(res["media"], Value::Null);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_IMPORT, json!({"path": t.path("x.h2tag").display().to_string()}))),
        code::REFUSED
    );
    let back = call(&mut api, &mut app, m::TAG_BROWSE, json!({}));
    assert_eq!(back["map"], "example");
    assert_eq!(back["browsing"], false);

    // Export, edit, import: the edit is gone, as one undo step.
    let out = t.path("out/kit.h2tag");
    let x = call(&mut api, &mut app, m::TAG_EXPORT, json!({"tag": "itmc", "out": out.display().to_string()}));
    assert_eq!(x["tags"], json!(["itmc:objects\\test\\kit"]));
    let before = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}));
    let r = call(&mut api, &mut app, m::TAG_IMPORT, json!({"path": out.display().to_string()}));
    assert_eq!(r["replaced"], json!(["itmc:objects\\test\\kit"]));
    let after = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    assert_eq!(after["fields"], before["fields"]);
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "tags"}));
    let undone = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    assert_ne!(undone["fields"], before["fields"], "undo takes the import back");
    // Never into a Git checkout.
    let repo = t.path("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let e = api
        .call(&mut app, m::TAG_EXPORT, json!({"tag": "itmc", "out": repo.join("k.h2tag").display().to_string()}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
}

struct Colors;
impl Provider for Colors {
    fn methods(&self) -> Vec<MethodDef> {
        vec![MethodDef {
            name: m::MODEL_COLORS,
            summary: "test colours",
            params: "{}",
            result: "object",
            example: "null",
            planned: None,
        }]
    }
    fn call(&mut self, _: &mut App, method: &str, _: &Value) -> Option<Reply> {
        (method == m::MODEL_COLORS).then(|| Ok(json!({"colors": [0, 1, 2, 3]})))
    }
}

#[test]
fn a_registered_provider_answers_a_planned_method() {
    let t = Temp::new("provider");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::MODEL_COLORS, Value::Null)),
        code::NOT_YET
    );
    api.register(Box::new(Colors));
    assert_eq!(
        call(&mut api, &mut app, m::MODEL_COLORS, Value::Null),
        json!({"colors": [0, 1, 2, 3]})
    );
    let listed = call(&mut api, &mut app, m::METHODS, Value::Null);
    let model = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == m::MODEL_COLORS)
        .unwrap();
    assert_eq!(model["status"], "ready");
    // The rest of the namespace stays planned.
    assert_eq!(
        code_of(api.call(&mut app, m::MODEL_EMBLEM, Value::Null)),
        code::NOT_YET
    );
}

/// A headless editor on a socket, as `h2-editor --serve` runs: the app
/// lives on its own thread, which answers the queue.
fn serve(
    sock: PathBuf,
    map: PathBuf,
    data: PathBuf,
) -> (server::Listener, std::thread::JoinHandle<()>) {
    let (listener, rx) = server::listen(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let mut app = App::new();
        app.set_data_root(data);
        let _ = map;
        let mut api = Api::new();
        for inc in rx.iter() {
            let r = api.handle(&mut app, inc.request.clone());
            inc.answer(r);
        }
    });
    (listener, h)
}

#[test]
fn a_headless_editor_opens_sets_saves_and_patches_only_those_bytes() {
    let t = Temp::new("e2e");
    let map = t.path("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    let sock = t.path("s.sock");
    let (listener, _h) = serve(sock.clone(), map.clone(), t.path("data"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the socket is the user's alone");
    }
    let mut c = studio_proto::cli::connect(&sock).unwrap();
    c.set_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let status = c.call(m::STATUS, Value::Null).unwrap();
    assert_eq!(status["window"], false);
    c.call(m::SOURCE_OPEN, json!({"path": map.display().to_string()}))
        .unwrap();
    let got = c
        .call(m::SCENARIO_SET, json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4.5, -2, 8]}))
        .unwrap();
    assert_eq!(got["value"], json!([4.5, -2.0, 8.0]));
    let diff = c.call(m::DOC_DIFF, json!({"doc": "map"})).unwrap();
    let patches = diff["patches"].as_array().unwrap();
    assert!(!patches.is_empty(), "{diff}");
    for p in patches {
        assert_eq!(p["block"], "netgame_flags", "{diff}");
        assert_eq!(p["fields"], json!(["position"]), "{diff}");
    }
    let out = t.path("patched.map");
    let saved = c
        .call(
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": out.display().to_string()}),
        )
        .unwrap();
    assert_eq!(saved["bytes"], fixture::fixture().len());
    // Errors come back as errors, on the same connection.
    let e = c
        .call(m::SCENARIO_GET, json!({"block": "nope", "index": 0}))
        .unwrap_err();
    assert!(
        matches!(e, agent_proto::CallError::Rpc(ref e) if e.code == code::INVALID_PARAMS),
        "{e}"
    );
    drop(c);

    let (a, b) = (fixture::fixture(), std::fs::read(&out).unwrap());
    assert_eq!(a.len(), b.len());
    let flag = fixture::sfile(fixture::FLAGS_AT);
    let changed: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    let body: Vec<usize> = changed.iter().copied().filter(|&i| i >= 0x800).collect();
    let mut want = Vec::new();
    for (k, v) in [4.5f32, -2.0, 8.0].iter().enumerate() {
        let old = 3.0f32.to_le_bytes();
        for (j, (x, y)) in v.to_le_bytes().iter().zip(old).enumerate() {
            if *x != y {
                want.push(flag + 4 * k + j);
            }
        }
    }
    assert_eq!(body, want, "only the position's bytes past the header");
    // In the header, at most the checksum word.
    let header: Vec<usize> = changed.iter().copied().filter(|&i| i < 0x800).collect();
    assert!(
        header.len() <= 4 && header.windows(2).all(|w| w[1] - w[0] < 4),
        "{header:?}"
    );

    // A notification gets no answer; a bad line gets a parse error.
    #[cfg(unix)]
    {
        use std::io::{BufRead, Write};
        let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        s.write_all(b"{\"method\":\"session.status\"}\nnot json\n")
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(s).read_line(&mut line).unwrap();
        let r: agent_proto::Response = serde_json::from_str(&line).unwrap();
        assert_eq!(r.error.unwrap().code, code::PARSE);
    }
    drop(listener);
    assert!(!sock.exists(), "the socket file goes with the listener");
}

#[test]
fn the_socket_is_local_only_and_one_editor_per_path() {
    let t = Temp::new("sock");
    assert!(server::listen(Path::new("tcp://127.0.0.1:4721")).is_err());
    let sock = t.path("one.sock");
    let (_l, _rx) = server::listen(&sock).unwrap();
    let e = server::listen(&sock)
        .err()
        .expect("a second listener is refused");
    assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse);
    let file = t.path("plain");
    std::fs::write(&file, "x").unwrap();
    assert!(
        server::listen(&file).is_err(),
        "a file that isn't a socket stays"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn a_window_answers_between_frames() {
    // The window's loop: pump drains the queue on the app it draws.
    let t = Temp::new("pump");
    let mut app = app(&t);
    let mut api = Api::new();
    api.window = true;
    let sock = t.path("w.sock");
    let (_l, rx) = server::listen(&sock).unwrap();
    let client = std::thread::spawn(move || {
        let mut c = studio_proto::cli::connect(&sock).unwrap();
        c.call(
            m::SCENARIO_MOVE,
            json!({"block": "netgame_flags", "index": 0, "position": [3, 2, 1]}),
        )
        .unwrap();
        c.call(m::STATUS, Value::Null).unwrap()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !client.is_finished() {
        api.pump(&mut app, &rx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let status = client.join().unwrap();
    assert_eq!(status["window"], true);
    assert_eq!(
        flag_position(&app),
        [3.0, 2.0, 1.0],
        "the window's own document changed"
    );
    assert_eq!(
        app.log().last().unwrap().status.text(),
        "moved NetgameFlags 0"
    );
}

/// An app with `hud_edit`'s invented map as the source, open in the HUD
/// tool through the API (no game data).
fn hud_app(t: &Temp) -> (Api, App) {
    let map = t.path("hudtest.map");
    std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    app.dispatch(Action::OpenSource(map));
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::HUD_LIST, json!({"map": "hudtest"}));
    assert_eq!(list["huds"].as_array().unwrap().len(), 2, "{list}");
    assert_eq!(list["current"], "invented\\hud\\player");
    (api, app)
}

/// The fixture's stray bitmap: full-screen offset (40, -48), flagged by
/// the checks.
const STRAY: hud_edit::WidgetKey = hud_edit::WidgetKey {
    kind: hud_edit::WidgetKind::Bitmap,
    index: 2,
};

/// The stray's entry in a hud.read.
fn stray_of(read: &Value) -> Value {
    read["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["widget"] == "bitmap:2")
        .expect("the stray")
        .clone()
}

#[test]
fn hud_read_check_move_set_render_and_save_through_the_hud_tool() {
    let t = Temp::new("hud");
    let (mut api, mut app) = hud_app(&t);
    let read = call(&mut api, &mut app, m::HUD_READ, Value::Null);
    let stray = stray_of(&read);
    assert_eq!((stray["fields"]["x0"].clone(), stray["fields"]["y0"].clone()), (json!(40), json!(-48)));
    assert!(stray["placed"].is_object(), "{stray}");
    assert!(read["fields"]["bitmap"].as_array().unwrap().iter().any(|f| f["key"] == "anchor"));
    // The checks name the stray.
    let found = call(&mut api, &mut app, m::HUD_CHECK, Value::Null);
    assert!(
        found.as_array().unwrap().iter().any(|f| f["widgets"]
            .as_array()
            .unwrap()
            .contains(&json!({"hud": 0, "widget": "bitmap:2"}))),
        "{found}"
    );
    // A move is the HUD tool's undo step and patch.
    let moved = call(&mut api, &mut app, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 60, "y": -48}));
    assert_eq!(moved["offset"], json!([60, -48]));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    assert!(app.tool_dirty(crate::ToolKind::Hud));
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "hud"}));
    assert_eq!(diff["patches"][0]["widget"], "bitmap:2", "{diff}");
    assert_eq!(diff["patches"][0]["fields"], json!(["x0"]));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "hud"}));
    assert_eq!(app.hud.offset(STRAY), Some((40, -48)));
    assert!(!app.tool_dirty(crate::ToolKind::Hud));
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "hud"}));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    // Another mode's offset: the state's mode follows.
    call(&mut api, &mut app, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 21, "y": -24, "mode": "half"}));
    assert_eq!(app.hud.state.mode, hud_edit::Mode::Half);
    let half = call(&mut api, &mut app, m::HUD_READ, Value::Null);
    assert_eq!(half["state"]["mode"], "half");
    assert_eq!(stray_of(&half)["fields"]["x1"], 21, "{half}");
    // Fields by key; string IDs are the Tags tool's; unknown keys say so.
    let set = call(&mut api, &mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "anchor", "value": 3}));
    assert_eq!(set["value"], 3);
    assert_eq!(
        app.hud.session.as_ref().unwrap().hud().unwrap().widget(STRAY).unwrap().anchor(),
        3
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "name", "value": 1}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "nope", "value": 1}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_MOVE, json!({"widget": "bitmap:99", "x": 0, "y": 0}))),
        code::INVALID_PARAMS
    );
    // Opening another map would drop the edits: refused without discard.
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_LIST, json!({"map": "hudtest"}))),
        code::UNSAVED
    );
    // The picture, never into the checkout.
    let png = t.path("out/hud.png");
    let r = call(&mut api, &mut app, m::HUD_RENDER, json!({"out": png.display().to_string(), "state": {"mode": "full", "shield": 0.5}}));
    assert_eq!(r["width"], 640);
    assert!(png.exists());
    assert_eq!(app.hud.state.shield, 0.5);
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("hud.png");
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_RENDER, json!({"out": inside.display().to_string()}))),
        code::REFUSED
    );
    assert!(!inside.exists());
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_RENDER, json!({"state": {"shield": 2}}))),
        code::INVALID_PARAMS
    );
    // The save: never over the source; the saved map has the edits.
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SAVE_AS, json!({"path": t.path("hudtest.map").display().to_string()}))),
        code::REFUSED
    );
    let out = t.path("out/hud.map");
    call(&mut api, &mut app, m::HUD_SAVE_AS, json!({"path": out.display().to_string()}));
    assert!(!app.tool_dirty(crate::ToolKind::Hud));
    // (No scenario in it, so the scenario tool's open logs an error.)
    app.dispatch(Action::OpenSource(out));
    call(&mut api, &mut app, m::HUD_LIST, json!({"map": "hud"}));
    let back = call(&mut api, &mut app, m::HUD_READ, json!({"state": {"mode": "full"}}));
    assert_eq!(stray_of(&back)["fields"]["x0"], 60);
    assert_eq!(stray_of(&back)["fields"]["anchor"], 3);
}

#[test]
fn the_hud_window_and_the_api_make_identical_documents() {
    let t = Temp::new("hud-ui");
    let (_, mut ui) = hud_app(&t);
    // The window's drag: three moves merged, then a field.
    for (i, x) in [41, 45, 60].into_iter().enumerate() {
        ui.dispatch(Action::HudMove(STRAY, (x, -48), i > 0));
    }
    ui.dispatch(Action::HudSet(STRAY, "anchor".into(), hud_edit::Value::Int(3), false));
    let t2 = Temp::new("hud-api");
    let (mut api, mut agent) = hud_app(&t2);
    call(&mut api, &mut agent, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 60, "y": -48}));
    call(&mut api, &mut agent, m::HUD_SET, json!({"widget": "bitmap:2", "field": "anchor", "value": 3}));
    let patches = |a: &App| a.hud.session.as_ref().unwrap().patches();
    assert_eq!(patches(&ui), patches(&agent));
    assert_eq!(
        ui.hud.session.as_ref().unwrap().history().count(),
        agent.hud.session.as_ref().unwrap().history().count()
    );
}

/// The viewport gizmo's drag (its actions, as `viewport.rs` dispatches
/// them) and `scenario.move` make the same map, one undo step each; the
/// playlist text fields' edits (`PlaylistEdit`, as `ui/playlists.rs`
/// applies them) and `playlist.set` the same playlists.
#[test]
fn the_gizmo_and_the_text_fields_go_through_the_api_commands() {
    let t = Temp::new("gizmo");
    let mut ui = app(&t);
    ui.dispatch(Action::Select(Some((BlockKind::NetgameFlags, 0))));
    let ypr = ui.session.selected_transform().unwrap().ypr();
    for position in [[2.0, 2.0, 2.0], [3.0, 4.0, 5.0], [4.0, 5.0, 6.0]] {
        ui.dispatch(Action::GestureSet { position, ypr });
    }
    assert!(ui.session.in_gesture());
    ui.dispatch(Action::EndGesture);
    assert!(!ui.session.in_gesture());
    let t2 = Temp::new("gizmo-api");
    let mut agent = app(&t2);
    let mut api = Api::new();
    call(
        &mut api,
        &mut agent,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [4, 5, 6]}),
    );
    assert_eq!(flag_position(&ui), [4.0, 5.0, 6.0]);
    assert_eq!(
        ui.session.scenario().unwrap().changes(),
        agent.session.scenario().unwrap().changes()
    );
    // The whole drag is one undo step.
    ui.dispatch(Action::Undo);
    call(&mut api, &mut agent, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&ui), flag_position(&agent));
    assert!(!ui.session.is_dirty());

    // The title field: each keystroke's text as a PlaylistEdit, merged
    // under the field's key, then sealed.
    let hop = |t: &Temp| t.path("multiplayer_hopper.dat");
    ui.dispatch(Action::OpenHopper(hop(&t)));
    for text in ["T", "Te", "Team"] {
        let e = PlaylistEdit::Title {
            playlist: 2,
            text: text.into(),
        };
        ui.hoppers.edit_with(|hs, _| (e.apply(hs).unwrap(), Some(7)));
    }
    ui.hoppers.seal();
    let long = "x".repeat(200);
    ui.hoppers.edit(|hs| {
        PlaylistEdit::Description {
            playlist: 2,
            text: long.clone(),
        }
        .apply(hs)
        .unwrap()
    });
    call(
        &mut api,
        &mut agent,
        m::DOC_OPEN,
        json!({"playlists": hop(&t2).display().to_string()}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_SET,
        json!({"playlist": 2, "title": "Team", "description": long}),
    );
    assert_eq!(ui.hoppers.hoppers(), agent.hoppers.hoppers());
    // Clipped to the format's room by the edit, not the widget.
    let d = &ui.hoppers.hoppers().unwrap()[2].description;
    assert!(d.len() < 200, "{}", d.len());
}

#[test]
fn placement_chain_uses_the_current_tag_document_and_stops_at_cycles() {
    let t = Temp::new("placement-chain");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    let placement = json!({"block": "weapons", "index": 0});
    let chain = call(&mut api, &mut app, m::TAG_CHAIN, placement.clone());
    let object = &chain["root"];
    assert_eq!(object["group"], "weap");
    let model = &object["children"][0];
    assert_eq!(model["group"], "hlmt");
    assert_eq!(model["via"], "model");
    let render = &model["children"][0];
    assert_eq!(render["group"], "mode");
    assert_eq!(render["via"], "render_model");
    assert_eq!(render["children"][0]["tag"], object["tag"]);
    assert_eq!(render["children"][0]["cycle"], true);
    assert_eq!(render["children"][0]["children"], json!([]));
    assert_eq!(object["children"][1]["via"], "parts[0].item");
    assert_eq!(object["children"][1]["label"], Value::Null);

    let shallow = call(&mut api, &mut app, m::TAG_CHAIN, json!({"tag":"weap", "depth":0}));
    assert_eq!(shallow["root"]["tag"], object["tag"]);
    assert_eq!(shallow["root"]["children"], json!([]));
    let spawn = call(&mut api, &mut app, m::TAG_CHAIN, json!({"block":"starting_locations", "index":0}));
    assert!(spawn["root"].is_null());
    call(&mut api, &mut app, m::TAG_SET, json!({"tag":"hlmt", "path":"render_model", "value":null}));
    let edited = call(&mut api, &mut app, m::TAG_CHAIN, placement);
    assert_eq!(edited["root"]["children"][0]["children"], json!([]));
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_CHAIN, json!({"block":"weapons", "tag":"weap"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn placement_inspector_and_tag_set_make_identical_documents_and_saves() {
    let window = Temp::new("inspector-tag");
    let socket = Temp::new("inspector-api");
    let mut ui = crate::app::tag_tests::placement_app(&window.0);
    let mut agent = crate::app::tag_tests::placement_app(&socket.0);
    let mut api = Api::new();
    ui.dispatch(Action::Select(Some((BlockKind::Weapons, 0))));
    let scenario = ui.session.scenario().unwrap();
    let root = crate::app::tag_api::placement_root(scenario, BlockKind::Weapons, 0).unwrap().0;
    // The same byte write the speed widget emits at the end of a drag.
    let t = ui.tags.tag(root).unwrap();
    let f = &ui.tags.layout(root).unwrap().fields[0];
    let action = crate::fields::write_action(root, t.address + f.offset, 4.25f32.to_le_bytes().to_vec(), &f.name);
    ui.dispatch(crate::ui::inspector::tag_action(action));
    ui.dispatch(crate::ui::inspector::tag_action(Action::Tag(crate::app::TagAction::Seal)));
    call(&mut api, &mut agent, m::TAG_SET, json!({"tag":"weap", "path":"speed", "value":4.25}));

    assert_eq!(ui.tags.patches(), agent.tags.patches());
    assert!(!ui.tags.patches().is_empty());
    assert_eq!(ui.session.scenario().unwrap().changes(), agent.session.scenario().unwrap().changes());
    let window_save = window.path("edited.map");
    let socket_save = socket.path("edited.map");
    ui.perform(Action::SaveMapAs(window_save.clone()), false).unwrap();
    call(&mut api, &mut agent, m::DOC_SAVE_AS, json!({"doc":"map", "path":socket_save}));
    assert_eq!(std::fs::read(window_save).unwrap(), std::fs::read(socket_save).unwrap());
}

#[test]
fn back_and_forward_restore_the_tags_document_selection() {
    use crate::app::NavAction;

    let t = Temp::new("tag-navigation");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    let weapon = fixture::id(2);
    let model = fixture::id(4);
    for tag in [weapon, model] {
        app.perform(Action::Navigate(NavAction::SelectTag(tag)), false)
            .unwrap();
        assert_eq!(app.tags.selected, Some(tag));
    }

    call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(app.tags.selected, Some(weapon));
    call(&mut api, &mut app, m::NAV_FORWARD, json!({}));
    assert_eq!(app.tags.selected, Some(model));
}

#[test]
fn reference_jump_and_history_restore_the_tag_and_field() {
    let t = Temp::new("inspector-nav");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    let weapon = fixture::id(2);
    let model = fixture::id(4);
    let render = fixture::id(5);
    let pick = json!({"block":"weapons", "index":0});
    let before = call(&mut api, &mut app, m::SCENARIO_SELECT, pick.clone());
    assert_eq!(before["selected"], pick);

    let jump = crate::fields::reference_action(Some(&[weapon]), "", "model", model);
    app.perform(crate::ui::inspector::tag_action(jump), false).unwrap();
    let arrived = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    assert_eq!(arrived["selected"], pick);
    assert_eq!(arrived["tag"], scenario::tag_id(model));
    assert_eq!(arrived["breadcrumbs"][1]["tag"], scenario::tag_id(weapon));
    assert_eq!(arrived["breadcrumbs"][2]["tag"], scenario::tag_id(model));

    let back = call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(back["tag"], scenario::tag_id(weapon));
    assert_eq!(back["path"], "model");
    assert!(back["can_forward"].as_bool().unwrap());
    assert_eq!(call(&mut api, &mut app, m::NAV_FORWARD, json!({})), arrived);
    assert_eq!(app.session.selected(), Some((BlockKind::Weapons, 0)));

    let to_render = call(&mut api, &mut app, m::NAV_GO, json!({
        "tag": "mode", "path": "example_back_reference"
    }));
    assert_eq!(to_render["tag"], scenario::tag_id(render));
    assert_eq!(to_render["breadcrumbs"].as_array().unwrap().len(), 4);
    let block_field = call(&mut api, &mut app, m::NAV_GO, json!({
        "tag": "weap", "path": "parts[1].value"
    }));
    assert_eq!(block_field["path"], "parts[1].value");
    call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(call(&mut api, &mut app, m::NAV_FORWARD, json!({})), block_field);
    let focus = crate::app::tag_api::field_focus(&app.tags, weapon, "parts[1].value").unwrap();
    assert_eq!(focus.blocks.len(), 1);
    assert_eq!(focus.blocks[0].1, 1);

    call(&mut api, &mut app, m::TAG_SET, json!({"tag":"weap", "path":"speed", "value":8}));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc":"tags"}));
    assert_eq!(call(&mut api, &mut app, m::NAV_STATUS, json!({})), block_field);

    assert_eq!(
        code_of(api.call(&mut app, m::NAV_GO, json!({"tag":"weap", "path":"parts[2].value"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(call(&mut api, &mut app, m::NAV_STATUS, json!({})), block_field);
}

#[test]
fn viewport_pick_pushes_one_history_entry() {
    let t = Temp::new("pick-history");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    call(&mut api, &mut app, m::SCENARIO_SELECT, json!({"tag":"mode"}));
    let before = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    let ray = scenario_edit::geometry::Ray {
        origin: [0.0, 1.0, 12.0],
        dir: [0.0, 0.0, -1.0],
    };
    let pick = crate::viewport::pick_action(
        &app.session, &ray, false, &app.layers, &Default::default(),
    ).unwrap();
    assert_eq!(pick, Action::Select(Some((BlockKind::Weapons, 0))));
    app.dispatch(pick.clone());
    let after = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    assert_eq!(after["selected"], json!({"block":"weapons", "index":0}));
    app.dispatch(pick);
    let mut back = call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(back["can_forward"], true);
    back["can_forward"] = json!(false);
    assert_eq!(back, before);
    assert_eq!(call(&mut api, &mut app, m::NAV_FORWARD, json!({})), after);
}

#[cfg(unix)]
mod mirror_presence {
    use super::*;
    use crate::play::{Conn, Play, select::EYE_HEIGHT, testing::fake};
    use agent_proto::{editor, live, sim};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    const WEAPON: u32 = 0x0010_0003;

    fn game(tag: &str, weapon: u32, window: Option<bool>) -> fake::Game {
        let game = fake::Game::start(tag, "example", sim::SessionKind::Local);
        game.edits.lock().unwrap().extra_object = Some(live::Object {
            id: weapon,
            tag: Some(fixture::id(2).0),
            kind: 2,
            placement: Some(0),
            position: [10.0, -1.0, 2.0 + EYE_HEIGHT],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            scale: 1.0,
            ..Default::default()
        });
        if let Some(window) = window {
            game.presentation(window);
        }
        game
    }

    fn connect(api: &mut Api, app: &mut App, play: &mut Play, game: &fake::Game) {
        play.update(app);
        call(api, app, m::PAIR_CONNECT, json!({"socket": game.path}));
        fake::until(
            play,
            app,
            "the local player and the matching map",
            |p, a| {
                p.edits_blocked(a).as_deref() == Some("View mode")
                    && a.pairing.shown.as_ref().is_some_and(|s| {
                        a.pairing.matches_sample(a, s)
                            && !s.stale
                            && !s.players.is_empty()
                            && s.objects.iter().any(|o| o.placement == Some(0))
                    })
            },
        );
    }

    fn marked(params: &Value, style: &str, object: u32) -> bool {
        let handle = format!("{object:08x}");
        params["marks"].as_array().is_some_and(|marks| {
            marks
                .iter()
                .any(|m| m["style"] == style && m["object"].as_str() == Some(handle.as_str()))
        })
    }

    #[test]
    fn mirror_pick_queries_without_selecting_and_aim_select_marks_the_dwell() {
        let t = Temp::new("presence-aim");
        let game = game("presence-aim", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let before = app.session.selected();
        let pick = call(&mut api, &mut app, m::MIRROR_PICK, json!({"from":"aim"}));
        assert_eq!(pick["object"], sim::objects::handle(WEAPON));
        assert_eq!(pick["placement"], json!({"block":"weapons","index":0}));
        assert_eq!(pick["tag"], sim::objects::handle(fixture::id(2).0));
        assert!(pick["distance"].as_f64().unwrap() > 0.);
        // An explicit ray includes the player's unit. Start this one beyond it.
        let ray = json!({"ray":{"origin":[10.,-2.,2. + EYE_HEIGHT], "dir":[0.,1.,0.]}});
        let hit = call(&mut api, &mut app, m::MIRROR_PICK, ray);
        assert_eq!(hit["object"], pick["object"]);
        assert_eq!(hit["placement"], pick["placement"]);
        assert!(hit["distance"].as_f64().unwrap() < pick["distance"].as_f64().unwrap());
        let miss = json!({"ray":{"origin":[99.,99.,99.],"dir":[0.,0.,1.]}});
        assert!(call(&mut api, &mut app, m::MIRROR_PICK, miss).is_null());
        assert_eq!(app.session.selected(), before);

        call(
            &mut api,
            &mut app,
            m::MIRROR_AIM_SELECT,
            json!({"enabled":true}),
        );
        fake::until(
            &mut play,
            &mut app,
            "candidate and selected marks",
            |_, a| {
                a.session.selected() == Some((BlockKind::Weapons, 0))
                    && game
                        .calls(editor::method::MARKS)
                        .iter()
                        .any(|p| marked(p, "candidate", WEAPON))
            },
        );
        let cursor = call(&mut api, &mut app, m::MIRROR_AIM_SELECT, json!({}));
        assert_eq!(cursor["enabled"], true);
        assert_eq!(cursor["candidate"]["object"], pick["object"]);
        fake::until(&mut play, &mut app, "aim indicator", |_, _| {
            game.calls(editor::method::LINK)
                .iter()
                .any(|p| p["mode"] == "view" && p["text"] == "Aim select")
        });
        call(
            &mut api,
            &mut app,
            m::MIRROR_AIM_SELECT,
            json!({"enabled":false}),
        );
        fake::until(
            &mut play,
            &mut app,
            "selection without the candidate",
            |_, _| {
                game.calls(editor::method::MARKS).last().is_some_and(|p| {
                    p["marks"].as_array().is_some_and(|m| m.len() == 1)
                        && marked(p, "selected", WEAPON)
                })
            },
        );
    }

    #[test]
    fn a_placement_issue_opens_its_row_after_picking_a_loose_live_object() {
        use crate::play::select;

        let t = Temp::new("presence-issue-nav");
        let game = game("presence-issue-nav", WEAPON, None);
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let row = json!({"block":"netgame_flags", "index":0});
        call(&mut api, &mut app, m::SCENARIO_MOVE, json!({
            "block":"netgame_flags", "index":0, "position":[1., 2., 3.]
        }));
        game.edits.lock().unwrap().extra_object.as_mut().unwrap().placement = None;
        fake::until(&mut play, &mut app, "loose weapon and pending flag edit", |_, a| {
            select::from_aim(a).is_some_and(|hit| hit.object == WEAPON && hit.placement.is_none())
                && a.pairing.pending.iter().any(|r| r["target"] == row)
        });
        let hit = select::from_aim(&app).unwrap();
        select::apply(&mut app, hit, false);
        assert_eq!(select::selected_object(&app), Some(WEAPON));
        assert_eq!(app.selection(), Some(Selection::Tag(fixture::id(2))));

        let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
        let issue = status["link"]["issues"].as_array().unwrap().iter()
            .find(|issue| issue["target"] == row).unwrap();
        let nav = call(&mut api, &mut app, m::NAV_GO, issue["target"].clone());
        assert_eq!(nav["selected"], row);
        assert_eq!(select::selected_object(&app), None);
    }

    #[test]
    fn placement_marks_renew_in_view_and_clear_on_unpair() {
        let t = Temp::new("presence-lease");
        let game = game("presence-lease", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        let start = Instant::now();
        fake::presence_at(&mut play, &mut app, start);
        let first = game.calls(editor::method::MARKS);
        assert_eq!(first.len(), 1);
        assert!(marked(&first[0], "selected", WEAPON));
        let deadline = start + Duration::from_secs(1);
        for when in [start, start + Duration::from_millis(300), deadline - Duration::from_nanos(1)] {
            fake::presence_at(&mut play, &mut app, when);
            assert_eq!(game.calls(editor::method::MARKS), first);
        }
        fake::presence_at(&mut play, &mut app, deadline);
        assert_eq!(game.calls(editor::method::MARKS), [first[0].clone(), first[0].clone()]);
        // View mode keeps presentation after the edit switch is turned off.
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":true}));
        fake::until(&mut play, &mut app, "edit indicator", |_, _| {
            game.calls(editor::method::LINK)
                .last()
                .is_some_and(|p| p["mode"] == "edit")
        });
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":false}));
        fake::until(&mut play, &mut app, "view indicator", |_, _| {
            game.calls(editor::method::LINK)
                .last()
                .is_some_and(|p| p["mode"] == "view")
        });
        assert!(marked(
            game.calls(editor::method::MARKS).last().unwrap(),
            "selected",
            WEAPON
        ));

        call(&mut api, &mut app, m::MIRROR_CAMERA, json!({"mode":"eyes"}));
        call(
            &mut api,
            &mut app,
            m::MIRROR_AIM_SELECT,
            json!({"enabled":true}),
        );
        call(
            &mut api,
            &mut app,
            m::PAIR_DISCONNECT,
            json!({"leave":"keep"}),
        );
        fake::until(&mut play, &mut app, "release of both leases", |p, _| {
            p.conn == Conn::Off
                && game
                    .calls(editor::method::LINK)
                    .last()
                    .is_some_and(|p| p["clear"] == true)
                && game
                    .calls(editor::method::MARKS)
                    .last()
                    .is_some_and(|p| p["marks"] == json!([]))
        });
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_CAMERA, json!({})),
            json!({"mode":"free","target":null})
        );
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_AIM_SELECT, json!({})),
            json!({"enabled":false,"candidate":null})
        );
    }

    #[test]
    fn a_rejected_clear_retries_at_the_deadline_and_on_close() {
        for close in [false, true] {
            let name = if close { "marks-close" } else { "marks-retry" };
            let t = Temp::new(name);
            let game = game(name, WEAPON, Some(true));
            let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
            connect(&mut api, &mut app, &mut play, &game);
            call(&mut api, &mut app, m::SCENARIO_SELECT, json!({"block":"weapons","index":0}));
            let now = Instant::now();
            fake::presence_at(&mut play, &mut app, now);
            game.edits.lock().unwrap().mark_replies.push_back(Err(
                agent_proto::RpcError::new(agent_proto::code::UNAVAILABLE, "fixture clear failed"),
            ));
            call(&mut api, &mut app, m::SCENARIO_SELECT, json!({}));
            fake::presence_at(&mut play, &mut app, now);
            let failed = game.calls(editor::method::MARKS);
            assert_eq!(failed.len(), 2);
            assert!(marked(&failed[0], "selected", WEAPON));
            assert_eq!(failed[1]["marks"], json!([]));
            assert!(play.link_view(&app).unavailable.iter().any(|s| s.contains("fixture clear failed")));
            let due = now + Duration::from_secs(1);
            fake::presence_at(&mut play, &mut app, due - Duration::from_nanos(1));
            assert_eq!(game.calls(editor::method::MARKS), failed);
            if close {
                play.unpair(&mut app);
                fake::until(&mut play, &mut app, "final clear", |_, _| {
                    game.calls(editor::method::MARKS).len() == 3
                });
            } else {
                fake::presence_at(&mut play, &mut app, due);
                assert!(!play.link_view(&app).unavailable.iter().any(|s| s.contains("fixture clear failed")));
                fake::presence_at(&mut play, &mut app, due + Duration::from_secs(3));
            }
            let sent = game.calls(editor::method::MARKS);
            assert_eq!(sent.len(), 3);
            assert_eq!(sent[2]["marks"], json!([]));
        }
    }

    #[test]
    fn a_held_set_renews_until_drawn_and_reports_refused_indices_independently() {
        let t = Temp::new("marks-camera");
        let game = game("marks-camera", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        app.session.select_many(vec![(BlockKind::Weapons, 0), (BlockKind::NetgameFlags, 0)]);
        // The weapon has both a selected and a candidate mark. The flag has a
        // fixed box which the host still holds if the weapon disappears.
        let start = Instant::now();
        call(&mut api, &mut app, m::MIRROR_AIM_SELECT, json!({"enabled":true}));
        let why = "no camera for the presented frame";
        let refused = json!([
            {"index":0,"why":"object is gone"}, {"index":2,"why":"object is gone"}
        ]);
        game.edits.lock().unwrap().mark_replies.extend([
            Ok(json!({"held":1,"drawn":false,"why":why,"refused":refused})),
            Ok(json!({"held":1,"drawn":true,"refused":refused})),
        ]);
        fake::presence_at(&mut play, &mut app, start);
        let first = game.calls(editor::method::MARKS);
        assert_eq!(first.len(), 1);
        let marks = first[0]["marks"].as_array().unwrap();
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[0]["object"], format!("{WEAPON:08x}"));
        assert!(marks[1]["box"].is_object());
        assert_eq!(marks[2]["style"], "candidate");
        let waiting = format!("editor.marks: {why}");
        let refused_lines = [0, 2].map(|i| format!("editor.marks: mark {i}: object is gone"));
        let view = play.link_view(&app);
        assert!(view.unavailable.contains(&waiting));
        assert!(refused_lines.iter().all(|line| view.unavailable.contains(line)));
        assert!(!app.log().iter().any(|l| l.status.text() == waiting.as_str()));

        fake::presence_at(&mut play, &mut app, start + Duration::from_secs(1));
        let renewed = game.calls(editor::method::MARKS);
        assert_eq!(renewed, [first[0].clone(), first[0].clone()]);
        let view = play.link_view(&app);
        assert!(!view.unavailable.contains(&waiting));
        assert!(refused_lines.iter().all(|line| view.unavailable.contains(line)));
        for reason in &refused_lines {
            assert_eq!(app.log().iter().filter(|l| l.status.text() == reason.as_str()).count(), 1);
        }
    }

    #[test]
    fn mirror_player_faces_the_camera_in_edit_and_refuses_view() {
        let t = Temp::new("presence-player");
        let game = game("presence-player", WEAPON, None);
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let move_here = json!({"to":[1.,2.,3.],"facing":true});
        let refusal = api.call(&mut app, m::MIRROR_PLAYER, move_here.clone()).unwrap_err();
        assert_eq!(refusal.code, code::REFUSED);
        assert_eq!(refusal.message, "View mode");
        let reports = |app: &App| {
            app.log().iter().filter(|line| line.status.text() == "View mode").count()
        };
        play.update(&mut app);
        assert_eq!(reports(&app), 1);
        assert_eq!(
            call(&mut api, &mut app, m::PAIR_STATUS, Value::Null)["link"]["issues"],
            json!([{"kind":"refused", "text":"View mode", "target":{"panel":"pending"}}])
        );
        assert!(app.pairing.pending.is_empty());
        assert_eq!(
            play.toasts(std::time::Instant::now())
                .filter(|(why, problem)| *problem && *why == refusal.message.as_str())
                .count(),
            1
        );
        assert!(game.calls(sim::method::STATE_SET).is_empty());

        let camera = app.session.camera_mut();
        camera.yaw = 0.41;
        camera.pitch = -0.32;
        let forward = camera.forward().map(f64::from);
        let (yaw, pitch) = sim::yaw_pitch(forward).unwrap();
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":true}));
        fake::until(&mut play, &mut app, "Edit", |p, a| {
            p.edits_blocked(a).is_none()
        });
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_PLAYER, move_here),
            json!({"requested":true})
        );
        fake::until(&mut play, &mut app, "teleport with facing", |_, _| {
            !game.calls(sim::method::STATE_SET).is_empty()
        });
        let moves = game.calls(sim::method::STATE_SET);
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0]["player"], "player.0");
        assert_eq!(moves[0]["pos"], json!([1., 2., 3.]));
        assert!((moves[0]["yaw"].as_f64().unwrap() - yaw).abs() < 1e-4);
        assert!((moves[0]["pitch"].as_f64().unwrap() - pitch).abs() < 1e-4);

        // The last published status is still Edit when the two socket calls arrive.
        // The ordered window requests turn off Edit before the move reaches the game.
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":false}));
        call(
            &mut api,
            &mut app,
            m::MIRROR_PLAYER,
            json!({"to":[9.,8.,7.]}),
        );
        fake::until(
            &mut play,
            &mut app,
            "the queued move refused in View",
            |_, a| reports(a) == 2,
        );
        assert_eq!(game.calls(sim::method::STATE_SET).len(), 1);
    }

    #[test]
    fn absent_presentation_methods_are_named_and_receive_no_requests() {
        let t = Temp::new("presence-methods");
        let game = game("presence-methods", WEAPON, None);
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
        let reasons = status["link"]["unavailable"].as_array().unwrap();
        for method in [editor::method::LINK, editor::method::MARKS] {
            assert!(reasons.iter().any(|r| r.as_str().unwrap().contains(method)));
        }
        // Install recording handlers after the handshake. They are callable on
        // the transport, but the editor has never seen them in session.methods.
        game.presentation(true);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":true}));
        fake::until(&mut play, &mut app, "edit switch", |p, a| {
            p.edits_blocked(a).is_none()
        });
        call(
            &mut api,
            &mut app,
            m::MIRROR_PLAYER,
            json!({"to":[7.,8.,9.]}),
        );
        fake::until(&mut play, &mut app, "ordered game request", |_, _| {
            !game.calls(sim::method::STATE_SET).is_empty()
        });
        assert!(game.calls(editor::method::LINK).is_empty());
        assert!(game.calls(editor::method::MARKS).is_empty());
    }

    #[test]
    fn headless_presentation_explains_the_refusal_once() {
        let t = Temp::new("presence-headless");
        let game = game("presence-headless", WEAPON, Some(false));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        fake::until(
            &mut play,
            &mut app,
            "another mark lease on the windowless host",
            |_, _| {
                game.calls(editor::method::MARKS)
                    .iter()
                    .filter(|p| marked(p, "selected", WEAPON))
                    .count()
                    >= 2
            },
        );
        let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
        let reasons = status["link"]["unavailable"].as_array().unwrap();
        for method in [editor::method::LINK, editor::method::MARKS] {
            let reason = format!("{method}: this host has no window");
            assert!(reasons.contains(&json!(reason)));
            assert_eq!(
                app.log()
                    .iter()
                    .filter(|line| line.status.text().contains(reason.as_str()))
                    .count(),
                1
            );
        }
        assert_eq!(game.calls(editor::method::LINK).len(), 1);
    }

    #[test]
    fn epoch_changes_release_the_shared_camera_and_replace_salted_marks() {
        let t = Temp::new("presence-epoch");
        let game = game("presence-epoch", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        fake::until(&mut play, &mut app, "old placement", |_, _| {
            game.calls(editor::method::MARKS)
                .last()
                .is_some_and(|p| marked(p, "selected", WEAPON))
        });
        for mode in ["follow", "eyes"] {
            let camera = call(&mut api, &mut app, m::MIRROR_CAMERA, json!({"mode":mode}));
            assert_eq!(camera["mode"], mode);
            assert_eq!(camera["target"], "player.0");
            play.update(&mut app);
        }
        let new_handle = WEAPON + 0x10000;
        {
            let _hold = game.boundary.lock().unwrap();
            game.edits.lock().unwrap().extra_object.as_mut().unwrap().id = new_handle;
            game.bsp.store(1, Ordering::Relaxed);
        }
        fake::until(
            &mut play,
            &mut app,
            "marks from the replacement epoch",
            |_, a| {
                a.pairing.shown.as_ref().is_some_and(|s| s.epoch == 2)
                    && game.calls(editor::method::MARKS).last().is_some_and(|p| {
                        marked(p, "selected", new_handle) && !marked(p, "selected", WEAPON)
                    })
            },
        );
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_CAMERA, json!({}))["mode"],
            "free"
        );
    }

    #[test]
    fn reconnect_sends_the_selection_from_the_new_stream() {
        let t = Temp::new("presence-retry");
        let first = game("presence-retry", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &first);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        fake::until(&mut play, &mut app, "first game's selection", |_, _| {
            first
                .calls(editor::method::MARKS)
                .last()
                .is_some_and(|p| marked(p, "selected", WEAPON))
        });
        drop(first);
        fake::until(&mut play, &mut app, "lost named socket", |p, _| {
            matches!(p.conn, Conn::Reconnecting { .. })
        });
        let new_handle = WEAPON + 0x20000;
        let second = game("presence-retry", new_handle, Some(true));
        fake::until(&mut play, &mut app, "reconnected presentation", |p, _| {
            p.is_paired()
                && !second.calls(editor::method::LINK).is_empty()
                && second
                    .calls(editor::method::MARKS)
                    .iter()
                    .any(|p| marked(p, "selected", new_handle))
        });
        assert!(
            !second
                .calls(editor::method::MARKS)
                .iter()
                .any(|p| marked(p, "selected", WEAPON))
        );
    }

    #[test]
    fn mirror_controls_cross_the_editor_and_game_sockets() {
        let t = Temp::new("presence-wire");
        let (game, mut editor) = paired_socket::Editor::paired(&t, "presence-wire");
        editor.wait("the matching player sample", |_, a| {
            a.pairing
                .shown
                .as_ref()
                .is_some_and(|s| a.pairing.matches_sample(a, s) && !s.players.is_empty())
        });
        assert!(editor.call(m::MIRROR_PICK, json!({"from":"aim"})).is_null());
        assert_eq!(
            editor.call(m::MIRROR_AIM_SELECT, json!({"enabled":true}))["enabled"],
            true
        );
        let camera = editor.call(m::MIRROR_CAMERA, json!({"mode":"eyes","target":"player.0"}));
        assert_eq!(camera, json!({"mode":"eyes","target":"player.0"}));
        editor.call(m::PAIR_EDIT, json!({"enabled":true}));
        editor.wait("Edit", |p, a| p.edits_blocked(a).is_none());
        let moved = editor.call(m::MIRROR_PLAYER, json!({"to":[2.,3.,4.]}));
        assert_eq!(moved, json!({"requested":true}));
        editor.wait("the game's player move", |_, _| {
            !game.calls(sim::method::STATE_SET).is_empty()
        });
        let moves = game.calls(sim::method::STATE_SET);
        assert_eq!(moves[0]["pos"], json!([2., 3., 4.]));
        assert!(moves[0].get("yaw").is_none() && moves[0].get("pitch").is_none());
    }
}

#[cfg(unix)]
mod windowless_serve {
    use super::*;
    use crate::app::tag_tests;
    use crate::play::Play;
    use crate::play::testing::fake::Game;
    use agent_proto::sim::SessionKind;
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    struct Served {
        client: agent_proto::Client,
        rx: Receiver<server::Incoming>,
        api: Api,
        app: App,
        play: Play,
    }

    impl Served {
        /// A socket call answered by `Api::serve_once`, as `h2-editor --serve` runs.
        fn call(&mut self, method: &str, params: Value) -> Value {
            let client = &mut self.client;
            std::thread::scope(|scope| {
                let request = scope.spawn(move || client.call(method, params));
                while !request.is_finished() {
                    assert!(self.api.serve_once(
                        &mut self.app,
                        &mut self.play,
                        &self.rx,
                        Duration::from_millis(2),
                    ));
                }
                request.join().unwrap().unwrap()
            })
        }
    }

    #[test]
    fn a_windowless_serve_pairs_and_answers_from_the_mirror() {
        let t = Temp::new("serve-pair");
        let game = Game::start("serve-pair", "example", SessionKind::Local);
        let (mut app, ..) = tag_tests::app(&t.0);
        app.pairing.auto_pair = false;
        let sock = t.path("serve.sock");
        let (_listener, rx) = server::listen(&sock).unwrap();
        let client = studio_proto::cli::connect(&sock).unwrap();
        client.set_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut play = Play::new();
        // As `serve` starts: the game link has published before the first request.
        play.update(&mut app);
        let mut served = Served {
            client,
            rx,
            api: Api::new(),
            app,
            play,
        };
        assert!(!served.api.window);
        let socket = game.path.display().to_string();
        served.call(m::PAIR_CONNECT, json!({"socket": socket}));
        let end = Instant::now() + Duration::from_secs(25);
        loop {
            let objects = served.call(m::MIRROR_OBJECTS, json!({}));
            if objects["objects"].as_array().is_some_and(|o| !o.is_empty()) {
                break;
            }
            assert!(Instant::now() < end, "no streamed objects: {objects}");
        }
        assert_eq!(
            served.call(m::PAIR_STATUS, Value::Null)["state"],
            "connected"
        );
        let status = served.call(m::MIRROR_STATUS, json!({}));
        assert_eq!(status["stream"]["state"], "streaming");
        assert!(status["store"]["tick"].is_u64());
        let control = served.call(m::MIRROR_CONTROL, json!({"op":"pause"}));
        assert_eq!(control["requested"], true);
    }
}

/// Open the invented textured map's floor cluster through the API.
fn textured(t: &Temp) -> (Api, App, PathBuf) {
    textured_from(t, fixture::fixture_with_textured_bsp())
}

/// [`textured`] on the map whose structure has a level collision and a
/// portal a stroke rebuilds (`fixture::fixture_with_textured_level`).
fn textured_level(t: &Temp) -> (Api, App, PathBuf) {
    textured_from(t, fixture::fixture_with_textured_level())
}

fn textured_from(t: &Temp, bytes: Vec<u8>) -> (Api, App, PathBuf) {
    let mut app = app(t);
    let mut api = Api::new();
    let map = t.path("textured.map");
    std::fs::write(&map, bytes).unwrap();
    call(&mut api, &mut app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
    let open = call(&mut api, &mut app, m::GEOMETRY_OPEN, json!({"target": "cluster:e1000004:0"}));
    assert_eq!((open["verts"].clone(), open["faces"].clone()), (json!(4), json!(2)));
    (api, app, map)
}

/// Select the floor's `(4, 4, 0)` corner and lift it by 1.
fn lift_corner(api: &mut Api, app: &mut App) -> Value {
    let read = call(api, app, m::GEOMETRY_READ, Value::Null);
    let id = read["verts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["position"] == json!([4.0, 4.0, 0.0]))
        .unwrap()["id"]
        .clone();
    call(api, app, m::MESH_SELECT, json!({"ids": [id]}));
    call(api, app, m::MESH_TRANSFORM, json!({"translate": [0.0, 0.0, 1.0]}))
}

#[test]
fn geometry_lists_the_floor_cluster_and_an_unedited_target_saves_nothing() {
    let t = Temp::new("geometry-list");
    let (mut api, mut app, _) = textured(&t);
    let list = call(&mut api, &mut app, m::GEOMETRY_LIST, Value::Null);
    let c = list
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["target"] == "cluster:e1000004:0")
        .unwrap()
        .clone();
    assert_eq!(
        (c["kind"].clone(), c["vertices"].clone(), c["triangles"].clone(), c["open"].clone()),
        (json!("cluster"), json!(4), json!(2), json!(true))
    );
    call(&mut api, &mut app, m::MESH_SELECT, json!({"all": true}));
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["targets"][0]["class"], "unchanged");
    assert_eq!(check["save_refusal"], Value::Null);
    assert!(!app.session.is_dirty());
}

#[test]
fn mesh_sculpt_is_one_logged_undo_step_and_a_bad_stroke_is_refused_by_name() {
    let t = Temp::new("mesh-sculpt");
    let (mut api, mut app, _) = textured_level(&t);
    let r = call(
        &mut api,
        &mut app,
        m::MESH_SCULPT,
        json!({"brush": "raise", "centre": [2.0, 2.0, 0.0], "radius": 1.5, "strength": 0.5, "detail": 0.5}),
    );
    // Refined where the brush is, so the floor is a grown write.
    assert_eq!(r["class"], "grown_raw");
    assert!(r["verts"].as_u64().unwrap() > 4, "{r}");
    let read = call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null);
    let top = read["verts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["position"][2].as_f64().unwrap())
        .fold(f64::MIN, f64::max);
    assert!((top - 0.5).abs() < 1e-4, "the centre is raised by the strength: {top}");
    let ops = call(&mut api, &mut app, m::MESH_OPS, Value::Null);
    assert_eq!(ops["ops"].as_array().unwrap().len(), 1);
    assert_eq!((ops["ops"][0]["op"].clone(), ops["ops"][0]["brush"].clone()), (json!("sculpt"), json!("raise")));
    // The same stroke through mesh.apply is the same mesh.
    let log = ops["ops"].clone();
    // A path, a cut and a falloff by name.
    call(
        &mut api,
        &mut app,
        m::MESH_SCULPT,
        json!({"brush": "cut", "points": [[0.5, 3.0, 0.0], [1.0, 3.0, 0.0]], "radius": 0.4, "falloff": "constant", "detail": 0.5}),
    );
    // Each stroke is one step of the map's undo.
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    let ops = call(&mut api, &mut app, m::MESH_OPS, Value::Null);
    assert_eq!(ops["ops"], log);
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    let read = call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null);
    assert_eq!(read["verts"].as_array().unwrap().len(), 4);
    call(&mut api, &mut app, m::MESH_APPLY, json!({"ops": log}));
    let again = call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null);
    assert_eq!(again["verts"].as_array().unwrap().len() as u64, r["verts"].as_u64().unwrap());
    // Refused with the kernel's reason, and nothing logged.
    for (params, why) in [
        (json!({"brush": "raise", "centre": [2.0, 2.0, 0.0], "radius": 0.0}), "the radius must be above 0"),
        (json!({"brush": "cut", "centre": [40.0, 40.0, 0.0], "radius": 1.0}), "the cut reaches no face"),
    ] {
        let e = api.call(&mut app, m::MESH_SCULPT, params).unwrap_err();
        assert_eq!(e.code, code::REFUSED, "{e}");
        assert!(e.message.contains(why), "{e}");
    }
    let e = api.call(&mut app, m::MESH_SCULPT, json!({"brush": "pinch", "centre": [0.0, 0.0, 0.0], "radius": 1.0})).unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    let ops = call(&mut api, &mut app, m::MESH_OPS, Value::Null);
    assert_eq!(ops["ops"].as_array().unwrap().len(), 1);
}

#[test]
fn a_geometry_edit_saves_only_with_the_unverified_switch_on() {
    let t = Temp::new("geometry-save");
    let (mut api, mut app, map) = textured(&t);
    let moved = lift_corner(&mut api, &mut app);
    assert_eq!((moved["class"].clone(), moved["moved"].clone()), (json!("in_place"), json!(1)));
    assert!(app.session.is_dirty());
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["allow_unverified"], false);
    assert_eq!(check["save_refusal"], crate::app::modeling::UNVERIFIED_REFUSAL);
    // Off: the save is refused and names why, and writes nothing.
    let out = t.path("edited.map");
    let r = api.call(
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "map", "path": out.display().to_string()}),
    );
    let e = r.unwrap_err();
    assert!(e.message.contains("unverified"), "{e}");
    assert!(!out.exists());
    // On: it saves, the same size, and differs from the source.
    app.dispatch(Action::SetAllowUnverifiedGeometry(true));
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["save_refusal"], Value::Null);
    call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "map", "path": out.display().to_string()}),
    );
    let (source, saved) = (std::fs::read(&map).unwrap(), std::fs::read(&out).unwrap());
    assert_eq!(saved.len(), source.len());
    assert_ne!(saved, source);
}

#[test]
fn use_for_lower_lods_is_for_model_sections() {
    let t = Temp::new("geometry-lods");
    let (mut api, mut app, _) = textured(&t);
    // A cluster has no levels of detail: refused, nothing recorded.
    let e = api
        .call(&mut app, m::GEOMETRY_LOWER_LODS, json!({"target": "cluster:e1000004:0"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS, "{e}");
    assert!(e.message.contains("levels of detail"), "{e}");
    // A model section that is not open is not found.
    let e = api
        .call(&mut app, m::GEOMETRY_LOWER_LODS, json!({"target": "mode_section:e1000002:0"}))
        .unwrap_err();
    assert_eq!(e.code, code::NO_DOCUMENT, "{e}");
    // `on` must be a boolean.
    assert!(
        api.call(&mut app, m::GEOMETRY_LOWER_LODS, json!({"target": "cluster:e1000004:0", "on": 3}))
            .is_err()
    );
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["targets"][0]["class"], "unchanged");
    assert_eq!(check["save_refusal"], Value::Null);
}

#[test]
fn a_coll_bsp_opens_as_a_mesh_and_a_compile_that_cannot_hold_is_named() {
    let t = Temp::new("geometry-coll");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("objects.map");
    std::fs::write(&map, fixture::fixture_with_object_collision()).unwrap();
    call(&mut api, &mut app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
    let list = call(&mut api, &mut app, m::GEOMETRY_LIST, Value::Null);
    let entry = list
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["kind"] == "coll_bsp")
        .expect("the object's collision BSP is listed")
        .clone();
    let id = entry["target"].as_str().unwrap().to_owned();
    assert!(id.starts_with("coll_bsp:"), "{id}");
    let cube = fixture::collision_cube();
    assert_eq!(entry["triangles"], json!(cube.surfaces.len()));
    let open = call(&mut api, &mut app, m::GEOMETRY_OPEN, json!({"target": id}));
    assert_eq!(open["faces"], json!(cube.surfaces.len()));
    // An unedited BSP has nothing to write.
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["targets"][0]["class"], "unchanged");
    // Moving it recompiles it; the invented tag has no collision
    // materials, so the compile is refused with the reason.
    call(&mut api, &mut app, m::MESH_MODE, json!({"mode": "vert"}));
    call(&mut api, &mut app, m::MESH_SELECT, json!({"all": true}));
    call(&mut api, &mut app, m::MESH_TRANSFORM, json!({"translate": [0.0, 0.0, 1.0]}));
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["targets"][0]["class"], "grown_blocks");
    assert!(check["targets"][0]["refused"].is_string(), "{check}");
    assert!(check["save_refusal"].is_string(), "{check}");
}

#[test]
fn physics_shape_edit_undoes_and_saves_with_the_map() {
    let t = Temp::new("physics-shape-edit");
    let mut app = app(&t);
    let mut api = Api::new();
    setup(&mut api, &mut app, &t, m::MODEL_PHYSICS_SHAPES);
    let query = json!({"tag": "phmo"});
    let before = call(&mut api, &mut app, m::MODEL_PHYSICS_SHAPES, query.clone());
    let set_radius = |value| json!({
        "tag": "phmo", "kind": "sphere", "index": 0, "field": "radius", "value": value
    });
    let bad = api.call(&mut app, m::MODEL_PHYSICS_SET, set_radius(-1.0)).unwrap_err();
    assert_eq!(bad.code, code::REFUSED);
    assert_eq!(call(&mut api, &mut app, m::MODEL_PHYSICS_SHAPES, query.clone()), before);
    assert!(!app.session.is_dirty());

    let result = call(&mut api, &mut app, m::MODEL_PHYSICS_SET, set_radius(1.25));
    assert!(result["warning"].is_string());
    let edited = call(&mut api, &mut app, m::MODEL_PHYSICS_SHAPES, query.clone());
    assert_eq!(edited["spheres"][0]["radius"], 1.25);
    let repeat = call(&mut api, &mut app, m::MODEL_PHYSICS_SET, set_radius(1.25));
    assert!(repeat["warning"].is_null());
    assert_eq!(call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "tags"}))["done"], true);
    assert_eq!(call(&mut api, &mut app, m::MODEL_PHYSICS_SHAPES, query.clone()), before);
    assert_eq!(call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "tags"}))["done"], true);
    assert_eq!(call(&mut api, &mut app, m::MODEL_PHYSICS_SHAPES, query), edited);

    let out = t.path("physics-edited.map");
    call(&mut api, &mut app, m::DOC_SAVE_AS, json!({"doc": "map", "path": out}));
    let mut map = map::Map::open(out).unwrap();
    let tag = fixture::id(7);
    let model = ::tags::collision::PhysicsModel::read(&mut map, tag).unwrap();
    assert_eq!(model.spheres[0].radius, 1.25);
    let mut original = map::Map::open(t.path("physics.map")).unwrap();
    let mut expected = original.read_tag_bytes(tag).unwrap();
    expected[0x13c..0x140].copy_from_slice(&1.25f32.to_le_bytes());
    assert_eq!(map.read_tag_bytes(tag).unwrap(), expected);
}

#[test]
fn from_render_replaces_collision_as_one_replayable_and_saveable_edit() {
    let t = Temp::new("collision-from-render");
    let mut app = app(&t);
    let mut api = Api::new();
    setup(&mut api, &mut app, &t, m::MODEL_COLLISION_FROM_RENDER);
    let render = "cluster:e1000004:0";
    let target = "coll_bsp:e100000a:0";
    let before = call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null);
    assert_eq!(before["faces"].as_array().unwrap().len(), 6);
    let bad = api.call(&mut app, m::MODEL_COLLISION_FROM_RENDER, json!({
        "render": render, "materials": [6]
    })).unwrap_err();
    assert_eq!(bad.code, code::REFUSED, "{bad}");
    assert!(bad.message.contains("uses material 6"), "{bad}");
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null), before);
    assert!(!app.session.is_dirty());

    let result = call(&mut api, &mut app, m::MODEL_COLLISION_FROM_RENDER, json!({
        "render": render, "materials": [1]
    }));
    assert_eq!(result, json!({"target": target, "verts": 4, "faces": 1}));
    let edited = call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null);
    assert_eq!(edited["faces"][0]["material"], 1);
    let ops = call(&mut api, &mut app, m::MESH_OPS, Value::Null)["ops"].clone();
    assert_eq!(ops.as_array().unwrap().len(), 1);
    assert_eq!(call(&mut api, &mut app, m::DOC_UNDO, json!({"doc":"map"}))["done"], true);
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null), before);
    assert_eq!(call(&mut api, &mut app, m::DOC_REDO, json!({"doc":"map"}))["done"], true);
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null), edited);
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc":"map"}));
    call(&mut api, &mut app, m::MESH_APPLY, json!({"ops": ops}));
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null), edited);

    app.dispatch(Action::SetAllowUnverifiedGeometry(true));
    let out = t.path("converted-collision.map");
    call(&mut api, &mut app, m::DOC_SAVE_AS, json!({"doc":"map", "path":out}));
    let mut map = map::Map::open(out).unwrap();
    let collision = ::tags::collision::Collision::read(&mut map, fixture::id(10)).unwrap();
    assert!(collision.validate().is_empty(), "{:?}", collision.validate());
    let bsp = collision.bsps().next().unwrap();
    assert_eq!(bsp.surfaces.len(), 1);
    assert_eq!(bsp.surfaces[0].material, 1);
    assert_eq!(bsp.surface_ring(0).len(), 4);
    assert!(bsp.vertices.iter().all(|v| v.point[2] == 0.0));
}

#[test]
fn doc_undo_on_the_map_undoes_the_geometry_edit_and_the_op_log_replays_it() {
    let t = Temp::new("geometry-undo");
    let (mut api, mut app, _) = textured(&t);
    lift_corner(&mut api, &mut app);
    let ops = call(&mut api, &mut app, m::MESH_OPS, Value::Null)["ops"].clone();
    assert_eq!(ops.as_array().unwrap().len(), 2);
    let edited = call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null)["verts"].clone();
    // Undo the move, then the selection.
    for _ in 0..2 {
        let u = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
        assert_eq!(u["done"], true);
    }
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null)["targets"][0]["class"], "unchanged");
    assert!(!app.session.is_dirty());
    // The log replays as one step to the same mesh.
    call(&mut api, &mut app, m::MESH_APPLY, json!({"ops": ops}));
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_READ, Value::Null)["verts"], edited);
    let u = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(u["done"], true);
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null)["targets"][0]["class"], "unchanged");
}

#[test]
fn a_grown_geometry_edit_refuses_the_save_and_names_the_target() {
    let t = Temp::new("geometry-grown");
    let (mut api, mut app, _) = textured(&t);
    app.dispatch(Action::SetAllowUnverifiedGeometry(true));
    call(
        &mut api,
        &mut app,
        m::MESH_APPLY,
        json!({"ops": [
            {"op": "mode", "mode": "face"},
            {"op": "select_all"},
            {"op": "extrude_faces", "offset": [0.0, 0.0, 1.0]}
        ]}),
    );
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["targets"][0]["class"], "grown_raw");
    let why = check["save_refusal"].as_str().unwrap();
    assert!(why.contains("cluster:e1000004:0") && why.contains("grown"), "{why}");
}

#[test]
fn the_design_systems_checks_and_catalogue_through_the_api() {
    let t = Temp::new("ui-kit");
    let mut app = app(&t);
    let mut api = Api::new();
    let one = call(&mut api, &mut app, m::UI_CONSISTENCY, json!({"screen": "kit.example"}));
    assert!(one["checked"].as_u64().unwrap() > 0);
    assert!(
        one["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| f["screen"] == "kit.example" && f["rule"].is_string() && f["ids"].is_array())
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_CONSISTENCY, json!({"screen": "no-such-screen"}))),
        code::INVALID_PARAMS
    );
    let c = call(&mut api, &mut app, m::UI_COMPONENTS, json!({}));
    let names: Vec<&str> = c["components"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    assert!(names.contains(&"title") && names.contains(&"legend"), "{names:?}");
    assert!(c["layouts"].as_array().unwrap().iter().any(|l| l["name"] == "layout.list"));
    assert!(c["palette"].as_array().unwrap().iter().any(|p| p["name"] == "hint_blue"));
}

#[test]
fn ui_screens_lists_the_folders_of_both_trees() {
    let t = Temp::new("ui-folders");
    let mut app = app(&t);
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::UI_SCREENS, Value::Null);
    let ours = &list["folders"]["ours"];
    // Every screen of ours is in the tree once (the existing fields stay).
    assert_eq!(
        ours["count"].as_u64().unwrap() as usize,
        list["screens"].as_array().unwrap().len()
    );
    assert!(!ours["folders"].as_array().unwrap().is_empty());
    let first = &ours["folders"][0];
    assert!(first["id"].is_string() && first["label"].is_string());
    assert!(!first["items"].as_array().unwrap().is_empty());
    // No disc: no retail tree, no grid shown.
    assert!(list["folders"]["retail"].is_null());
    assert!(list["folders"]["browse"].is_null());
    assert_eq!(list["retail"], json!([]));
}

#[test]
fn ui_select_folder_shows_the_grid_and_picking_goes_back_to_the_picture() {
    let t = Temp::new("ui-select-folder");
    let mut app = app(&t);
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::UI_SCREENS, Value::Null);
    let folder = list["folders"]["ours"]["folders"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let inside = list["folders"]["ours"]["folders"][0]["count"].as_u64().unwrap();

    let shown = call(&mut api, &mut app, m::UI_SELECT_FOLDER, json!({"folder": format!("ours/{folder}")}));
    assert_eq!(shown["browse"]["path"], format!("ours/{folder}"));
    assert_eq!(shown["tiles"].as_u64().unwrap(), inside);
    assert_eq!(app.tool, crate::ToolKind::UiScreens);
    let at = app.ui_screens.gallery.at.clone().unwrap();
    assert_eq!(at.folder, folder);
    let after = call(&mut api, &mut app, m::UI_SCREENS, Value::Null);
    assert_eq!(after["folders"]["browse"]["folder"], json!(folder));

    // The root, by root alone; `root` and `folder` together.
    let all = call(&mut api, &mut app, m::UI_SELECT_FOLDER, json!({"root": "ours"}));
    assert_eq!(
        all["tiles"].as_u64().unwrap() as usize,
        list["screens"].as_array().unwrap().len()
    );
    let both = call(&mut api, &mut app, m::UI_SELECT_FOLDER, json!({"root": "ours", "folder": folder}));
    assert_eq!(both["browse"]["path"], format!("ours/{folder}"));

    // Picking a screen shows its picture again.
    let first = crate::app::UiScreens::tree()[0].0;
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": first}));
    assert!(app.ui_screens.gallery.at.is_none());

    // What cannot be shown says why.
    let bad = |api: &mut Api, app: &mut App, p: Value| code_of(api.call(app, m::UI_SELECT_FOLDER, p));
    assert_eq!(bad(&mut api, &mut app, json!({})), code::INVALID_PARAMS);
    assert_eq!(bad(&mut api, &mut app, json!({"folder": "ours/no-such-folder"})), code::INVALID_PARAMS);
    assert_eq!(bad(&mut api, &mut app, json!({"folder": "mine/x"})), code::INVALID_PARAMS);
    assert_eq!(bad(&mut api, &mut app, json!({"root": "elsewhere"})), code::INVALID_PARAMS);
    assert_eq!(bad(&mut api, &mut app, json!({"root": "retail"})), code::FAILED, "no disc");
    assert!(app.ui_screens.gallery.at.is_none(), "a refusal changes nothing");
}

#[test]
fn ui_sheet_renders_a_folders_previews_into_one_png() {
    let t = Temp::new("ui-sheet");
    let mut app = app(&t);
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::UI_SCREENS, Value::Null);
    let folder = list["folders"]["ours"]["folders"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let inside = list["folders"]["ours"]["folders"][0]["count"].as_u64().unwrap() as usize;

    let out = t.path("out/folder.png");
    let sheet = call(
        &mut api,
        &mut app,
        m::UI_SHEET,
        json!({"folder": format!("ours/{folder}"), "out": out.display().to_string(), "columns": 2}),
    );
    assert_eq!(sheet["path"], out.display().to_string());
    let bytes = std::fs::read(&out).unwrap();
    assert_eq!(&bytes[1..4], b"PNG");
    let (w, h) = (sheet["width"].as_u64().unwrap(), sheet["height"].as_u64().unwrap());
    let tiles = sheet["tiles"].as_array().unwrap();
    assert_eq!(tiles.len(), inside);
    for tile in tiles {
        assert!(tile["pick"]["screen"].is_string(), "{tile}");
        assert!(tile["x"].as_u64().unwrap() + tile["w"].as_u64().unwrap() <= w);
        assert!(tile["y"].as_u64().unwrap() + tile["h"].as_u64().unwrap() <= h);
        assert_eq!(tile["checked"], true);
        assert!(tile["findings"].is_u64() && tile.get("error").is_none());
        // One line per finding, the count's own.
        assert_eq!(
            tile["found"].as_array().unwrap().len() as u64,
            tile["findings"].as_u64().unwrap(),
            "{tile}"
        );
    }
    // Two columns: the second tile sits to the right of the first.
    if tiles.len() > 1 {
        assert!(tiles[1]["x"].as_u64() > tiles[0]["x"].as_u64());
    }
    // The grid shown is the default; `states` gives a tile per state.
    let shown = call(&mut api, &mut app, m::UI_SELECT_FOLDER, json!({"folder": format!("ours/{folder}")}));
    assert_eq!(shown["tiles"].as_u64().unwrap() as usize, inside);
    let again = call(&mut api, &mut app, m::UI_SHEET, json!({"out": t.path("out/again.png").display().to_string()}));
    assert_eq!(again["tiles"].as_array().unwrap().len(), inside);
    let states = call(
        &mut api,
        &mut app,
        m::UI_SHEET,
        json!({"states": true, "out": t.path("out/states.png").display().to_string()}),
    );
    assert!(states["tiles"].as_array().unwrap().len() >= inside);
    assert_eq!(states["states"], true);

    // Refusals: nothing to draw, no folder, a Git checkout.
    let t2 = Temp::new("ui-sheet-fresh");
    let mut fresh = self::app(&t2);
    assert_eq!(code_of(api.call(&mut fresh, m::UI_SHEET, json!({}))), code::INVALID_PARAMS);
    assert_eq!(
        code_of(api.call(&mut app, m::UI_SHEET, json!({"folder": "ours/no-such-folder"}))),
        code::INVALID_PARAMS
    );
    let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("never-sheet.png");
    assert_eq!(
        code_of(api.call(&mut app, m::UI_SHEET, json!({"root": "ours", "out": png.display().to_string()}))),
        code::REFUSED
    );
    assert!(!png.exists());
}

/// The screen the UI edit methods' examples and tests edit: our example
/// screen, which has a static `title` and rows whose values Rust fills.
const UI_EDIT_SCREEN: &str = "kit.example";

/// Points our screens' files at a temporary checkout, never the repo's.
fn ui_edit_checkout(app: &mut App, t: &Temp) -> PathBuf {
    let checkout = t.path("checkout");
    use crate::app::ui_screens::edit::ours::{SCREENS_DIR, UI_MANIFEST};
    std::fs::create_dir_all(checkout.join(SCREENS_DIR)).unwrap();
    std::fs::write(
        checkout.join(UI_MANIFEST),
        include_str!("../../../../../content/ui/package.toml"),
    )
    .unwrap();
    app.ui_screens.edit.ours.set_checkout(Some(checkout.clone()));
    checkout
}

/// An app showing our example screen, its checkout under `t`.
fn ui_edit_app(t: &Temp) -> (Api, App, PathBuf) {
    let mut app = app(t);
    let mut api = Api::new();
    let checkout = ui_edit_checkout(&mut app, t);
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": UI_EDIT_SCREEN}));
    (api, app, checkout)
}

fn ui_title(api: &mut Api, app: &mut App) -> Value {
    let widgets = call(api, app, m::UI_WIDGETS, json!({}));
    widgets["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == "title")
        .cloned()
        .expect("the example has a title")
}

/// Widget `id`'s rect as `ui.widgets` reports it.
fn ui_widget_rect(api: &mut Api, app: &mut App, id: &str) -> [i64; 4] {
    let w = call(api, app, m::UI_WIDGETS, json!({}));
    let w = w["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == id)
        .cloned()
        .unwrap_or_else(|| panic!("no widget {id}"));
    serde_json::from_value(w["rect"].clone()).unwrap()
}

#[test]
fn ui_edits_answer_with_the_widget_and_the_render_shows_them() {
    let t = Temp::new("uiedit-api");
    let (mut api, mut app, _) = ui_edit_app(&t);
    let sel = call(&mut api, &mut app, m::UI_SELECT, json!({"id": "title"}));
    assert_eq!(sel["selected"], "title");
    assert_eq!(sel["widget"]["id"], "title");

    let text = call(&mut api, &mut app, m::UI_SET_TEXT, json!({"id": "title", "text": "From the API"}));
    assert_eq!(text["widget"]["text"], "From the API");
    assert_eq!(text["sink"], "ours");
    assert_eq!(text["dirty"], true);
    assert!(text["findings"].is_array());
    // ui.widgets (what ui.render draws) shows the edit.
    assert_eq!(ui_title(&mut api, &mut app)["text"], "From the API");

    // The title sits where the header layout puts it; the description moves.
    assert_eq!(
        code_of(api.call(&mut app, m::UI_MOVE, json!({"id": "title", "dx": 4, "dy": -2}))),
        code::REFUSED
    );
    let [top, left, bottom, right] = ui_widget_rect(&mut api, &mut app, "description");
    let moved = call(&mut api, &mut app, m::UI_MOVE, json!({"id": "description", "dx": 4, "dy": -2}));
    assert_eq!(
        moved["widget"]["rect"],
        json!([top - 2, left + 4, bottom - 2, right + 4])
    );
    let png = t.path("out/edited.png");
    std::fs::create_dir_all(t.path("out")).unwrap();
    call(&mut api, &mut app, m::UI_RENDER, json!({"out": png.display().to_string()}));
    assert!(png.exists());

    // Past the title-safe area: the checks name the title.
    let off = call(
        &mut api,
        &mut app,
        m::UI_SET_RECT,
        json!({"id": "description", "rect": [236, -316, 212, 24]}),
    );
    assert_eq!(off["widget"]["rect"], json!([236, -316, 212, 24]));
    assert!(
        off["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["ids"].as_array().unwrap().iter().any(|i| i == "description")),
        "{off}"
    );

    let edits = call(&mut api, &mut app, m::UI_EDITS, json!({}));
    assert_eq!(edits["sink"], "ours");
    assert_eq!(edits["ours_dirty"], true);
    assert_eq!(edits["ours"][0]["screen"], UI_EDIT_SCREEN);
    assert!(edits["ours"][0]["yaml"].as_str().unwrap().contains("From the API"));
}

#[test]
fn ui_undo_redo_and_revert_through_the_api() {
    let t = Temp::new("uiedit-undo");
    let (mut api, mut app, _) = ui_edit_app(&t);
    let before = ui_title(&mut api, &mut app)["text"].clone();
    call(&mut api, &mut app, m::UI_SET_TEXT, json!({"id": "title", "text": "Once"}));

    let undo = call(&mut api, &mut app, m::UI_UNDO, json!({}));
    assert_eq!(undo["done"], true);
    assert_eq!(undo["can_redo"], true);
    assert_eq!(ui_title(&mut api, &mut app)["text"], before);
    let again = call(&mut api, &mut app, m::UI_UNDO, json!({}));
    assert_eq!(again["done"], false, "nothing left");

    let redo = call(&mut api, &mut app, m::UI_REDO, json!({}));
    assert_eq!(redo["done"], true);
    assert_eq!(ui_title(&mut api, &mut app)["text"], "Once");

    let revert = call(&mut api, &mut app, m::UI_REVERT, json!({"id": "title"}));
    assert_eq!(revert["edits"]["ours_dirty"], false);
    assert_eq!(ui_title(&mut api, &mut app)["text"], before);
    // The revert is an undo step of its own.
    call(&mut api, &mut app, m::UI_UNDO, json!({}));
    assert_eq!(ui_title(&mut api, &mut app)["text"], "Once");
    call(&mut api, &mut app, m::UI_REVERT, json!({}));
    assert_eq!(ui_title(&mut api, &mut app)["text"], before);
}

#[test]
fn ui_edits_refuse_rust_owned_widgets_and_bad_rects() {
    let t = Temp::new("uiedit-refuse");
    let (mut api, mut app, _) = ui_edit_app(&t);
    let widgets = call(&mut api, &mut app, m::UI_WIDGETS, json!({}));
    let value = widgets["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["id"].as_str().unwrap().to_owned())
        .find(|id| id.ends_with(".value"))
        .expect("the example has a value row");
    assert_eq!(
        code_of(api.call(&mut app, m::UI_SET_TEXT, json!({"id": value, "text": "x"}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::UI_SET_RECT,
            json!({"id": "title", "rect": [0, 10, 20, 0]})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_SET_RECT, json!({"id": "title", "rect": [1, 2, 3]}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_MOVE, json!({"id": "title", "dx": 40000, "dy": 0}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_SELECT, json!({"id": "nope"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_ANIMATE, json!({"phase": "sideways"}))),
        code::INVALID_PARAMS
    );
    let edits = call(&mut api, &mut app, m::UI_EDITS, json!({}));
    assert_eq!(edits["ours_dirty"], false);
}

#[test]
fn ui_save_writes_the_yaml_into_the_ui_package() {
    let t = Temp::new("uiedit-save");
    let (mut api, mut app, checkout) = ui_edit_app(&t);
    call(&mut api, &mut app, m::UI_SET_TEXT, json!({"id": "title", "text": "Saved by the API"}));
    let saved = call(&mut api, &mut app, m::UI_SAVE, json!({}));
    assert_eq!(saved["sink"], "ours");
    let file = crate::app::ui_screens::edit::ours::file_of(&checkout, UI_EDIT_SCREEN);
    assert_eq!(saved["written"], json!([file.display().to_string()]));
    assert!(std::fs::read_to_string(&file).unwrap().contains("Saved by the API"));
    assert_eq!(call(&mut api, &mut app, m::UI_EDITS, json!({}))["ours_dirty"], false);

    // Outside a checkout the save is refused and the edit kept.
    app.ui_screens.edit.ours.set_checkout(None);
    call(&mut api, &mut app, m::UI_SET_TEXT, json!({"id": "title", "text": "Kept"}));
    assert_eq!(code_of(api.call(&mut app, m::UI_SAVE, json!({}))), code::REFUSED);
    assert_eq!(call(&mut api, &mut app, m::UI_EDITS, json!({}))["ours_dirty"], true);
}

#[test]
fn ui_animate_writes_a_filmstrip_outside_git() {
    let t = Temp::new("uiedit-film");
    let (mut api, mut app, _) = ui_edit_app(&t);
    let out = t.path("out/open.png");
    let film = call(
        &mut api,
        &mut app,
        m::UI_ANIMATE,
        json!({"phase": "open", "frames": 3, "out": out.display().to_string()}),
    );
    assert_eq!(film["path"], out.display().to_string());
    assert!(out.exists());
    let frames = film["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[0]["tick"], 0);
    assert_eq!(frames[0]["t"], 0.0);
    assert_eq!(frames[2]["tick"], boot::ui_kit::animation::Transition::Open.ticks());
    assert_eq!(film["width"], 3 * (320 + 8) + 8);
    let close = call(&mut api, &mut app, m::UI_ANIMATE, json!({"phase": "close", "frames": 2, "out": t.path("out/close.png").display().to_string()}));
    assert_eq!(close["frames"].as_array().unwrap().len(), 2);
    assert_eq!(
        code_of(api.call(&mut app, m::UI_ANIMATE, json!({"phase": "open", "frames": 0}))),
        code::INVALID_PARAMS
    );
    let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("never-film.png");
    assert_eq!(
        code_of(api.call(&mut app, m::UI_ANIMATE, json!({"phase": "open", "out": png.display().to_string()}))),
        code::REFUSED
    );
    assert!(!png.exists());
}

#[test]
fn ui_edits_need_a_screen_and_retail_needs_the_disc() {
    let t = Temp::new("uiedit-retail");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::UI_MOVE, json!({"id": "title", "dx": 1, "dy": 0}))),
        code::NO_DOCUMENT
    );
    // ui.edits answers with nothing shown.
    assert_eq!(call(&mut api, &mut app, m::UI_EDITS, json!({}))["sink"], Value::Null);
    // A retail screen without the disc draws nothing, so has no widget to
    // edit; nor a filmstrip.
    app.dispatch(Action::SetTool(crate::app::ToolKind::UiScreens));
    app.dispatch(Action::UiPick(crate::app::Pick::retail(r"ui\screens\x")));
    assert_eq!(
        code_of(api.call(&mut app, m::UI_MOVE, json!({"id": "text.0", "dx": 1, "dy": 0}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_ANIMATE, json!({"phase": "open", "out": t.path("out/r.png").display().to_string()}))),
        code::INVALID_PARAMS
    );
    let undo = call(&mut api, &mut app, m::UI_UNDO, json!({}));
    assert_eq!(undo["done"], false);
}

#[test]
fn ui_retail_tag_edits_refuse_our_screens_and_need_the_disc() {
    let t = Temp::new("uiedit-retail-tag");
    let mut app = app(&t);
    let mut api = Api::new();
    let hide = json!({"id": "text.0"});
    let copy = json!({"id": "text.0", "rect": [95, -260, 75, 70]});
    let string = json!({"id": "text.0", "string": "lobby_mode", "shows": "CUSTOM GAMES"});
    // No screen shown.
    for (method, params) in [
        (m::UI_HIDE, &hide),
        (m::UI_DUPLICATE, &copy),
        (m::UI_SET_STRING, &string),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, method, params.clone())),
            code::NO_DOCUMENT,
            "{method}"
        );
    }
    // A screen of ours: refused, naming it, and nothing is edited.
    ui_edit_checkout(&mut app, &t);
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": UI_EDIT_SCREEN}));
    for (method, params) in [
        (m::UI_HIDE, json!({"id": "title"})),
        (m::UI_DUPLICATE, json!({"id": "title"})),
        (m::UI_SET_STRING, json!({"id": "title", "string": "lobby_mode"})),
    ] {
        let e = api.call(&mut app, method, params).unwrap_err();
        assert_eq!(e.code, code::REFUSED, "{method}: {e}");
        assert!(e.message.contains(UI_EDIT_SCREEN), "{method}: {e}");
    }
    let edits = call(&mut api, &mut app, m::UI_EDITS, json!({}));
    assert_eq!(edits["ours_dirty"], false);
    assert_eq!(edits["retail_blocks"], json!({}));
    assert_eq!(edits["stand_ins"], json!({}));
    // Params are checked before the edit.
    assert_eq!(
        code_of(api.call(&mut app, m::UI_HIDE, json!({}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_DUPLICATE, json!({"id": "title", "rect": [1, 2]}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_SET_STRING, json!({"id": "title"}))),
        code::INVALID_PARAMS
    );
    // A retail screen without the disc has no widget to hide or copy, and
    // a string ID needs the menu map open.
    app.dispatch(Action::UiPick(crate::app::Pick::retail(r"ui\screens\x")));
    assert_eq!(
        code_of(api.call(&mut app, m::UI_HIDE, hide.clone())),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::UI_DUPLICATE, copy.clone())),
        code::INVALID_PARAMS
    );
    let e = api.call(&mut app, m::UI_SET_STRING, string.clone()).unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{e}");
    assert!(e.message.contains("mainmenu"), "{e}");
    // A name that is not a string ID's is the caller's mistake.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::UI_SET_STRING,
            json!({"id": "text.0", "string": "Lobby Mode"})
        )),
        code::INVALID_PARAMS
    );
    let edits = call(&mut api, &mut app, m::UI_EDITS, json!({}));
    assert_eq!(edits["sink"], "retail");
    assert_eq!(edits["stand_ins"], json!({}));
    assert_eq!(edits["retail_dirty"], false);
}

/// The layers' rows in `view.layers`, by name.
fn layer_row(api: &mut Api, app: &mut App, name: &str) -> Value {
    let layers = call(api, app, m::VIEW_LAYERS, Value::Null);
    layers["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap()
        .clone()
}

/// Open `bytes` as the source, at `name` under `t`.
fn open_map(api: &mut Api, app: &mut App, t: &Temp, name: &str, bytes: Vec<u8>) -> PathBuf {
    let map = t.path(name);
    std::fs::write(&map, bytes).unwrap();
    call(
        api,
        app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    map
}

/// The invented BSP's two pathfinding sectors through `nav.pathfinding`;
/// `nav.pick` finds each by point and by ray whatever layers are shown,
/// and selects one. The Navigation layers start hidden, count what they
/// draw, and the Navigation preset shows them.
#[test]
fn nav_pathfinding_and_pick_read_the_invented_sectors() {
    let t = Temp::new("nav-pathfinding");
    let mut app = app(&t);
    let mut api = Api::new();
    let none = call(&mut api, &mut app, m::NAV_PATHFINDING, Value::Null);
    assert_eq!(none["counts"], json!({"sectors": 0, "links": 0}));
    assert_eq!(none["note"], "the map has no structure BSP");
    open_map(
        &mut api,
        &mut app,
        &t,
        "pathfinding.map",
        fixture::fixture_with_pathfinding(),
    );
    let all = call(&mut api, &mut app, m::NAV_PATHFINDING, Value::Null);
    assert_eq!(all["counts"], json!({"sectors": 2, "links": 7}));
    assert_eq!(all["note"], Value::Null);
    assert_eq!(all["errors"], json!([]));
    let bsp = &all["bsps"][0];
    assert_eq!(bsp["present"], true);
    assert_eq!(
        (bsp["sectors"].clone(), bsp["nodes"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(bsp["orphan_links"], 1);
    let rows = all["sectors"].as_array().unwrap();
    assert_eq!(rows[0]["flags"], fixture::SECTOR_FLAGS[0]);
    assert_eq!(rows[1]["links"], json!([4, 5, 6, 1]));
    assert_eq!(rows[1]["floor"], true);
    assert_eq!(rows[0]["ring"].as_array().unwrap().len(), 4);
    let paged = call(
        &mut api,
        &mut app,
        m::NAV_PATHFINDING,
        json!({"offset": 1, "limit": 1}),
    );
    assert_eq!(paged["matching"], 2);
    assert_eq!(paged["sectors"][0]["index"], 1);

    let [x, y, _] = fixture::ROOM_AT;
    let point = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"point": [x + 2.0, y + 2.0, 0.0]}),
    );
    assert_eq!(point["hit"]["kind"], "sector");
    assert_eq!(point["hit"]["index"], 0);
    let ray = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"origin": [x + 7.5, y + 5.0, 10.0], "dir": [0, 0, -2], "select": true}),
    );
    assert_eq!(ray["hit"]["index"], 1);
    assert!(
        (ray["distance"].as_f64().unwrap() - (10.0 - 0.01)).abs() < 1e-3,
        "{ray}"
    );
    assert_eq!(ray["selected"], json!({"kind": "sector", "index": 1}));
    assert_eq!(app.session.selected(), None);
    // Hidden layers pick nothing when asked to honour them.
    let shown = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"point": [x + 2.0, y + 2.0, 0.0], "shown": true}),
    );
    assert_eq!(shown["hit"], Value::Null);
    let other = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"point": [x + 2.0, y + 2.0, 0.0], "layer": "bot_nav"}),
    );
    assert_eq!(other["hit"], Value::Null);
    let direct = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"kind": "sector", "index": 0, "focus": true}),
    );
    assert_eq!(direct["selected"], json!({"kind": "sector", "index": 0}));
    let target = app.session.camera().target;
    assert!((target[0] - (x + 2.5)).abs() < 1e-3, "{target:?}");
    for bad in [
        json!({"point": [0, 0, 0], "origin": [0, 0, 0], "dir": [1, 0, 0]}),
        json!({"origin": [0, 0, 0]}),
        json!({"origin": [0, 0, 0], "dir": [0, 0, 0]}),
        json!({"point": [0, 0, 0], "layer": "collision"}),
        json!({"kind": "sector", "index": 9}),
        json!({"kind": "wall", "index": 0}),
        json!({"kind": "sector"}),
        json!({"kind": "sector", "index": 0, "point": [0, 0, 0]}),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, m::NAV_PICK, bad.clone())),
            code::INVALID_PARAMS,
            "{bad}"
        );
    }

    for (name, count) in [("pathfinding", 2), ("bot_nav", 0), ("bot_spots", 0)] {
        let row = layer_row(&mut api, &mut app, name);
        assert_eq!(row["group"], "Navigation", "{name}");
        assert_eq!(row["on"], false, "{name}");
        assert_eq!(row["count"], count, "{name}");
    }
    let shown = call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"preset": "navigation"}),
    );
    assert!(
        shown["layers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["group"] == "Navigation")
            .all(|r| r["on"] == true)
    );

    // The sectors draw in the snapshot, over a level with nothing else.
    let out = t.path("pathfinding.png");
    let params = json!({
        "out": out, "size": [96, 96],
        "camera": {"target": [x + 5.0, y + 5.0, 0], "distance": 25, "yaw": 0, "pitch": 80}
    });
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Ok(_) => {}
        Err(e) => panic!("view.snapshot: {e}"),
    }
    let drawn = png_pixels(&out);
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"set": {"pathfinding": false}}),
    );
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(
        png_pixels(&out) != drawn,
        "hiding the layer must remove the sectors"
    );
}

/// An invented bot cache, spots file and the three state files beside
/// it, for the fixture map (its header's name is `fixture`).
fn invented_bot_files(data: &Path, map_bytes: &[u8]) {
    use bots::nav::{Edge, Kind, NavMesh, Poly};
    let dir = bots::cache::dir(data);
    std::fs::create_dir_all(&dir).unwrap();
    let fingerprint = bots::autonav::fingerprint(&map_bytes[..bots::autonav::MAP_HEADER_LEN]);
    let [x, y, _] = fixture::ROOM_AT;
    let quad = |x0: f32, z: f32| {
        Poly::new(vec![
            [x + x0, y, z],
            [x + x0 + 5.0, y, z],
            [x + x0 + 5.0, y + 10.0, z],
            [x + x0, y + 10.0, z],
        ])
    };
    let edge = |from, to, kind| Edge {
        from,
        to,
        a: [x + 5.0, y, 0.0],
        b: [x + 5.0, y + 10.0, 0.0],
        kind,
    };
    let mut nav = NavMesh::new(
        vec![quad(0.0, 0.0), quad(5.0, 0.0), quad(10.0, -1.0)],
        vec![
            edge(0, 1, Kind::Walk),
            edge(1, 0, Kind::Walk),
            edge(1, 2, Kind::Drop),
        ],
    );
    // The one start is on the low floor: the two floors that drop into it
    // are out of the map.
    assert_eq!(nav.mark_out_of_map(&[[x + 12.5, y + 5.0, -1.0]]), 2);
    let data_file = bots::cache::MapData {
        nav,
        walls: bots::los::Walls::new(vec![[[x, y, 0.0], [x, y + 1.0, 0.0], [x, y, 1.0]]]),
        places: Default::default(),
    };
    std::fs::write(
        dir.join(bots::cache::file_name("fixture", 0)),
        data_file.to_bytes(bots::cache::Stamp::now(fingerprint)),
    )
    .unwrap();
    let spot = |kind, shape, at: [f32; 3]| superbounce::Spot {
        kind,
        shape,
        stand: at,
        edge: [at, [at[0] + 1.0, at[1], at[2]]],
        score: 0.8,
        launch: 10.0,
        ..superbounce::Spot::default()
    };
    use superbounce::{Kind as SpotKind, Shape, Status};
    let (bounce, strip, crouch) = (
        [x + 1.0, y + 1.0, 0.0],
        [x + 3.0, y + 8.0, 0.0],
        [x + 8.0, y + 2.0, 0.0],
    );
    let found = [
        spot(SpotKind::Bounce, Shape::Slot, bounce),
        spot(SpotKind::Bounce, Shape::Crease, strip),
        spot(SpotKind::Crouch, Shape::Slot, crouch),
    ];
    bots::spots::write(&dir, "fixture", bots::spots::stamp(fingerprint), &found).unwrap();
    let verified = [superbounce::verified::Entry {
        kind: SpotKind::Crouch,
        status: Status::Proven,
        at: crouch,
        trials: 3,
        ok: 2,
        ..Default::default()
    }];
    std::fs::write(
        dir.join(bots::spots::verified_file_name("fixture")),
        superbounce::verified::to_text(&verified),
    )
    .unwrap();
    let learned = [superbounce::learned::Entry {
        state: superbounce::learned::State::Dead,
        at: bounce,
        tries: 12,
        why: Some("nobounce".into()),
        ..Default::default()
    }];
    std::fs::write(
        dir.join(bots::spots::learned_file_name("fixture")),
        superbounce::learned::to_text(&learned),
    )
    .unwrap();
    let ghost = superbounce::ghost::Ghost {
        spot: strip,
        launch: 11.0,
        touchdown: 1,
        frames: vec![
            superbounce::ghost::Frame {
                at: strip,
                ..Default::default()
            };
            2
        ],
        ..Default::default()
    };
    std::fs::write(
        dir.join(bots::harvest::ghosts_file_name("fixture")),
        superbounce::ghost::to_text(&[ghost]),
    )
    .unwrap();
}

/// The invented bot cache through `nav.bots`: its polygons and links by
/// kind (the drop one way), its spots by kind and by what the verified,
/// learned and ghosts files say; `nav.pick` finds a spot and a polygon.
/// Showing a bot layer starts the load the same way.
#[test]
fn nav_bots_reads_an_invented_cache_and_its_spot_files() {
    let t = Temp::new("nav-bots");
    let mut app = app(&t);
    let mut api = Api::new();
    let bytes = fixture::fixture_with_pathfinding();
    invented_bot_files(&t.path("data"), &bytes);
    open_map(&mut api, &mut app, &t, "bots.map", bytes);
    assert_eq!(app.bot_nav_state().name(), "idle", "nothing asked yet");
    let r = call(
        &mut api,
        &mut app,
        m::NAV_BOTS,
        json!({"wait": true, "polys": true, "links": true, "spots": true}),
    );
    assert_eq!(r["state"], "ready", "{r}");
    assert_eq!(r["error"], Value::Null);
    assert_eq!(r["map"], "fixture");
    assert_eq!(r["notes"], json!([]), "{r}");
    assert!(
        r["origins"][0]
            .as_str()
            .unwrap()
            .ends_with("fixture.0.bots")
    );
    assert_eq!(
        r["counts"],
        json!({"polys": 3, "out_of_map": 2, "links": 3, "walls": 1, "spots": 3})
    );
    let polys = r["polys"].as_array().unwrap();
    let out: Vec<_> = polys.iter().map(|p| p["out_of_map"].clone()).collect();
    assert_eq!(out, [json!(true), json!(true), json!(false)]);
    assert_eq!(r["links_by_kind"]["walk"], 2);
    assert_eq!(r["links_by_kind"]["drop"], 1);
    assert_eq!(r["links_by_kind"]["jump"], 0);
    let links = r["links"].as_array().unwrap();
    assert_eq!(links[2]["kind"], "drop");
    assert_eq!(links[2]["one_way"], true);
    assert_eq!(links[0]["one_way"], false);
    assert_eq!(
        r["spots_by_kind"],
        json!({"bounce": 1, "strip": 1, "crouch": 1})
    );
    assert_eq!(
        r["spots_by_state"],
        json!({"proven": 1, "failed": 0, "dead": 1, "learned": 0, "training": 0, "likely": 1})
    );
    let spots = r["spots"].as_array().unwrap();
    let of = |kind: &str| spots.iter().find(|s| s["kind"] == kind).unwrap().clone();
    assert_eq!(of("bounce")["state"], "dead");
    assert_eq!(of("bounce")["line"], "dead");
    assert_eq!(of("bounce")["tries"], 12);
    assert_eq!(of("crouch")["state"], "proven");
    assert_eq!(of("crouch")["trials"], 3);
    assert_eq!(of("strip")["state"], "likely");
    assert_eq!(of("strip")["ghost"], true);
    assert_eq!(of("bounce")["ghost"], false);

    let [x, y, _] = fixture::ROOM_AT;
    let spot = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"point": [x + 8.0, y + 2.0, 0.0], "select": true}),
    );
    assert_eq!(spot["hit"]["kind"], "spot");
    assert_eq!(spot["hit"]["state"], "proven");
    assert_eq!(spot["selected"]["kind"], "spot");
    let poly = call(
        &mut api,
        &mut app,
        m::NAV_PICK,
        json!({"origin": [x + 12.0, y + 8.0, 5.0], "dir": [0, 0, -1], "layer": "bot_nav"}),
    );
    assert_eq!(poly["hit"]["kind"], "poly");
    assert_eq!(poly["hit"]["index"], 2);
    assert_eq!(poly["hit"]["out_of_map"], false);
    for (name, count) in [("bot_nav", 3), ("bot_spots", 3)] {
        assert_eq!(
            layer_row(&mut api, &mut app, name)["count"],
            count,
            "{name}"
        );
    }

    // Reopening the map drops its data; showing a bot layer loads it again.
    open_map(
        &mut api,
        &mut app,
        &t,
        "bots.map",
        fixture::fixture_with_pathfinding(),
    );
    assert!(app.session.bot_nav().is_none());
    app.layers
        .set(scenario_edit::layers::LayerId::BotSpots, true);
    app.poll_bot_nav();
    app.wait_bot_nav();
    assert_eq!(app.session.bot_nav().map(|b| b.spots.len()), Some(3));
}

/// With no cache file the bots' own builder makes the mesh in memory from
/// the invented room's collision, and says so.
#[test]
fn nav_bots_builds_the_mesh_when_there_is_no_cache() {
    let t = Temp::new("nav-bots-build");
    let mut app = app(&t);
    let mut api = Api::new();
    open_map(
        &mut api,
        &mut app,
        &t,
        "room.map",
        fixture::fixture_with_collision(),
    );
    let r = call(&mut api, &mut app, m::NAV_BOTS, json!({"wait": true}));
    assert_eq!(r["state"], "ready", "{r}");
    assert!(r["counts"]["polys"].as_u64().unwrap() >= 1, "{r}");
    assert_eq!(r["origins"], json!(["built"]));
    let notes = r["notes"].as_array().unwrap();
    assert!(
        notes[0].as_str().unwrap().contains("built in memory"),
        "{r}"
    );
    assert!(
        !t.path("data").join("bots").join("fixture.0.bots").exists(),
        "the build is not written"
    );
}

#[test]
fn ui_pick_names_a_retail_screens_sample_variant() {
    let t = Temp::new("ui-pick-sample");
    let mut app = app(&t);
    let mut api = Api::new();
    let setup = r"ui\screens\game_shell\pregame_lobby\squad_settings_dialog";
    let r = call(
        &mut api,
        &mut app,
        m::UI_PICK,
        json!({"retail": setup, "sample": "switch-to"}),
    );
    assert_eq!(r["pick"], json!({"retail": setup, "sample": "switch-to"}));
    assert_eq!(
        app.ui_screens.pick,
        Some(crate::app::Pick::Retail {
            tag: setup.into(),
            sample: Some("switch-to".into()),
        })
    );
    // Without a name: the plain sample.
    let r = call(&mut api, &mut app, m::UI_PICK, json!({"retail": setup}));
    assert_eq!(r["pick"], json!({"retail": setup}));
    // A name the screen has no sample for is refused with its names.
    let e = api
        .call(&mut app, m::UI_PICK, json!({"retail": setup, "sample": "no-such"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS, "{e}");
    assert!(e.message.contains("switch-to"), "{e}");
    assert_eq!(app.ui_screens.pick, Some(crate::app::Pick::retail(setup)), "nothing changed");
    // A sample is a retail screen's: not one of ours.
    assert_eq!(
        code_of(api.call(&mut app, m::UI_PICK, json!({"screen": "kit.example", "sample": "switch-to"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn ui_pick_shows_a_picker_with_a_scripts_rows() {
    let t = Temp::new("ui-pick-rows");
    let mut app = app(&t);
    let mut api = Api::new();
    let rows = ["BATTLE RIFLE + MAGNUM", "SHOTGUN + MAGNUM", "SNIPER RIFLE + SMG"];
    let r = call(
        &mut api,
        &mut app,
        m::UI_PICK,
        json!({"screen": "kit.picker.game", "title": "ATTACKER LOADOUT", "rows": rows}),
    );
    assert_eq!(r["pick"]["screen"], "kit.picker.game", "{r}");
    let w = call(&mut api, &mut app, m::UI_WIDGETS, json!({})).to_string();
    for row in rows.iter().chain(&["ATTACKER LOADOUT"]) {
        assert!(w.contains(row), "{row} is not on the screen: {w}");
    }
    // The offer is for that pick only: the next one is the kit's sample.
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": "kit.picker.game"}));
    let w = call(&mut api, &mut app, m::UI_WIDGETS, json!({})).to_string();
    assert!(!w.contains("SHOTGUN + MAGNUM"), "{w}");
    // Refused: title alone, rows on another screen, no rows, too many rows.
    for bad in [
        json!({"screen": "kit.picker.game", "title": "T"}),
        json!({"screen": "kit.example", "rows": ["a"]}),
        json!({"screen": "kit.picker.game", "rows": []}),
        json!({"screen": "kit.picker.game", "rows": vec!["x"; 17]}),
        json!({"screen": "kit.picker.game", "rows": [1]}),
    ] {
        assert_eq!(code_of(api.call(&mut app, m::UI_PICK, bad.clone())), code::INVALID_PARAMS, "{bad}");
    }
}

/// The invented map with a compiled box room, open inside `[0, 8] x
/// [0, 8] x [0, 4]`, as structure e1000004's level collision: its eight
/// blocks, a per-leaf block of one element a leaf and two collision
/// materials, no MOPP. `geometry_edit::level_collision::read` reads it.
fn level_collision_map() -> Vec<u8> {
    use geometry_edit::collision_build;
    use geometry_edit::level_collision::{LEAVES_FIELD, MATERIALS_FIELD, box_solid};
    use scenario_edit::fixture::{META, META_FILE, SBSP, SBSP_COLLISION_AT, SBSP_SIZE};
    const LEAVES_AT: usize = 0x1a00;
    let mut room = box_solid([0.0; 3], [8.0, 8.0, 4.0], 1);
    for p in &mut room {
        p.points.reverse();
    }
    let bsp = collision_build::build(&room).unwrap().bsp;
    let mut b = fixture::fixture_with_collision();
    let root = META_FILE + SBSP;
    let blocks = META + (SBSP + SBSP_COLLISION_AT) as u32;
    let encoded = bsp.encode_blocks(blocks);
    assert!(SBSP_COLLISION_AT + encoded.len() <= SBSP_SIZE);
    let at = root + SBSP_COLLISION_AT;
    b[at..root + SBSP_SIZE].fill(0);
    b[at..at + encoded.len()].copy_from_slice(&encoded);
    fixture::put(&mut b, root + MATERIALS_FIELD, 2);
    fixture::put(&mut b, root + MATERIALS_FIELD + 4, META + 0x1900);
    let leaves = vec![0u8; 8 * bsp.leaves.len()];
    assert!(LEAVES_AT + leaves.len() <= 0x2000);
    b[META_FILE + LEAVES_AT..META_FILE + LEAVES_AT + leaves.len()].copy_from_slice(&leaves);
    fixture::put(&mut b, root + LEAVES_FIELD, bsp.leaves.len() as u32);
    fixture::put(&mut b, root + LEAVES_FIELD + 4, META + LEAVES_AT as u32);
    fixture::metadata_last(b)
}

#[test]
fn collision_boxes_chain_save_and_clear_as_modeling_actions() {
    let t = Temp::new("geometry-collision");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("level.map");
    std::fs::write(&map, level_collision_map()).unwrap();
    call(&mut api, &mut app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
    let structure = json!("e1000004");

    let one = call(
        &mut api,
        &mut app,
        m::GEOMETRY_COLLISION_ADD_BOX,
        json!({"structure": structure, "lo": [2, 2, 1], "hi": [3, 3, 2]}),
    );
    assert_eq!(one["structure"], "0xe1000004");
    let file = one["file_surfaces"].as_u64().unwrap();
    assert!(one["surfaces"].as_u64().unwrap() > file, "{one}");
    // The edit is what Play sends (`Modeling::level_collision`).
    let tag = map::TagId(0xe100_0004);
    assert_eq!(app.modeling.level_collision.len(), 1);
    let rev = app.modeling.level_collision_rev;

    // A second box goes onto the first, from the same file.
    let two = call(
        &mut api,
        &mut app,
        m::GEOMETRY_COLLISION_ADD_BOX,
        json!({"structure": structure, "lo": [5, 5, 1], "hi": [6, 6, 2], "material": 1}),
    );
    assert!(two["surfaces"].as_u64().unwrap() > one["surfaces"].as_u64().unwrap(), "{two}");
    assert_eq!(two["file_surfaces"].as_u64().unwrap(), file);
    assert!(app.modeling.level_collision_rev > rev);
    let edit = app.modeling.level_collision[&tag].clone();

    // A box with no volume, or one outside the room, changes nothing.
    for bad in [
        json!({"structure": structure, "lo": [2, 2, 1], "hi": [2, 3, 2]}),
        json!({"structure": structure, "lo": [20, 20, 1], "hi": [21, 21, 2]}),
    ] {
        assert!(api.call(&mut app, m::GEOMETRY_COLLISION_ADD_BOX, bad.clone()).is_err(), "{bad}");
        assert_eq!(app.modeling.level_collision[&tag], edit);
    }
    // Not a structure: refused.
    assert!(
        api.call(
            &mut app,
            m::GEOMETRY_COLLISION_ADD_BOX,
            json!({"structure": "e1000001", "lo": [2, 2, 1], "hi": [3, 3, 2]}),
        )
        .is_err()
    );

    // Clear: the file's collision again, and nothing for the save.
    let cleared = call(&mut api, &mut app, m::GEOMETRY_COLLISION_CLEAR, json!({"structure": structure}));
    assert_eq!(cleared["cleared"], true);
    assert!(app.modeling.level_collision.is_empty());
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null)["save_refusal"], Value::Null);
    let again = call(&mut api, &mut app, m::GEOMETRY_COLLISION_CLEAR, json!({"structure": structure}));
    assert_eq!(again["cleared"], false);

    // The same two boxes again: the same level.
    for (lo, hi, material) in [([2, 2, 1], [3, 3, 2], 0), ([5, 5, 1], [6, 6, 2], 1)] {
        call(
            &mut api,
            &mut app,
            m::GEOMETRY_COLLISION_ADD_BOX,
            json!({"structure": structure, "lo": lo, "hi": hi, "material": material}),
        );
    }
    assert_eq!(app.modeling.level_collision[&tag], edit);

    // The save needs the unverified switch, then writes both boxes.
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["save_refusal"], crate::app::modeling::UNVERIFIED_REFUSAL);
    app.dispatch(Action::SetAllowUnverifiedGeometry(true));
    assert_eq!(call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null)["save_refusal"], Value::Null);
    let out = t.path("boxes.map");
    call(
        &mut api,
        &mut app,
        m::DOC_SAVE_AS,
        json!({"doc": "map", "path": out.display().to_string()}),
    );
    let mut saved = ::map::Map::from_reader(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    saved.validate_checksum().unwrap();
    let back = geometry_edit::level_collision::read(&mut saved, tag).unwrap();
    assert_eq!(back.level, edit.added.level);
}

/// An outside editor's save: `text`, with a modification time `secs`
/// ahead, so Studio's watch sees a new one.
fn save_as_editor(path: &Path, text: &str, secs: u64) {
    std::fs::write(path, text).unwrap();
    let f = std::fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn a_game_type_script_opens_in_vs_code_reloads_from_its_save_and_runs_a_scenario() {
    let t = Temp::new("rules-external");
    let mut app = app(&t);
    let mut api = Api::new();
    call(&mut api, &mut app, m::RULES_OPEN, json!({"starter": "slayer"}));

    // Its file under the data root, and the commands that open it at the
    // line; nothing is started with launch false.
    let v = call(
        &mut api,
        &mut app,
        m::RULES_EXTERNAL,
        json!({"editor": "code", "line": 4, "launch": false}),
    );
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert!(path.starts_with(t.path("data")), "{v}");
    assert_eq!((v["line"].clone(), v["col"].clone()), (json!(4), json!(1)));
    assert_eq!(v["editor"], "code");
    assert_eq!(v["launched"], false);
    for c in v["commands"].as_array().unwrap() {
        assert_eq!(c["args"][0], "-g", "{c}");
        assert!(c["args"][1].as_str().unwrap().ends_with(":4:1"), "{c}");
    }
    assert_eq!(app.rules.watch.as_ref().map(|w| w.path.clone()), Some(path.clone()));
    assert_eq!(
        code_of(api.call(&mut app, m::RULES_EXTERNAL, json!({"editor": "emacs"}))),
        code::INVALID_PARAMS
    );

    // VS Code's Reload in game: the save is read before the reload.
    let text = std::fs::read_to_string(&path).unwrap();
    let theirs = text.replace("Slayer", "Theirs");
    std::fs::write(&path, &theirs).unwrap();
    let p = path.display().to_string();
    let v = call(&mut api, &mut app, m::RULES_RELOAD, json!({"path": p}));
    assert!(v["queued"].is_string(), "{v}");
    assert_eq!(app.rules.text, theirs);
    assert!(!app.rules.dirty());

    // Edits here that the file does not have: refused, both kept.
    app.rules.set_text(theirs.replace("Theirs", "Mine"));
    std::fs::write(&path, theirs.replace("Theirs", "Disk")).unwrap();
    assert_eq!(
        code_of(api.call(&mut app, m::RULES_RELOAD, json!({"path": p}))),
        code::UNSAVED
    );
    assert!(app.rules.text.contains("Mine"));

    // Another file: opened and watched, unless edits here would be lost.
    let other = t.path("invented_other.rules");
    std::fs::write(&other, ::rules::builtin::SLAYER).unwrap();
    let o = other.display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::RULES_RELOAD, json!({"path": o}))),
        code::UNSAVED
    );
    call(&mut api, &mut app, m::RULES_RELOAD, json!({"path": o, "discard": true, "mode": "restart"}));
    assert_eq!(app.rules.path.as_deref(), Some(other.as_path()));
    assert_eq!(app.rules.watch.as_ref().map(|w| w.path.clone()), Some(other.clone()));
    assert_eq!(app.rules.text, ::rules::builtin::SLAYER);

    // A scenario file over the open script, as h2 rules run prints it.
    let case = t.path("invented_case.json");
    std::fs::write(&case, r#"{"ticks":[{},{},{}]}"#).unwrap();
    let v = call(
        &mut api,
        &mut app,
        m::RULES_RUN,
        json!({"scenario": case.display().to_string(), "trace": true}),
    );
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["ticks"], 3);
    assert_eq!(v["label"], "invented_other.rules");
    assert!(v["text"].as_str().unwrap().starts_with("tick 0: "), "{v}");
    // Text that does not build, and a scenario that is neither.
    assert_eq!(
        code_of(api.call(&mut app, m::RULES_RUN, json!({"scenario": {"ticks": []}, "source": "rules"}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::RULES_RUN, json!({"scenario": 3}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn scenario_scripts_open_as_a_working_copy_and_its_saves_read_back() {
    let t = Temp::new("script-external");
    let mut app = app(&t);
    let mut api = Api::new();
    let dir = t.path("scripts");
    std::fs::create_dir_all(&dir).unwrap();
    let map = crate::app::scripts::tests::scripted_source(&dir, true);
    call(&mut api, &mut app, m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
    assert_eq!(call(&mut api, &mut app, m::SCRIPT_WORKING, json!({})), json!({"open": false}));

    let v = call(
        &mut api,
        &mut app,
        m::SCRIPT_EXTERNAL,
        json!({"name": "helper", "editor": "zed", "launch": false}),
    );
    let path = PathBuf::from(v["path"].as_str().unwrap());
    assert_eq!(path.extension().and_then(|e| e.to_str()), Some("hsc"));
    assert!(path.starts_with(t.path("data")), "{v}");
    assert!(Path::new(v["context"].as_str().unwrap()).is_file(), "{v}");
    let text = std::fs::read_to_string(&path).unwrap();
    let line = v["line"].as_u64().unwrap();
    assert!(text.lines().nth(line as usize - 1).unwrap().contains("helper"), "{line}: {text}");
    for c in v["commands"].as_array().unwrap() {
        assert!(c["args"][0].as_str().unwrap().ends_with(&format!(":{line}:1")), "{c}");
    }
    assert_eq!(v["working"]["open"], true);
    assert_eq!(v["working"]["edited"], false, "{v}");
    assert_eq!(
        code_of(api.call(&mut app, m::SCRIPT_EXTERNAL, json!({"name": "invented_none", "launch": false}))),
        code::INVALID_PARAMS
    );

    // A save in the editor: script.working reads and compares it.
    save_as_editor(
        &path,
        &format!("{text}\n(script static void invented_added\n  (helper))\n"),
        5,
    );
    let v = call(&mut api, &mut app, m::SCRIPT_WORKING, json!({}));
    assert_eq!(v["edited"], true, "{v}");
    assert_eq!(v["added"], json!(["invented_added"]));
    assert_eq!(v["changed"], json!([]));
    assert_eq!(v["stale"], false);
    assert!(v["summary"].as_str().unwrap().contains("invented_added added"), "{v}");
}

/// `workspace.*` over an invented disc (docs/editor/agent-api.md, "The
/// tag workspace"): no game data, and the tree goes under the test's
/// data root, never the checkout.
#[test]
fn the_workspace_reports_the_start_page_and_extracts_an_invented_disc() {
    let t = Temp::new("workspace");
    // No source open: the first start's page shows with no tree.
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let mut api = Api::new();
    let status = call(&mut api, &mut app, m::WORKSPACE_STATUS, json!({}));
    assert_eq!(status["start_page"], true);
    assert_eq!(status["has_tree"], false);
    assert_eq!(status["tree"], Value::Null);
    assert_eq!(status["sizes"]["total"], json!(0));

    // Extract every map of an invented disc.
    let disc = t.path("disc");
    std::fs::create_dir_all(&disc).unwrap();
    std::fs::write(
        disc.join("example.map"),
        crate::app::workspace::tests::invented_map("example"),
    )
    .unwrap();
    let started = call(
        &mut api,
        &mut app,
        m::WORKSPACE_EXTRACT,
        json!({"disc": disc.display().to_string()}),
    );
    assert_eq!(started["maps"], json!(["example"]));
    assert_eq!(started["root"], json!(app.tags_root().display().to_string()));
    app.wait_workspace();

    let status = call(&mut api, &mut app, m::WORKSPACE_STATUS, json!({}));
    assert_eq!(status["start_page"], false);
    assert_eq!(status["has_tree"], true);
    assert_eq!(status["tree"]["maps"], json!(1));
    assert_eq!(status["tree"]["tags"], json!(4));
    assert_eq!(status["stale"]["maps"], json!([]));
    assert_eq!(status["last"]["maps"], json!(1));
    assert!(status["sizes"]["tree"].as_u64().unwrap() > 0);
    assert_eq!(status["extracting"], Value::Null);

    // An unchanged disc is a no-op.
    call(&mut api, &mut app, m::WORKSPACE_EXTRACT, json!({}));
    app.wait_workspace();
    let status = call(&mut api, &mut app, m::WORKSPACE_STATUS, json!({}));
    assert_eq!(status["last"]["maps"], json!(0));

    // Prune on the invented tree: nothing to remove, and the answer says
    // what it looked at.
    let pruned = call(&mut api, &mut app, m::WORKSPACE_PRUNE, json!({}));
    assert_eq!(pruned["raw_removed"], json!(0));
    assert_eq!(pruned["bytes"], json!(0));
    assert_eq!(pruned["outputs_removed"], json!([]));
}

/// `project.*` and `edits.*` over an invented tree (docs/editor/agent-api.md,
/// "Projects and edit files"): a local package's map item onto the invented map, an
/// edit written by the API and the check that names the tag no layer holds.
#[test]
fn projects_and_edit_files_resolve_over_an_invented_tree() {
    let t = Temp::new("projects");
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let mut api = Api::new();
    let disc = t.path("disc");
    std::fs::create_dir_all(&disc).unwrap();
    std::fs::write(
        disc.join("example.map"),
        crate::app::workspace::tests::invented_map("example"),
    )
    .unwrap();
    call(
        &mut api,
        &mut app,
        m::WORKSPACE_EXTRACT,
        json!({"disc": disc.display().to_string()}),
    );
    app.wait_workspace();
    // A local package whose `map` item is the project.
    let package = t.path("data/packages/mine");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("package.toml"),
        "[package]\nformat = 1\nid = \"mine\"\nname = \"Mine\"\n[[item]]\nkind = \"map\"\nsource = \"map.toml\"\nmaps = \"all\"\n",
    )
    .unwrap();
    std::fs::write(
        package.join("map.toml"),
        "name = \"mine\"\nmode = \"onto\"\nbase = \"example\"\n",
    )
    .unwrap();

    let listed = call(&mut api, &mut app, m::PROJECT_LIST, json!({}));
    let mine = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "mine")
        .unwrap_or_else(|| panic!("{listed}"));
    assert_eq!(mine["origin"], "local");
    assert_eq!(mine["mode"], "onto");
    assert_eq!((&mine["package"], &mine["item"]), (&json!("mine"), &json!(0)));

    let read = call(&mut api, &mut app, m::PROJECT_READ, json!({"project": "mine"}));
    assert_eq!(read["problems"], json!([]), "{read}");
    assert_eq!(read["layers"][0]["kind"], "tree");

    let set = call(
        &mut api,
        &mut app,
        m::EDITS_SET,
        json!({"file": "mine", "tag": "jpt!:invented\\nowhere", "path": "damage_lower_bound", "value": 2.0}),
    );
    assert_eq!(set["line"], 1, "{set}");
    let files = call(&mut api, &mut app, m::EDITS_LIST, json!({}));
    assert!(
        files["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "mine" && f["origin"] == "local"),
        "{files}"
    );
    // The project's own local file applies: its tag is in no layer.
    let checked = call(&mut api, &mut app, m::EDITS_CHECK, json!({"project": "mine"}));
    assert_eq!(checked["files"], json!(["mine"]), "{checked}");
    let problems = checked["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1, "{checked}");
    assert_eq!(problems[0]["line"], 1);
    assert!(
        problems[0]["why"].as_str().unwrap().contains("no layer holds this tag"),
        "{checked}"
    );
    // The build stops on it, with the same words.
    let e = api
        .call(&mut app, m::PROJECT_BUILD, json!({"project": "mine"}))
        .unwrap_err();
    assert_eq!(e.code, code::FAILED);
    assert!(e.message.contains("no layer holds this tag"), "{}", e.message);
    let e = api
        .call(&mut app, m::PROJECT_READ, json!({"project": "nobody"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

#[test]
fn the_tags_tree_is_refused_inside_a_git_checkout() {
    let t = Temp::new("workspace-git");
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let mut api = Api::new();
    let disc = t.path("disc");
    std::fs::create_dir_all(&disc).unwrap();
    std::fs::write(
        disc.join("example.map"),
        crate::app::workspace::tests::invented_map("example"),
    )
    .unwrap();
    std::fs::create_dir_all(t.path("repo/.git")).unwrap();
    let e = api
        .call(
            &mut app,
            m::WORKSPACE_EXTRACT,
            json!({
                "disc": disc.display().to_string(),
                "root": t.path("repo/tags").display().to_string(),
            }),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("Git checkout"), "{}", e.message);
    // The root was not recorded, and nothing runs.
    assert_eq!(app.tags_root(), t.path("data").join("tags"));
    assert!(app.workspace_start_page());
    // A bad map name is a bad parameter.
    let e = api
        .call(
            &mut app,
            m::WORKSPACE_EXTRACT,
            json!({"disc": disc.display().to_string(), "maps": ["nope"]}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("nope.map"), "{}", e.message);
}

/// A map variant document of an invented map, for `variant.maps`.
fn map_variant_doc(title: &str) -> forge_doc::ForgeDoc {
    use forge_doc::{BlockKind as B, BuilderVersions, Edit, ForgeDoc, MapRef, Op, OpId, Placement, Sha256};
    let mut doc = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "fixture".into(),
            sha256: Sha256::of(b"an invented map"),
        },
        sources: vec![],
        title: title.into(),
        author: "Editor".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![Op {
            id: OpId(1),
            edit: Edit::Placement(Placement::Add {
                block: B::NetgameFlags,
                like: None,
                fields: vec![],
            }),
        }],
        expect: None,
        waves: None,
    };
    doc.format = doc.required_format();
    doc
}

#[test]
fn variant_export_suggests_the_map_variants_variant_maps_lists() {
    let t = Temp::new("variant-maps");
    let mut app = app(&t);
    let mut api = Api::new();
    let data = t.path("data");
    // None on a machine with none.
    let none = call(&mut api, &mut app, m::VARIANT_MAPS, json!({}));
    assert_eq!(none["maps"], json!([]));
    let castle = map_variant_doc("Castle");
    forge::files::variants::save(&data, &castle, false).unwrap();
    forge::files::variants::save(&data, &map_variant_doc("Tower"), false).unwrap();
    let listed = call(&mut api, &mut app, m::VARIANT_MAPS, json!({}));
    let maps = listed["maps"].as_array().unwrap();
    assert_eq!(maps.len(), 2);
    assert_eq!((maps[0]["name"].as_str(), maps[0]["map"].as_str()), (Some("Castle"), Some("fixture")));
    assert_eq!(maps[0]["variant"], castle.id().unwrap().to_string());
    assert_eq!(maps[0]["base"], castle.base.sha256.to_string());
    assert_eq!(maps[0]["source"], "saved");

    call(&mut api, &mut app, m::DOC_OPEN, json!({"variant": {"new": "slayer"}, "discard": true}));
    let out = t.path("out/suggest.h2variant");
    let r = call(
        &mut api,
        &mut app,
        m::VARIANT_EXPORT,
        json!({"path": out.display().to_string(), "maps": [maps[1]["variant"], maps[0]["variant"]]}),
    );
    assert_eq!(r["maps"], 2);
    let b = ::variants::portable::read(&std::fs::read_to_string(&out).unwrap()).unwrap();
    let got: Vec<_> = b.maps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(got, ["Tower", "Castle"], "in the order asked");
    assert_eq!(b.maps[1].base, castle.base.sha256.to_string());
    // Without maps it is the version-1 file it was.
    let plain = t.path("out/plain.h2variant");
    call(&mut api, &mut app, m::VARIANT_EXPORT, json!({"path": plain.display().to_string()}));
    assert!(std::fs::read_to_string(&plain).unwrap().contains("\"version\": 1"));
    // An id this machine has no map variant of is refused by name.
    let bad = api.call(
        &mut app,
        m::VARIANT_EXPORT,
        json!({"path": t.path("out/bad.h2variant").display().to_string(), "maps": ["0".repeat(64)]}),
    );
    assert_eq!(code_of(bad), code::INVALID_PARAMS);
    assert!(!t.path("out/bad.h2variant").exists());
}
