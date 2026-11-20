//! Hot iteration methods (docs/graphics/hot-iteration.md): live tag edits
//! (`edit.*`) and the host renderer's overrides, parameters and toggles
//! (`render.*`). Host methods: answered where the game's host registers
//! them (crates/extensions/agent's `hot`), so `session.methods` lists them there.
//!
//! Params and results are JSON objects, documented per method below and
//! in the design page; tag and program ids are hex strings.

/// Method names.
pub mod method {
    /// The loaded map's tags: `{"group"?: "shad", "name"?: "<substring>",
    /// "limit"?: 200}` → `{"scenario", "names", "total", "tags": [{"tag",
    /// "group", "name", "address", "size", "defined"}]}`.
    pub const EDIT_TAGS: &str = "edit.tags";
    /// A field by path, or an element's fields: `{"tag": "0xe1230045" |
    /// "<name>", "group"?, "path"?: "postprocess[0].colors[1].color"}`.
    pub const EDIT_GET: &str = "edit.get";
    /// Writes a field, journalled: `{"tag", "group"?, "path", "value"}` →
    /// the change (`{"id", "old", "new", ...}`).
    pub const EDIT_SET: &str = "edit.set";
    /// The journal: every change, `live` (still holds) and `stale` (made on
    /// another map).
    pub const EDIT_CHANGES: &str = "edit.changes";
    /// Puts back one change (`{"id": 3}`) or all (no params).
    pub const EDIT_REVERT: &str = "edit.revert";

    /// The override directory, what is loaded and how each fared.
    pub const RENDER_OVERRIDES: &str = "render.overrides";
    /// Reads the override directory again (`{"dir"?: "<path>"}` moves it).
    pub const RENDER_RELOAD: &str = "render.reload";
    /// The translated programs, by id: `{"vertex"?: "microcode",
    /// "limit"?: 500}`.
    pub const RENDER_PROGRAMS: &str = "render.programs";
    /// Writes translated WGSL, the start of an override: `{"program"?:
    /// "<id>", "to"?: "<dir>"}` (all programs to `<override dir>/translated`
    /// by default). Served at the next frame; `render.overrides` reports it.
    pub const RENDER_DUMP: &str = "render.dump";
    /// Parameters: none → the list; `{"name"}` → one; `{"name", "value":
    /// 0.5}` sets it; `{"name", "value": null}` resets it.
    pub const RENDER_PARAM: &str = "render.param";
    /// Toggles: none → the list; `{"name", "on": false}` sets it;
    /// `{"name", "on": null}` resets it.
    pub const RENDER_TOGGLE: &str = "render.toggle";

    /// Every hot method.
    pub const ALL: &[&str] = &[
        EDIT_TAGS,
        EDIT_GET,
        EDIT_SET,
        EDIT_CHANGES,
        EDIT_REVERT,
        RENDER_OVERRIDES,
        RENDER_RELOAD,
        RENDER_PROGRAMS,
        RENDER_DUMP,
        RENDER_PARAM,
        RENDER_TOGGLE,
    ];
}

/// Where overrides are read from when nothing says: `$H2_RENDER_OVERRIDES`,
/// else `render-overrides` in the cache directory ([`crate::cache_dir`]).
pub fn default_override_dir() -> std::path::PathBuf {
    match std::env::var_os("H2_RENDER_OVERRIDES").filter(|v| !v.is_empty()) {
        Some(d) => d.into(),
        None => crate::cache_dir().join("render-overrides"),
    }
}
