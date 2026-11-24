//! Where the studio's other tools plug into the agent API.
//!
//! A tool registers a [`Provider`] in [`register`]. A provider answers
//! methods ahead of the built-in ones and the planned stubs: it returns
//! `Some(reply)` for a call it answers and `None` to let the built-in
//! method (or the NOT_YET stub) answer. Its [`Provider::methods`] replace
//! the [`super::METHODS`] entries of the same name in `session.methods`
//! (so a planned method turns "ready" when its definition has
//! `planned: None`), and add new ones. A method still planned answers
//! NOT_YET before its params are checked or any provider is asked.
//!
//! Edits go through the tool's [`Action`]s and [`App::perform`], as the
//! built-in namespaces do, so the window and the API stay one path:
//!
//! - **studio-tags** (the every-field tag editor and `tags::registry`):
//!   answer `tag.read` with the registry's labelled fields, and `tag.set`,
//!   `tag.block.add`, `tag.block.remove`, `tag.block.duplicate` with the
//!   tag document's actions. Return `None` for the scenario tag's
//!   placement blocks (paths `<block>[<i>].<field>`): the built-in methods
//!   route those to the scenario's own commands.
//! - **curve functions** (the curve editor of a `view=function` data
//!   field, rich editors F1): `function.read`, `function.set` (same
//!   length, one undo step) and `function.sample` over
//!   `tags::effects::function` ([`super::curves`]).
//! - **tag browsing and bundles**: `tag.list` over the document the Tags
//!   tool shows, `tag.browse`, `tag.resources`, `tag.export` and
//!   `tag.import` through `Action::BrowseTags`, `Action::TagExport` and
//!   `Action::TagImport` ([`super::tag_io`]).
//! - **studio-hud** (the HUD editor): `hud.list`, `hud.read`, `hud.set`,
//!   `hud.move`, `hud.render`, `hud.check` and `hud.save_as` through the
//!   HUD tool's `Action::Hud*` ([`super::hud`]).
//! - **studio-modeling** (the Modeling tool): `geometry.list`, `open`,
//!   `close`, `read`, `check` and `mesh.mode`, `select`, `transform`,
//!   `ops`, `apply` through `Action::Model` ([`super::geometry`]).
//! - **Textures** (the open map's bitmaps): `bitmap.list`, `read`,
//!   `export`, `import`, `paint`, `fill`, `adjust`, `pixels` through
//!   `Action::Bitmap` ([`super::bitmaps`]).
//! - **Image generation**: `image.generate`, `image.result`,
//!   `image.apply`, `image.accept` and `image.discard` over the app's jobs
//!   (`crate::imagegen`), written through `Action::Bitmap`'s commands
//!   ([`super::image_gen`]).
//! - **Scripts** (the open map's scripts, read): `script.list`,
//!   `script.source`, `script.follow`, `script.uses`, `script.builtins` and
//!   `script.doc` through `Action::Script` ([`super::scripts`]).
//! - **game type scripts** (`app::rules_edit`): `rules.open`, `rules.get`,
//!   `rules.set`, `rules.save`, `rules.format` and `rules.reload`
//!   ([`super::game_script`]).
//! - **content checks**: `sound.measure` ([`super::sound_measure`]),
//!   `ui.check` ([`super::ui_check`]) and `check.run`, whose checks are
//!   the rows of [`super::checks::CHECKS`].
//! - **import**: `import.sound` writes a WAV into a package as a `sound`
//!   item ([`super::import`], `app::sound_import`).
//! - **models**: `model.list`, `read`, `render` and `check` over the map
//!   the scenario tool has open, or a named one, with the Pogo Stick's kit
//!   when asked ([`super::models`]); read-only, so no actions.
//!   `model.colors`, `model.emblem` and `custom.*` are planned
//!   (docs/editor/agent-api.md).
//! - **model poses**: `anim.list` and `anim.pose` over the Tags tool's
//!   own pose reading (`app::anim`), one path with the Animation window
//!   ([`super::anim`]); read-only.
//! - **the Announcer** and **Voices**: `voice.say`, `voice.list`,
//!   `voice.commit`, `voice.characters`, `voice.dataset.build`,
//!   `voice.dataset.status`, `voice.clone.train` and `voice.clone.say`
//!   ([`super::voice`], [`super::voice_clone`]) over `app::announcer`
//!   and `app::voices`.
//!
//! [`Action`]: crate::app::Action
//! [`App::perform`]: crate::App::perform

use serde_json::Value;

use super::{Api, MethodDef, Reply};
use crate::App;

