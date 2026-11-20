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
//! - **tag browsing and bundles**: `tag.list` over the document the Tags
//!   tool shows, `tag.browse`, `tag.resources`, `tag.export` and
//!   `tag.import` through `Action::BrowseTags`, `Action::TagExport` and
//!   `Action::TagImport` ([`super::tag_io`]).
//! - **studio-hud** (the HUD editor): `hud.list`, `hud.read`, `hud.set`,
//!   `hud.move`, `hud.render`, `hud.check` and `hud.save_as` through the
//!   HUD tool's `Action::Hud*` ([`super::hud`]).
//! - **player models** and **customisation**: `model.*` and `custom.*`
//!   are planned the same way (docs/editor/agent-api.md).
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
    // The Tags tool's browsing (shared caches), resources and tag bundles.
    api.register(Box::new(super::tag_io::TagIo));
    // studio-hud: the HUD tool's HUDs, widgets, picture, checks and save.
    api.register(Box::new(super::hud::Hud));
    // pair.*: the window's pairing with a running game (play::pair).
    api.register(Box::new(super::pair::Pair));
}
