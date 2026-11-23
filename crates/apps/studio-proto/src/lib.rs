//! The studio agent API (docs/editor/agent-api.md): what `h2-editor`
//! answers on its socket, and the client both `h2 studio` and
//! `h2-editor call` use.
//!
//! The wire is the game's agent API's ([`agent_proto`]): JSON-RPC 2.0, one
//! JSON object per line each way, over a Unix socket. The studio only
//! listens on a local Unix socket, never TCP.
//!
//! This crate has no editor dependencies, so the `h2` CLI links it without
//! wgpu or Dear ImGui.

pub mod cli;
mod short;

pub use short::{parse_call, split};

/// Bumped when a method or a field changes meaning. Additions don't bump it.
pub const PROTOCOL: u32 = 1;

/// The environment variable naming where the studio listens (a Unix socket
/// path).
pub const ENV: &str = "H2_STUDIO";

/// Where `h2-editor` listens and `h2 studio` connects when neither is told:
/// `$H2_STUDIO`, else `studio.sock` in the user's cache directory (beside
/// the game's `agent.sock`, never in a repository).
pub fn default_socket() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os(ENV).filter(|p| !p.is_empty()) {
        return p.into();
    }
    agent_proto::cache_dir().join("studio.sock")
}

/// Where API outputs go when a call names no path: `studio/` in the user's
/// cache directory (renders, snapshots). Never a repository.
pub fn output_dir() -> std::path::PathBuf {
    agent_proto::cache_dir().join("studio")
}

/// Error codes past JSON-RPC's own (which [`agent_proto::code`] has).
pub mod code {
    pub use agent_proto::code::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE};
    /// The document the call needs is not open (no map, no variant, no
    /// playlist file).
    pub const NO_DOCUMENT: i64 = 10;
    /// The edit was checked and refused; `message` says why (a block that
    /// can't grow safely, a read-only field, a built-in variant).
    pub const REFUSED: i64 = 11;
    /// The call would drop unsaved edits; pass `"discard": true`, or save
    /// first.
    pub const UNSAVED: i64 = 12;
    /// A planned method: its namespace is designed but not built yet.
    /// `data.lane` names who builds it.
    pub const NOT_YET: i64 = 13;
    /// The call failed while running (I/O, a map that does not read).
    pub const FAILED: i64 = 14;
    /// Nothing renders here (no wgpu adapter).
    pub const NO_GPU: i64 = 15;
}

/// Method names, by namespace. `session.methods` lists them with their
/// param and result schemas.
pub mod method {
    pub const STATUS: &str = "session.status";
    pub const METHODS: &str = "session.methods";
    pub const LOG: &str = "session.log";

    pub const SOURCE_OPEN: &str = "source.open";
    pub const SOURCE_MAPS: &str = "source.maps";

    pub const DOC_LIST: &str = "doc.list";
    pub const DOC_OPEN: &str = "doc.open";
    pub const DOC_SAVE: &str = "doc.save";
    pub const DOC_SAVE_AS: &str = "doc.save_as";
    pub const DOC_DIFF: &str = "doc.diff";
    pub const DOC_UNDO: &str = "doc.undo";
    pub const DOC_REDO: &str = "doc.redo";
    pub const DOC_CLOSE: &str = "doc.close";

    pub const TAG_LIST: &str = "tag.list";
    pub const TAG_FIELDS: &str = "tag.fields";
    pub const TAG_VIEW: &str = "tag.view";
    pub const TAG_CHOICES: &str = "tag.choices";
    pub const TAG_BYTES: &str = "tag.bytes";
    pub const TAG_RESET: &str = "tag.reset";
    pub const TAG_ELEMENTS: &str = "tag.elements";
    pub const TAG_READ: &str = "tag.read";
    pub const TAG_TREE: &str = "tag.tree";
    pub const TAG_REFS: &str = "tag.refs";
    pub const TAG_CHAIN: &str = "tag.chain";
    pub const TAG_FOLLOW: &str = "tag.follow";
    pub const TAG_SET: &str = "tag.set";
    pub const TAG_BLOCK_ADD: &str = "tag.block.add";
    pub const TAG_BLOCK_REMOVE: &str = "tag.block.remove";
    pub const TAG_BLOCK_DUPLICATE: &str = "tag.block.duplicate";
    pub const TAG_BROWSE: &str = "tag.browse";
    pub const TAG_RESOURCES: &str = "tag.resources";
    pub const TAG_EXPORT: &str = "tag.export";
    pub const TAG_IMPORT: &str = "tag.import";

    pub const SOUND_READ: &str = "sound.read";
    pub const SOUND_PLAY: &str = "sound.play";
    pub const SOUND_STOP: &str = "sound.stop";
    pub const SOUND_MEASURE: &str = "sound.measure";

    // Every content check in one list (api/checks.rs).
    pub const CHECK_RUN: &str = "check.run";

