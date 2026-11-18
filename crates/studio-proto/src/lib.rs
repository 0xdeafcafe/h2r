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
    pub const TAG_READ: &str = "tag.read";
    pub const TAG_REFS: &str = "tag.refs";
    pub const TAG_FOLLOW: &str = "tag.follow";
    pub const TAG_SET: &str = "tag.set";
    pub const TAG_BLOCK_ADD: &str = "tag.block.add";
    pub const TAG_BLOCK_REMOVE: &str = "tag.block.remove";
    pub const TAG_BLOCK_DUPLICATE: &str = "tag.block.duplicate";

    pub const SCENARIO_BLOCKS: &str = "scenario.blocks";
    pub const SCENARIO_LIST: &str = "scenario.list";
    pub const SCENARIO_GET: &str = "scenario.get";
    pub const SCENARIO_SET: &str = "scenario.set";
    pub const SCENARIO_ADD: &str = "scenario.add";
    pub const SCENARIO_MOVE: &str = "scenario.move";
    pub const SCENARIO_DELETE: &str = "scenario.delete";
    pub const SCENARIO_DUPLICATE: &str = "scenario.duplicate";
    pub const SCENARIO_SELECT: &str = "scenario.select";

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

    pub const UI_SCREENS: &str = "ui.screens";
    pub const UI_PICK: &str = "ui.pick";
    pub const UI_PRESS: &str = "ui.press";
    pub const UI_WIDGETS: &str = "ui.widgets";
    pub const UI_RENDER: &str = "ui.render";

    pub const VIEW_CAMERA: &str = "view.camera";
    pub const VIEW_SNAPSHOT: &str = "view.snapshot";

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
    pub const MODEL_COLORS: &str = "model.colors";
    pub const MODEL_EMBLEM: &str = "model.emblem";
    pub const CUSTOM_LIST: &str = "custom.list";
    pub const CUSTOM_SET: &str = "custom.set";
}

#[cfg(test)]
mod tests;