/// Answers some of the API's methods.
pub trait Provider {
    /// The methods it answers, with their shapes (`planned: None`).
    fn methods(&self) -> Vec<MethodDef>;
    /// Answers `method`, or `None` to leave it to the built-in method.
    /// `params` is an object already checked against the method's shape.
    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply>;
}

/// Registers every tool's provider. Each lane adds its line here.
pub fn register(api: &mut Api) {
    // studio-tags: every field of any tag through the Tags tool's commands.
    api.register(Box::new(super::tag_fields::TagFields));
    // Curve functions of a `view=function` field, read, edited and
    // sampled (rich editors F1).
    api.register(Box::new(super::curves::Curves));
    api.register(Box::new(super::nav::Navigation));
    // The Navigation layers' data: the game's pathfinding, the bots' mesh
    // and spots, and a pick.
    api.register(Box::new(super::nav_mesh::NavMesh));
    // Playtest's static checks over the open map (app::playtest_static).
    api.register(Box::new(super::playtest::Playtest::new()));
    // The Tags tool's browsing (shared caches), resources and tag bundles.
    api.register(Box::new(super::tag_io::TagIo));
    // Sound audition: decode, play and the waveform of a snd! tag.
    api.register(Box::new(super::sound::Sound));
    api.register(Box::new(super::tags::TagGraph));
    // strings.list: every unic string by name and language, searched.
    api.register(Box::new(super::strings::Strings));
    // studio-hud: the HUD tool's HUDs, widgets, picture, checks and save.
    api.register(Box::new(super::hud::Hud));
    // studio-modeling: the Modeling tool's targets, meshes, edits and check.
    api.register(Box::new(super::geometry::Geometry));
    // The Textures tool: the open map's bitmaps, paint, import and export.
    api.register(Box::new(super::bitmaps::Bitmaps));
    // Image generation: a picture made for a bitmap slot, applied,
    // accepted or discarded through the Textures tool (app::image_gen).
    api.register(Box::new(super::image_gen::ImageGenApi));
    // The Scripts tool: the open map's scripts read, links and builtins.
    api.register(Box::new(super::scripts::Scripts));
    // pair.*: the window's pairing with a running game (play::pair).
    api.register(Box::new(super::pair::Pair));
    api.register(Box::new(super::mirror::Mirror));
    // pair.preview, pair.audio, pair.result: content shown in the paired
    // game and read back (api::preview).
    api.register(Box::new(super::preview::Preview::default()));
    // The game type script editor: open, check, save and reload a .rules
    // script (app::rules_edit).
    api.register(Box::new(super::game_script::GameScript));
    // The open map's retail screen edits written as retail edit files
    // (boot::ui_kit::retail_edits).
    api.register(Box::new(super::ui_export::UiExport));
    // uik-edit: the UI Screens tool's canvas edits, undo, save and filmstrip
    // (app::ui_screens::edit).
    api.register(Box::new(super::ui_edit::UiEdit));
    // A retail screen's tag edits past a rect: a block hidden or copied,
    // a string ID set (app::ui_screens::edit::retail_tag).
    api.register(Box::new(super::ui_retail::UiRetail));
    // Content checks: a sound's levels against the map's, our screens and
    // HUDs checked, and every check in one list (check.run).
    api.register(Box::new(super::sound_measure::SoundMeasure));
    // import.sound: a WAV added to a package as a `sound` item.
    api.register(Box::new(super::import::Import));
    // sound.edit, sound.render and sound.permutation: a package sound's
    // waveform edits and permutation files.
    api.register(Box::new(super::sound_edit::SoundEdit));
    api.register(Box::new(super::ui_check::UiCheck));
    api.register(Box::new(super::checks::Checks));
    // model.*: objects' models read, rendered (ground, third and first
    // person) and checked as content, Pogo kit maps included.
    api.register(Box::new(super::models::Models::default()));
    // markers.*: the open map's game type markers listed, and the marker
    // shapes rendered on a grid with no map.
    api.register(Box::new(super::markers::Markers::default()));
    // anim.*: a jmad's animations listed and one posed on the model that
    // uses it (app::anim), the Animation window's own path.
    api.register(Box::new(super::anim::Anim::default()));
    // The Announcer: a line said by a voice, made a clip and kept
    // (app::announcer).
    api.register(Box::new(super::voice::Voice));
    // Voices: game characters cloned from the disc: datasets, the local
    // model's jobs and lines (app::voices).
    api.register(Box::new(super::voice_clone::VoiceClone));
}
