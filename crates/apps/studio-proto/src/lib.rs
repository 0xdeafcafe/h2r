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
    /// The call writes to the edit target and none is set (`package.target`).
    pub const NO_TARGET: i64 = 16;
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

    // The document tabs (docs/editor/studio-redesign.md, "The shell").
    pub const SHELL_TABS: &str = "shell.tabs";
    pub const SHELL_OPEN: &str = "shell.open";
    pub const SHELL_FOCUS: &str = "shell.focus";
    pub const SHELL_CLOSE: &str = "shell.close";
    // The browser (docs/editor/studio-redesign.md, "The browser";
    // api/assets.rs).
    pub const ASSET_LIST: &str = "asset.list";
    pub const ASSET_THUMB: &str = "asset.thumb";
    pub const ASSET_COPY: &str = "asset.copy";
    pub const ASSET_LINKS: &str = "asset.links";
    pub const ASSET_WHERE_USED: &str = "asset.where_used";
    pub const ASSET_EDITS: &str = "asset.edits";
    pub const ASSET_DROP: &str = "asset.drop";
    pub const ASSET_FORK: &str = "asset.fork";
    // Forge map variants and the Forge palette (api/forge_variants.rs).
    pub const FORGE_VARIANT_LIST: &str = "forge.variant.list";
    pub const FORGE_VARIANT_READ: &str = "forge.variant.read";
    pub const FORGE_VARIANT_SET: &str = "forge.variant.set";
    pub const FORGE_VARIANT_SAVE: &str = "forge.variant.save";
    pub const FORGE_ITEMS: &str = "forge.items";
    pub const ASSET_REVERT: &str = "asset.revert";
    // The package document (api/packages.rs).
    pub const PACKAGE_LIST: &str = "package.list";
    pub const PACKAGE_READ: &str = "package.read";
    pub const PACKAGE_NEW: &str = "package.new";
    pub const PACKAGE_SET: &str = "package.set";
    pub const PACKAGE_ITEM_ADD: &str = "package.item.add";
    pub const PACKAGE_ITEM_SET: &str = "package.item.set";
    pub const PACKAGE_ITEM_REMOVE: &str = "package.item.remove";
    pub const PACKAGE_TARGET: &str = "package.target";
    pub const PACKAGE_ENABLE: &str = "package.enable";
    pub const PACKAGE_ORDER: &str = "package.order";
    pub const PACKAGE_CHECK: &str = "package.check";
    pub const PACKAGE_BUILD: &str = "package.build";
    pub const PACKAGE_EXPORT: &str = "package.export";
    pub const PACKAGE_PLAY: &str = "package.play";
    pub const PACKAGE_DIFF: &str = "package.diff";
    // The Problems panel (docs/editor/studio-redesign.md; api/problems.rs).
    pub const PROBLEMS_LIST: &str = "problems.list";
    // The preview switch (docs/editor/studio-redesign.md; api/preview.rs).
    pub const PREVIEW_TARGET: &str = "preview.target";
    // The tag workspace (docs/editor/tag-workspace.md, "Interfaces";
    // api/workspace.rs).
    pub const WORKSPACE_STATUS: &str = "workspace.status";
    pub const WORKSPACE_EXTRACT: &str = "workspace.extract";
    pub const WORKSPACE_IMPORT: &str = "workspace.import";
    pub const WORKSPACE_PRUNE: &str = "workspace.prune";
    // The workspace's projects and edit files (docs/editor/tag-workspace.md,
    // "Projects", "Edits are data"; api/projects.rs).
    pub const PROJECT_LIST: &str = "project.list";
    pub const PROJECT_READ: &str = "project.read";
    pub const PROJECT_BUILD: &str = "project.build";
    pub const EDITS_LIST: &str = "edits.list";
    pub const EDITS_CHECK: &str = "edits.check";
    pub const EDITS_SET: &str = "edits.set";

    pub const TAG_LIST: &str = "tag.list";
    pub const TAG_FIELDS: &str = "tag.fields";
    pub const TAG_VIEW: &str = "tag.view";
    pub const TAG_FIND: &str = "tag.find";
    pub const TAG_CHOICES: &str = "tag.choices";
    pub const TAG_NAMES: &str = "tag.names";
    pub const TAG_BYTES: &str = "tag.bytes";
    pub const TAG_RESET: &str = "tag.reset";
    pub const TAG_ELEMENTS: &str = "tag.elements";
    pub const TAG_READ: &str = "tag.read";
    pub const TAG_TREE: &str = "tag.tree";
    pub const TAG_REFS: &str = "tag.refs";
    pub const TAG_KINDS: &str = "tag.kinds";
    pub const TAG_OPEN: &str = "tag.open";
    pub const TAG_SUBJECT: &str = "tag.subject";
    pub const STRINGS_LIST: &str = "strings.list";
    pub const TAG_CHAIN: &str = "tag.chain";
    pub const TAG_USES: &str = "tag.uses";
    pub const TAG_FOLLOW: &str = "tag.follow";
    pub const TAG_SET: &str = "tag.set";
    pub const TAG_BLOCK_ADD: &str = "tag.block.add";
    pub const TAG_BLOCK_REMOVE: &str = "tag.block.remove";
    pub const TAG_BLOCK_DUPLICATE: &str = "tag.block.duplicate";
    pub const TAG_BROWSE: &str = "tag.browse";
    pub const TAG_RESOURCES: &str = "tag.resources";
    pub const TAG_EXPORT: &str = "tag.export";
    pub const TAG_IMPORT: &str = "tag.import";

    // Curve functions of a function field (api/curves.rs).
    pub const FUNCTION_READ: &str = "function.read";
    pub const FUNCTION_SET: &str = "function.set";
    pub const FUNCTION_SAMPLE: &str = "function.sample";
    pub const FUNCTION_COLOURS: &str = "function.colours";

    // Colour fields of a tag (api/colours.rs).
    pub const TAG_COLOURS: &str = "tag.colours";

    // The Shader tab (api/shader.rs; slice S1 of
    // docs/editor/rich-editors.md): a shad's slots read and written, and
    // the material drawn alone.
    pub const SHADER_READ: &str = "shader.read";
    pub const SHADER_SET: &str = "shader.set";
    pub const SHADER_PREVIEW: &str = "shader.preview";
    // The Code section (slice S4): a pass's programs as text.
    pub const SHADER_CODE: &str = "shader.code";
    // WGSL overrides from the editor (slice S5): a pass's program edited,
    // pushed to the paired game as an override file, and reverted.
    pub const SHADER_OVERRIDE: &str = "shader.override";
    pub const SHADER_PUSH: &str = "shader.push";
    pub const SHADER_REVERT: &str = "shader.revert";

    pub const SOUND_READ: &str = "sound.read";
    pub const SOUND_PLAY: &str = "sound.play";
    pub const SOUND_STOP: &str = "sound.stop";
    pub const SOUND_MOUTH: &str = "sound.mouth";
    pub const SOUND_MEASURE: &str = "sound.measure";
    // The face preview: a model's talk overlay by a mouth curve (api/anim.rs).
    pub const SOUND_FACE: &str = "sound.face";
    // Waveform edits and permutations of a package sound (api/sound_edit.rs).
    pub const SOUND_EDIT: &str = "sound.edit";
    pub const SOUND_RENDER: &str = "sound.render";
    pub const SOUND_PERMUTATION: &str = "sound.permutation";
    // A WAV imported into a package as a `sound` item (api/import.rs).
    pub const IMPORT_SOUND: &str = "import.sound";
    pub const IMPORT_MODEL: &str = "import.model";
    // A PNG imported into a package as a `bitmap` item (api/import.rs).
    pub const IMPORT_BITMAP: &str = "import.bitmap";

    // The Announcer: lines said by a voice (api/voice.rs).
    pub const VOICE_SAY: &str = "voice.say";
    pub const VOICE_LIST: &str = "voice.list";
    pub const VOICE_COMMIT: &str = "voice.commit";
    // Voices: game characters cloned from the user's disc (api/voice_clone.rs).
    pub const VOICE_CHARACTERS: &str = "voice.characters";
    pub const VOICE_DATASET_BUILD: &str = "voice.dataset.build";
    pub const VOICE_DATASET_STATUS: &str = "voice.dataset.status";
    pub const VOICE_CLONE_TRAIN: &str = "voice.clone.train";
    pub const VOICE_CLONE_SAY: &str = "voice.clone.say";
    // The Voices batch: many lines at once (api/voice_clone.rs).
    pub const VOICE_SAY_BATCH: &str = "voice.say_batch";
    pub const VOICE_BATCH_STATUS: &str = "voice.batch.status";
    pub const VOICE_BATCH_RUN: &str = "voice.batch.run";
    pub const VOICE_BATCH_SAVE: &str = "voice.batch.save";

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
    pub const GROUND_STATUS: &str = "ground.status";
    pub const GROUND_BUILD: &str = "ground.build";
    pub const GROUND_SAVE: &str = "ground.save";
    pub const SIEGE_LAYOUT: &str = "siege.layout";
    pub const MAP_NEW: &str = "map.new";
    pub const PLAYTEST_STATIC: &str = "playtest.static";
    pub const PLAYTEST_RUN: &str = "playtest.run";
    pub const PLAYTEST_STATUS: &str = "playtest.status";
    pub const PLAYTEST_RESULT: &str = "playtest.result";
    pub const PLAYTEST_CANCEL: &str = "playtest.cancel";
    pub const PLAYTEST_COMPARE: &str = "playtest.compare";

    pub const VARIANT_LIST: &str = "variant.list";
    pub const VARIANT_OPEN: &str = "variant.open";
    pub const VARIANT_NEW: &str = "variant.new";
    pub const VARIANT_GET: &str = "variant.get";
    pub const VARIANT_SET: &str = "variant.set";
    pub const VARIANT_RENAME: &str = "variant.rename";
    pub const VARIANT_COPY: &str = "variant.copy";
    pub const VARIANT_EXPORT: &str = "variant.export";
    pub const VARIANT_MAPS: &str = "variant.maps";
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

    pub const GAME_STATUS: &str = "game.status";
    pub const GAME_SNAPSHOT: &str = "game.snapshot";
    pub const GAME_START: &str = "game.start";
    pub const GAME_STOP: &str = "game.stop";
    pub const GAME_MODE: &str = "game.mode";

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
    // Game type markers (docs/game/gametype-markers.md, "Agent API").
    pub const MARKERS_LIST: &str = "markers.list";
    pub const MARKERS_RENDER: &str = "markers.render";
    pub const MARKERS_AREA_SET: &str = "markers.area.set";
    pub const MARKERS_AREA_CONVERT: &str = "markers.area.convert";
    // Forge block-out pieces (docs/game/forge-blockout.md, "Interfaces").
    pub const BLOCKOUT_RENDER: &str = "blockout.render";
    pub const BLOCKOUT_DESCRIBE: &str = "blockout.describe";
    pub const GEOMETRY_LIST: &str = "geometry.list";
    pub const GEOMETRY_OPEN: &str = "geometry.open";
    pub const GEOMETRY_CLOSE: &str = "geometry.close";
    pub const GEOMETRY_READ: &str = "geometry.read";
    pub const GEOMETRY_CHECK: &str = "geometry.check";
    pub const GEOMETRY_LOWER_LODS: &str = "geometry.lower_lods";
    pub const GEOMETRY_COLLISION_ADD_BOX: &str = "geometry.collision.add_box";
    pub const GEOMETRY_COLLISION_CLEAR: &str = "geometry.collision.clear";
    pub const MESH_MODE: &str = "mesh.mode";
    pub const MESH_SELECT: &str = "mesh.select";
    pub const MESH_TRANSFORM: &str = "mesh.transform";
    pub const MESH_OPS: &str = "mesh.ops";
    pub const MESH_APPLY: &str = "mesh.apply";
    pub const MESH_SCULPT: &str = "mesh.sculpt";

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
    pub const BITMAP_IMAGE_ADD: &str = "bitmap.image.add";
    pub const BITMAP_IMAGE_RESIZE: &str = "bitmap.image.resize";
    pub const BITMAP_SEQUENCE_ADD: &str = "bitmap.sequence.add";
    pub const BITMAP_SEQUENCE_SET: &str = "bitmap.sequence.set";
    pub const BITMAP_SPRITES: &str = "bitmap.sprites";
    pub const BITMAP_SPRITE_ADD: &str = "bitmap.sprite.add";
    pub const BITMAP_SPRITE_SET: &str = "bitmap.sprite.set";
    pub const BITMAP_SPRITE_REMOVE: &str = "bitmap.sprite.remove";
    pub const BITMAP_CHANNEL_COPY: &str = "bitmap.channel_copy";
    pub const BITMAP_TURN: &str = "bitmap.turn";

    // Image generation (docs/editor/agent-api.md, "Generating images").
    pub const IMAGE_GENERATE: &str = "image.generate";
    pub const IMAGE_RESULT: &str = "image.result";
    pub const IMAGE_APPLY: &str = "image.apply";
    pub const IMAGE_ACCEPT: &str = "image.accept";
    pub const IMAGE_DISCARD: &str = "image.discard";

    // The Scripts tool (docs/editor/rich-editors.md, "Scripts").
    // The Recordings tool (docs/editor/rich-editors.md, "Recordings").
    pub const RECORDING_LIST: &str = "recording.list";
    pub const RECORDING_READ: &str = "recording.read";
    pub const RECORDING_PLAY: &str = "recording.play";
    pub const RECORDING_PATH: &str = "recording.path";
    pub const RECORDING_WRITE: &str = "recording.write";
    pub const RECORDING_REMOVE: &str = "recording.remove";
    pub const RECORDING_UNDO: &str = "recording.undo";

    pub const SCRIPT_LIST: &str = "script.list";
    pub const SCRIPT_SOURCE: &str = "script.source";
    pub const SCRIPT_FOLLOW: &str = "script.follow";
    pub const SCRIPT_USES: &str = "script.uses";
    pub const SCRIPT_BUILTINS: &str = "script.builtins";
    pub const SCRIPT_DOC: &str = "script.doc";
    pub const SCRIPT_EXTERNAL: &str = "script.external";
    pub const SCRIPT_WORKING: &str = "script.working";
    pub const SCRIPT_COMPILE: &str = "script.compile";
    pub const SCRIPT_UNDO: &str = "script.undo";
    pub const SCRIPT_EVAL: &str = "script.eval";

    // The game type script editor (docs/editor/editor-guide.md, "Game
    // type scripts").
    pub const RULES_OPEN: &str = "rules.open";
    pub const RULES_GET: &str = "rules.get";
    pub const RULES_SET: &str = "rules.set";
    pub const RULES_SAVE: &str = "rules.save";
    pub const RULES_FORMAT: &str = "rules.format";
    pub const RULES_RELOAD: &str = "rules.reload";
    pub const RULES_EXTERNAL: &str = "rules.external";
    pub const RULES_RUN: &str = "rules.run";

    // Model poses (docs/editor/agent-api.md, "Model poses"; the M1 slice
    // of docs/editor/rich-editors.md): a `jmad` tag's animations listed
    // and one posed on the model that uses it.
    pub const ANIM_LIST: &str = "anim.list";
    pub const ANIM_POSE: &str = "anim.pose";
    // The animation editor, read (AN1 of docs/editor/studio-redesign.md):
    // one animation's channels as keys, and the events it declares.
    pub const ANIM_READ: &str = "anim.read";
    // In-memory animation documents (AN3).
    pub const ANIM_NEW: &str = "anim.new";
    pub const ANIM_DUPLICATE: &str = "anim.duplicate";
    pub const ANIM_KEY: &str = "anim.key";
    pub const ANIM_CHECK: &str = "anim.check";
    pub const ANIM_SAVE: &str = "anim.save";
    pub const ANIM_RETARGET: &str = "anim.retarget";
    pub const ANIM_IMPORT: &str = "anim.import";

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
    pub const MODEL_PHYSICS_SHAPES: &str = "model.physics.shapes";
    pub const MODEL_PHYSICS_SET: &str = "model.physics.set";
    pub const MODEL_COLLISION_FROM_RENDER: &str = "model.collision_from_render";
    pub const MODEL_RENDER: &str = "model.render";
    pub const MODEL_CHECK: &str = "model.check";
    pub const MODEL_COLORS: &str = "model.colors";
    pub const MODEL_EMBLEM: &str = "model.emblem";
    pub const CUSTOM_LIST: &str = "custom.list";
    pub const CUSTOM_SET: &str = "custom.set";
}

#[cfg(test)]
mod tests;