    pub const SCENARIO_BLOCKS: &str = "scenario.blocks";
    pub const SCENARIO_LIST: &str = "scenario.list";
    pub const SCENARIO_GET: &str = "scenario.get";
    pub const SCENARIO_SET: &str = "scenario.set";
    pub const SCENARIO_ADD: &str = "scenario.add";
    pub const SCENARIO_MOVE: &str = "scenario.move";
    pub const SCENARIO_DELETE: &str = "scenario.delete";
    pub const SCENARIO_DUPLICATE: &str = "scenario.duplicate";
    pub const SCENARIO_SELECT: &str = "scenario.select";
    pub const VOLUME_LIST: &str = "volume.list";
    pub const VOLUME_SELECT: &str = "volume.select";
    pub const VOLUME_SET: &str = "volume.set";
    pub const VOLUME_ADD: &str = "volume.add";
    pub const VOLUME_REMOVE: &str = "volume.remove";
    pub const COLLISION_SURFACES: &str = "collision.surfaces";
    pub const COLLISION_AT: &str = "collision.at";
    pub const COLLISION_SELECT: &str = "collision.select";
    pub const COLLISION_OBJECTS: &str = "collision.objects";
    pub const NAV_STATUS: &str = "nav.status";
    pub const NAV_BACK: &str = "nav.back";
    pub const NAV_FORWARD: &str = "nav.forward";
    pub const NAV_GO: &str = "nav.go";
    pub const NAV_PATHFINDING: &str = "nav.pathfinding";
    pub const NAV_BOTS: &str = "nav.bots";
    pub const NAV_PICK: &str = "nav.pick";
    pub const PLAYTEST_STATIC: &str = "playtest.static";

    pub const VARIANT_LIST: &str = "variant.list";
    pub const VARIANT_OPEN: &str = "variant.open";
    pub const VARIANT_NEW: &str = "variant.new";
    pub const VARIANT_GET: &str = "variant.get";
    pub const VARIANT_SET: &str = "variant.set";
    pub const VARIANT_RENAME: &str = "variant.rename";
    pub const VARIANT_COPY: &str = "variant.copy";
    pub const VARIANT_EXPORT: &str = "variant.export";
    pub const VARIANT_IMPORT: &str = "variant.import";
    pub const VARIANT_DELETE: &str = "variant.delete";

    pub const PLAYLIST_OPEN: &str = "playlist.open";
    pub const PLAYLIST_LIST: &str = "playlist.list";
    pub const PLAYLIST_GET: &str = "playlist.get";
    pub const PLAYLIST_SET: &str = "playlist.set";
    pub const PLAYLIST_ENTRY: &str = "playlist.entry";
    pub const PLAYLIST_ENTRY_VARIANT: &str = "playlist.entry_variant";

    pub const UI_PALETTE: &str = "ui.palette";
    pub const UI_SCREENS: &str = "ui.screens";
    pub const UI_PICK: &str = "ui.pick";
    pub const UI_EXPORT_EDITS: &str = "ui.export_edits";
    pub const UI_PRESS: &str = "ui.press";
    pub const UI_WIDGETS: &str = "ui.widgets";
    pub const UI_RENDER: &str = "ui.render";
    pub const UI_CONSISTENCY: &str = "ui.consistency";
    pub const UI_CHECK: &str = "ui.check";
    pub const UI_COMPONENTS: &str = "ui.components";
    pub const UI_SELECT_FOLDER: &str = "ui.select_folder";
    pub const UI_SHEET: &str = "ui.sheet";
    pub const UI_SELECT: &str = "ui.select";
    pub const UI_MOVE: &str = "ui.move";
    pub const UI_SET_RECT: &str = "ui.set_rect";
    pub const UI_SET_TEXT: &str = "ui.set_text";
    pub const UI_REVERT: &str = "ui.revert";
    pub const UI_UNDO: &str = "ui.undo";
    pub const UI_REDO: &str = "ui.redo";
    pub const UI_EDITS: &str = "ui.edits";
    pub const UI_SAVE: &str = "ui.save";
    pub const UI_ANIMATE: &str = "ui.animate";
    pub const UI_HIDE: &str = "ui.hide";
    pub const UI_DUPLICATE: &str = "ui.duplicate";
    pub const UI_SET_STRING: &str = "ui.set_string";

    pub const PAIR_LIST: &str = "pair.list";
    pub const PAIR_STATUS: &str = "pair.status";
    pub const PAIR_CONNECT: &str = "pair.connect";
    pub const PAIR_DISCONNECT: &str = "pair.disconnect";
    pub const PAIR_AUTO: &str = "pair.auto";
    pub const PAIR_PLAY: &str = "pair.play";
    pub const PAIR_PLAYTEST: &str = "pair.playtest";
    pub const PAIR_SETTINGS: &str = "pair.settings";
    pub const PAIR_STOP: &str = "pair.stop";
    pub const PAIR_EDIT: &str = "pair.edit";
    pub const PAIR_CONTROL: &str = "pair.control";
    pub const PAIR_PREVIEW: &str = "pair.preview";
    pub const PAIR_AUDIO: &str = "pair.audio";
    pub const PAIR_RESULT: &str = "pair.result";

    pub const MIRROR_STATUS: &str = "mirror.status";
    pub const MIRROR_PENDING: &str = "mirror.pending";
    pub const MIRROR_APPLY: &str = "mirror.apply";
    pub const MIRROR_PICK: &str = "mirror.pick";
    pub const MIRROR_AIM_SELECT: &str = "mirror.aim_select";
    pub const MIRROR_CAMERA: &str = "mirror.camera";
    pub const MIRROR_CONTROL: &str = "mirror.control";
    pub const MIRROR_PLAYER: &str = "mirror.player";
    pub const MIRROR_OBJECTS: &str = "mirror.objects";

    pub const VIEW_CAMERA: &str = "view.camera";
    pub const VIEW_LAYERS: &str = "view.layers";
    pub const VIEW_LIGHTING: &str = "view.lighting";
    pub const VIEW_SNAPSHOT: &str = "view.snapshot";
    pub const VIEW_STATS: &str = "view.stats";
    pub const GEOMETRY_LIST: &str = "geometry.list";
    pub const GEOMETRY_OPEN: &str = "geometry.open";
    pub const GEOMETRY_CLOSE: &str = "geometry.close";
    pub const GEOMETRY_READ: &str = "geometry.read";
    pub const GEOMETRY_CHECK: &str = "geometry.check";
    pub const GEOMETRY_COLLISION_ADD_BOX: &str = "geometry.collision.add_box";
    pub const GEOMETRY_COLLISION_CLEAR: &str = "geometry.collision.clear";
    pub const MESH_MODE: &str = "mesh.mode";
    pub const MESH_SELECT: &str = "mesh.select";
    pub const MESH_TRANSFORM: &str = "mesh.transform";
    pub const MESH_OPS: &str = "mesh.ops";
    pub const MESH_APPLY: &str = "mesh.apply";

    // The Textures tool (docs/editor/modeling.md, "Texture editing").
    pub const BITMAP_LIST: &str = "bitmap.list";
    pub const BITMAP_READ: &str = "bitmap.read";
    pub const BITMAP_EXPORT: &str = "bitmap.export";
    pub const BITMAP_IMPORT: &str = "bitmap.import";
    pub const BITMAP_PAINT: &str = "bitmap.paint";
    pub const BITMAP_FILL: &str = "bitmap.fill";
    pub const BITMAP_ADJUST: &str = "bitmap.adjust";
    pub const BITMAP_PIXELS: &str = "bitmap.pixels";
    pub const BITMAP_RESHAPE: &str = "bitmap.reshape";
    pub const BITMAP_SPANS: &str = "bitmap.spans";

    // The Scripts tool (docs/editor/rich-editors.md, "Scripts").
    pub const SCRIPT_LIST: &str = "script.list";
    pub const SCRIPT_SOURCE: &str = "script.source";
    pub const SCRIPT_FOLLOW: &str = "script.follow";
    pub const SCRIPT_USES: &str = "script.uses";
    pub const SCRIPT_BUILTINS: &str = "script.builtins";
    pub const SCRIPT_DOC: &str = "script.doc";

    // The game type script editor (docs/editor/editor-guide.md, "Game
    // type scripts").
    pub const RULES_OPEN: &str = "rules.open";
    pub const RULES_GET: &str = "rules.get";
    pub const RULES_SET: &str = "rules.set";
    pub const RULES_SAVE: &str = "rules.save";
    pub const RULES_FORMAT: &str = "rules.format";
    pub const RULES_RELOAD: &str = "rules.reload";

    // Planned namespaces: listed by `session.methods` with status
    // "planned"; a call answers NOT_YET until its lane registers it.
    pub const HUD_LIST: &str = "hud.list";
    pub const HUD_READ: &str = "hud.read";
    pub const HUD_SET: &str = "hud.set";
    pub const HUD_MOVE: &str = "hud.move";
    pub const HUD_RENDER: &str = "hud.render";
    pub const HUD_CHECK: &str = "hud.check";
    pub const HUD_SAVE_AS: &str = "hud.save_as";
    pub const MODEL_LIST: &str = "model.list";
    pub const MODEL_READ: &str = "model.read";
    pub const MODEL_RENDER: &str = "model.render";
    pub const MODEL_CHECK: &str = "model.check";
    pub const MODEL_COLORS: &str = "model.colors";
    pub const MODEL_EMBLEM: &str = "model.emblem";
    pub const CUSTOM_LIST: &str = "custom.list";
    pub const CUSTOM_SET: &str = "custom.set";
}

#[cfg(test)]
mod tests;
