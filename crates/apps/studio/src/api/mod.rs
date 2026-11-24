//! The studio agent API (docs/editor/agent-api.md): every edit the window
//! makes, as a JSON method over a local socket, in the style of the game's
//! `h2 agent`.
//!
//! - [`METHODS`]: every method with its params, result and an example;
//!   `session.methods` serves it, so the API describes itself.
//! - [`Api::call`]: checks the params against the method's shape, then
//!   runs it. Edits become [`Action`]s through [`App::perform`], the same
//!   path the window's clicks take ([`App::dispatch`]), so the window and
//!   the API make identical documents.
//! - [`server`]: the Unix socket. Connection threads only queue; whoever
//!   owns the [`App`] answers ([`Api::pump`]): the window between frames
//!   (the user sees an agent's edits live), or `h2-editor --serve`.
//! - [`seams`]: where the parallel tools plug in. A [`Provider`] answers
//!   methods ahead of the built-in ones: the Tags tool's `tag.*` and the
//!   HUD tool's `hud.*`, and `model.*`; `model.colors`, `model.emblem` and
//!   `custom.*` are planned, answering NOT_YET until one does.
//!
//! [`Action`]: crate::app::Action
//! [`App::dispatch`]: crate::App::dispatch

use agent_proto::{Request, Response, RpcError, memory_forensics as mem};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use crate::App;

mod anim;
mod assets;
mod bitmaps;
pub(crate) mod checks;
mod problems;
mod collision;
mod colours;
mod shader;
mod curves;
mod docs;
mod game_script;
mod geometry;
mod hud;
mod image_gen;
mod import;
mod markers;
mod markers_check;
mod mirror;
mod models;
mod nav;
mod game;
mod ground;
mod nav_mesh;
mod pair;
mod params;
mod packages;
mod preview;
mod playlists;
mod playtest;
mod scenario;
pub(crate) mod recordings;
pub(crate) mod scripts;
pub mod seams;
mod shell;
pub mod server;
pub(crate) mod sound;
mod strings;
pub(crate) mod sound_edit;
pub(crate) mod sound_measure;
mod tag_fields;
mod tag_io;
mod tag_names;
mod tags;
mod ui_check;
mod ui_kit;
mod ui_browse;
mod ui_export;
mod ui_edit;
mod ui_retail;
mod ui_screens;
mod variants;
mod view;
mod voice;
mod voice_clone;
mod workspace;

#[cfg(test)]
mod tests;

pub use params::P;
pub(crate) use params::block_name;
pub use seams::Provider;

/// One method: what it does, its params and result, and an example call.
///
/// `params` and `result` are shapes: a JSON object whose keys are the
/// fields (a trailing `?` marks an optional one) and whose values name the
/// type (`"string"`, `"integer"`, `"number"`, `"boolean"`, `"[x,y,z]"`,
/// `"any"`, or a nested shape). [`Api::call`] refuses a param the shape
/// does not name, and a missing required one.
#[derive(Clone, Debug)]
pub struct MethodDef {
    pub name: &'static str,
    pub summary: &'static str,
    pub params: &'static str,
    pub result: &'static str,
    /// Example params (`null` for none).
    pub example: &'static str,
    /// Planned and not built: who builds it.
    pub planned: Option<&'static str>,
}

const fn def(
    name: &'static str,
    summary: &'static str,
    params: &'static str,
    result: &'static str,
    example: &'static str,
) -> MethodDef {
    MethodDef {
        name,
        summary,
        params,
        result,
        example,
        planned: None,
    }
}

const fn planned(
    name: &'static str,
    summary: &'static str,
    params: &'static str,
    lane: &'static str,
) -> MethodDef {
    MethodDef {
        name,
        summary,
        params,
        result: "{}",
        example: "null",
        planned: Some(lane),
    }
}

/// Every method, in the order `session.methods` lists them.
pub const METHODS: &[MethodDef] = &[
    def(
        agent_proto::method::HOST_MEMORY,
        "Process footprint, limits, recent samples, probes and report files",
        r#"{"report?":"boolean"}"#,
        r#"{"footprint_bytes":"integer","peak_bytes":"integer","soft":"integer","hard":"integer","growth_bytes_per_min":"number","samples":"[[t_ms,bytes]]","probes":"object","reports":"[path]"}"#,
        "null",
    ),
    def(
        mem::HISTORY,
        "Read a page of the editor's memory flight recorder",
        r#"{"since_ms?":"integer"}"#,
        r#"{"records":"[record]","next_since_ms":"integer|null"}"#,
        "null",
    ),
    def(
        mem::HEAP,
        "Live allocation counts and bytes per tag, and top live sampled stacks",
        r#"{"top?":"integer (1..128, default 20)"}"#,
        r#"{"tags":"object","stacks":"[stack]","sample_interval_bytes":"integer","dropped_samples":"integer"}"#,
        r#"{"top":20}"#,
    ),
    def(
        mem::GPU,
        "Live graphics resource counts, tracked bytes and driver totals",
        "{}",
        r#"{"metric name":"integer"}"#,
        "null",
    ),
    def(
        mem::DIFF,
        "Rank memory growth between a recorded uptime and the latest record",
        r#"{"from_ms":"integer"}"#,
        r#"{"from_ms":"integer","to_ms":"integer","footprint_delta":"integer","growers":"[change]"}"#,
        r#"{"from_ms":0}"#,
    ),
    def(
        mem::POSTMORTEM,
        "Explain a dead process's recording in the memory report directory",
        r#"{"pid?":"integer"}"#,
        r#"{"pid":"integer","file":"path","diff":"object","reports":"[path]","report":"object|null"}"#,
        "null",
    ),
    def(
        m::STATUS,
        "The editor: protocol, window or headless, the tool shown, the source and every document",
        "{}",
        r#"{"protocol":"integer","window":"boolean","tool":"string","source":"string|null","docs":"[doc.list entry]"}"#,
        "null",
    ),
    def(
        m::METHODS,
        "Every method with its params, result, an example and whether it is built",
        "{}",
        r#"[{"name":"string","summary":"string","params":"shape","result":"shape","example":"any","status":"ready|planned","lane?":"string"}]"#,
        "null",
    ),
    def(
        m::LOG,
        "The editor's log, newest last",
        r#"{"last?":"integer (default 20)"}"#,
        r#"[{"at":"number","error":"boolean","text":"string"}]"#,
        r#"{"last":5}"#,
    ),
    def(
        m::SOURCE_OPEN,
        "Open a disc image, a .map file or a folder of maps as the source (a lone .map opens straight into the scenario tool)",
        r#"{"path":"string","discard?":"boolean"}"#,
        r#"{"maps":"[string]","loaded":"string|null","log":"[string]"}"#,
        r#"{"path":"$MAP"}"#,
    ),
    def(
        m::SOURCE_MAPS,
        "The source maps and library entries with their picker groups, and the map loaded",
        "{}",
        r#"{"source":"string|null","maps":"[string]","map_entries":"[{map,label,group}]","halo1_maps":"[{map,label,group}]","map_pack":"[{map,label,group}]","loaded":"string|null","packages":"[{key,map,label,group}]"}"#,
        "null",
    ),
    def(
        m::ASSET_LIST,
        "The browser's rows. root tags (the default): every tag of the tags tree by path, one row for a tag all its maps hold alike and `variants` (each with its maps) where they differ, plus the tags packages add (`new`, badge `new in <package>`); a tag an edit line touches has badge `edited by <package>`. root packages: the set (a package per row), or with `package` that package's items. root live: the server's offer (none is held offline). `kind` is Maps, Objects, Models, Animations, Bitmaps, Shaders, Effects, Sounds, Screens, HUDs, Strings, Other, a tag group, or Voices (a `voice:<id>` row for each character and the announcer, with `state` no dataset, dataset, fine-tuned or announcer, `minutes`, `pending_asr` and `tuned`); `map` keeps what that map holds; every word of `query` must match the path, group, kind or a package id",
        r#"{"root?":"tags|packages|live","package?":"string","kind?":"string","map?":"string","query?":"string","limit?":"integer"}"#,
        r#"{"root":"string","rows":"[{asset,root,kind,name,group?,path?,origin?,new?,badge?,packages?,maps?,variants?,...}]","total":"integer","limit":"integer","truncated":"boolean","maps":"[string]","kinds":"[string]","problems":"[string]"}"#,
        r#"{"kind":"Objects","query":"rifle"}"#,
    ),
    def(
        m::ASSET_THUMB,
        "A thumbnail as a PNG path: for a package's bitmap item (item:<package>:<n>) with a PNG source. A tag's own picture (a bitmap, a model, a waveform, a screen) answers not-yet until the renderer draws it",
        r#"{"asset":"string","out?":"string"}"#,
        r#"{"asset":"string","path":"string","bytes":"integer","source":"string"}"#,
        r#"{"asset":"item:kit-test:2"}"#,
    ),
    def(
        m::ASSET_COPY,
        "Copy into a package: a new `tag` item `like:` the tag of the tree, in a package under <data>/packages/ (the source file holds no bytes of the disc's tag; the lock is rewritten). The new path is `to`, or the tag's path with _copy added",
        r#"{"asset":"string (tag:<group>:<path>)","package":"string","to?":"string"}"#,
        r#"{"package":"string","from":"string","tag":"string","source":"string","item":"integer","dir":"string"}"#,
        r#"{"asset":"tag:weap:objects\\weapons\\rifle","package":"kit-test"}"#,
    ),
    def(
        m::ASSET_LINKS,
        "What an asset uses, what uses it and which editors open it, read each time (docs/editor/studio-flow.md, \"Links\"). A tag: the tags it references (role = the target's kind: Model, Animations, Sounds, HUD, Projectile, Bitmaps, ...; `via` is `node <n> +<offset>` in its file), the voice that says it when it is a dialogue sound, and in used_by the tags that reference it and the packages that add or edit it. A `rules` or `variant` item: the other kind of its script key. Any other item: the tag it adds or edits, and its package. A package: its items and `requires`, and the packages that require it. A voice: in used_by the dialogue sounds its rules pick, in uses the target sounds of the lines it made. `editors` lists each editor with `why_not` when it cannot open. `problems` names what could not be read (a tag file that does not decode, a missing tree)",
        r#"{"asset":"string (tag:<group>:<path>, package:<id>, item:<package>:<n> or voice:<id>)"}"#,
        r#"{"asset":"string","uses":"[{asset,role,via}]","used_by":"[{asset,role,via}]","editors":"[{id,label,why_not}]","problems":"[string]"}"#,
        r#"{"asset":"package:kit-test"}"#,
    ),
    def(
        m::ASSET_FORK,
        "Fork for this map: the lines of a tag as they are for one map, in an `edits` item of a package under <data>/packages/ (`package`, else the edit target) whose `maps` names that map alone (`edits/<map>.yaml`). It is never a copy of the tag: every other level reads the asset as it was. Without `set` the lines are the ones the package's other `edits` items that reach the map have for the tag (the last to set a path wins); a package with none is refused, and so is a tag already forked for the map. With `set`, those lines are put in by path. The lock is rewritten",
        r#"{"tag":"string (<group>:<path> or tag:<group>:<path>)","map":"string","package?":"string","set?":"[{path,value,from?}]"}"#,
        r#"{"package":"string","tag":"string","map":"string","file":"string","item":"integer|null","lines":"integer","from_items":"[integer]","created":"boolean"}"#,
        r#"{"tag":"weap:objects\\weapons\\rifle","map":"example","package":"kit-test"}"#,
    ),
    def(
        m::ASSET_REVERT,
        "Take a tag's edit lines out of a package's `edits` items (`package`, else the edit target). With `map`, only the item that names that map alone (the fork `asset.fork` made); without, every `edits` item with an entry for the tag. An item left with no entry goes with its file, and the lock is rewritten. A tag with no line is refused",
        r#"{"tag":"string (<group>:<path> or tag:<group>:<path>)","map?":"string","package?":"string"}"#,
        r#"{"package":"string","tag":"string","map":"string|null","removed":"[{item,file,maps,lines,item_removed}]"}"#,
        r#"{"tag":"weap:objects\\weapons\\rifle","map":"example","package":"kit-test"}"#,
    ),
    def(
        m::PACKAGE_DIFF,
        "What a package (`id`, else the edit target) does, per asset, for a map of the open source or for every map: `edit` rows hold each field's `to` (and `from` when the line names the value it replaces) and the block lines; `patch` (a tag-delta), `override` (a `replaces`) and `added` rows name the asset; `forked` marks an item that names maps. `levels` are the maps the package changes. With `map`, the whole set is built over that map and `conflicts` holds the conflict table's rows (a package off for the level with the reason, a field or an asset a later package takes), `involves` marking the ones that name this package; Problems lists them under `package` until the level is diffed again",
        r#"{"id?":"string","map?":"string"}"#,
        r#"{"id":"string","map":"string|null","assets":"[{asset,item,item_asset,change: edit|patch|override|added,maps,forked,fields?,blocks?,was?,replaces?}]","counts":"{edit,patch,override,added,forked}","levels":"[string]","built":"boolean","conflicts":"[{map,kind: refused|override,package,why,involves,other?,tag?,path?}]","problems":"[string]"}"#,
        r#"{"id":"kit-test"}"#,
    ),
    def(
        m::PACKAGE_LIST,
        "The package set in load order, then the packages that are off: id, name, PackageId, origin (baked or local), dir, on or off and why, always_on, item counts by kind, scopes, whether it is the edit target and whether it is editable; the set's problems, its content hash and the edit target",
        "{}",
        r#"{"packages":"[{id,name,package_id,origin,dir?,on,why?,always_on,signed,features,items,scopes,target,editable}]","problems":"[string]","content_hash":"string|null","target":"string|null","dir":"string"}"#,
        "null",
    ),
    def(
        m::PACKAGE_READ,
        "One package: its manifest (`package`, `credit`), each item with what it builds (`builder`, `builds`, `scope`, `changes_map`, `asset`), its lock and its findings (what package.check says)",
        r#"{"id":"string"}"#,
        r#"{"id":"string","package_id":"string","origin":"baked|local","dir":"string|null","editable":"boolean","on":"boolean","why":"string|null","target":"boolean","package":"object","credit":"object","items":"[{index,asset,kind,source?,inputs?,tag?,maps,builder,builds,scope,changes_map}]","lock":"[{file,sha256}]","problems":"[{kind,message,item?,file?,line?}]"}"#,
        r#"{"id":"kit-test"}"#,
    ),
    def(
        m::PACKAGE_NEW,
        "A new empty package under <data>/packages/<id>/ with its manifest and lock. The id is lowercase letters, digits and '-', not taken (by a baked package too) and not always-on",
        r#"{"id":"string","name":"string"}"#,
        r#"{"id":"string","package_id":"string","items":"integer","dir":"string"}"#,
        r#"{"id":"mine","name":"Mine"}"#,
    ),
    def(
        m::PACKAGE_SET,
        "Set a manifest field of a local package: name, description, image, requires, after, features, or credit.<field> (authors, notes, ...). A missing or null value removes an optional field. `id` and `format` are not edited. Refused when the manifest, its files or the set (a cycle, a missing always-on rule) would not read; the old manifest stays. The lock is rewritten",
        r#"{"id":"string","path":"string","value?":"any"}"#,
        r#"{"id":"string","package_id":"string","items":"integer","path":"string","value":"any"}"#,
        r#"{"id":"kit-test","path":"description","value":"A kit for tests."}"#,
    ),
    def(
        m::PACKAGE_ITEM_ADD,
        "Add an item to a local package (`id`, else the edit target): `item` is its manifest form {kind, source?, inputs?, tag?, maps?}, put at `at` or last. Its files must already be in the package folder (no .. or absolute paths). Refused with the manifest's or the set's reason; the lock is rewritten",
        r#"{"id?":"string","item":"object","at?":"integer"}"#,
        r#"{"id":"string","package_id":"string","items":"integer","item":"integer","asset":"string"}"#,
        r#"{"id":"kit-test","item":{"kind":"data","source":"data/note.txt"}}"#,
    ),
    def(
        m::PACKAGE_ITEM_SET,
        "Set one field of an item of a local package (`id`, else the edit target): kind, source, inputs, tag or maps; a missing or null value removes an optional one",
        r#"{"id?":"string","index":"integer","path":"string","value?":"any"}"#,
        r#"{"id":"string","package_id":"string","items":"integer","item":"integer","path":"string"}"#,
        r#"{"id":"kit-test","index":0,"path":"maps","value":"all"}"#,
    ),
    def(
        m::PACKAGE_ITEM_REMOVE,
        "Remove an item from a local package's manifest (`id`, else the edit target); its files stay in the folder",
        r#"{"id?":"string","index":"integer"}"#,
        r#"{"id":"string","package_id":"string","items":"integer","index":"integer","removed":"object"}"#,
        r#"{"id":"kit-test","index":0}"#,
    ),
    def(
        m::PACKAGE_TARGET,
        "Read the edit target (the package that receives every edit), set it to a local package, or clear it. A baked package is refused. With no target, the calls that write to it answer NO_TARGET",
        r#"{"id?":"string","clear?":"boolean"}"#,
        r#"{"target":"string|null","name?":"string","dir?":"string","stale?":"string","why?":"string"}"#,
        r#"{"id":"kit-test"}"#,
    ),
    def(
        m::PACKAGE_ENABLE,
        "Switch a package on or off in <data>/packages/set.toml. `core` and `ui` are always on: turning one off is refused by name. The answer lists the packages the change leaves out because they require this one",
        r#"{"id":"string","on":"boolean"}"#,
        r#"{"id":"string","on":"boolean","off":"[string]","left_out":"[{package,why}]","order":"[string]"}"#,
        r#"{"id":"kit-test","on":false}"#,
    ),
    def(
        m::PACKAGE_ORDER,
        "Write the user's load order to set.toml: package ids, first to load first (ties the dependencies leave). An order that puts a package before one it requires or follows is refused naming both; so is an unknown or repeated id or a set that cannot be ordered",
        r#"{"ids":"[string]"}"#,
        r#"{"order":"[string]","loaded":"[string]","left_out":"[{package,why}]"}"#,
        r#"{"ids":["kit-test","pogo"]}"#,
    ),
    def(
        m::PACKAGE_CHECK,
        "What `h2 package check` says of one package (`id`, else the edit target): a missing or stale lock, a cache map, .h2tag bundle, disc file, voice dataset or model weights folder in its folder, an item with no builder, a missing or oversize source, a source its builder refuses, a requirement that is missing or off, a tag path another package adds too, and a file that is a raw resource or a tag file of the tag store (`store`)",
        r#"{"id?":"string"}"#,
        r#"{"id":"string","ok":"boolean","count":"integer","findings":"[{kind: lock|files|store|item|set|conflict, message, item?, file?, line?}]"}"#,
        r#"{"id":"kit-test"}"#,
    ),
    def(
        m::PACKAGE_BUILD,
        "Build what a package (`id`, else the edit target) and the packages it requires give a map of the open source, or every map it has an item for; nothing is written unless `out` (a folder) is given. A package that is off is refused",
        r#"{"id?":"string","map?":"string","out?":"string"}"#,
        r#"{"id":"string","maps":"[{map,scope,touched,took_ms,tags,added,changed,overrides,bytes,problems}]"}"#,
        r#"{"id":"kit-test"}"#,
    ),
    def(
        m::PACKAGE_PLAY,
        "Build the package for a map of the open source, then have the paired game load that map. Refused when not paired, when the build fails or when the paired game read another package set at boot (restart it)",
        r#"{"id":"string","map":"string"}"#,
        r#"{"id":"string","map":"string","build":"object","game":"any"}"#,
        r#"{"id":"kit-test","map":"example"}"#,
    ),
    def(
        m::PROBLEMS_LIST,
        "The Problems panel's list, read again: every source or one. `check` is check.run's models and sounds, `ui` is ui.check, `hud` is hud.check and the HUD kit's, `package` is the manifest checks over the packages under <data>/packages/, `build` is what the game's package set refuses to load; the open script's diagnostics are `check` rows of area `scripts`, with their `line` and `col`. `asset` (an asset id) keeps the rows about that asset, as the count on the header's Check is. For a package item it refreshes package checks and open-script diagnostics. Worst first; each row names what opens it (`asset`: a tag, a package or an item; `field`; or the `tool` that holds a screen or a HUD). A check that cannot run is under `skipped`",
        r#"{"source?":"check|ui|hud|package|build","asset?":"string"}"#,
        r#"{"problems":"[{source,area,code,severity,message,target,path?,asset?,field?,tool?,line?,col?}]","counts":"{error,warning,note,total,by_source:{source:integer}}","ran":"[{source,check,findings}]","skipped":"[{source,check,why}]"}"#,
        r#"{"source":"package"}"#,
    ),
    def(
        m::WORKSPACE_STATUS,
        "The tag workspace: the tree (root, maps, lossy counts), the plan's stale parts, an extraction in progress, the last one's outcome, the sizes of everything it holds, and whether the start page shows",
        "{}",
        r#"{"root":"path","has_tree":"boolean","tree":"{maps,tags,lossy,refused,variants,extractor,format}|null","disc":"path|null","stale":"{reset,maps:[{map,why,groups}]}|null","extracting":"{disc,root,maps_total,maps_done,last}|null","last":"object|{error}|null","sizes":"{tree,layers,projects,outputs,total}","start_page":"boolean"}"#,
        "null",
    ),
    def(
        m::WORKSPACE_EXTRACT,
        "Run the plan against the disc (an image, a .map or a folder of maps; the recorded one, H2_DISC or disc/halo2.iso when not given) and extract every map, or only `maps` of it; `root` sets the tree's root (refused inside a Git checkout). Progress in workspace.status",
        r#"{"disc?":"string","maps?":"[string]","verify?":"boolean","root?":"string"}"#,
        r#"{"disc":"path","root":"path","maps":"[string]","verify":"boolean"}"#,
        r#"{"disc":"$DISC"}"#,
    ),
    def(
        m::WORKSPACE_PRUNE,
        "Remove what the workspace no longer needs: raw resources no tag file or manifest names, generated layers no project asks for and build caches unused for a week; `dry_run` reports without removing",
        r#"{"dry_run?":"boolean"}"#,
        r#"{"dry_run":"boolean","raw_removed":"integer","raw":"[hash]","generated_removed":"[string]","generated_kept":"[string]","caches_removed":"[{name,unused_days}]","outputs_removed":"[string]","bytes":"integer"}"#,
        r#"{"dry_run": true}"#,
    ),
    def(
        m::DOC_LIST,
        "Every document: each tool's, then each open tag tab's (its own undo stack); open, its name, dirty, undo and redo",
        "{}",
        r#"[{"doc":"map|variant|playlists|ui|... or a tag tab's group:path","open":"boolean","name":"string|null","dirty":"boolean","can_undo":"boolean","can_redo":"boolean","path?":"string"}]"#,
        "null",
    ),
    def(
        m::DOC_OPEN,
        "Open a document: a map of the source (or a .map path), a package, a variant (built-in, saved or new) or a playlist file (a path, default or blank). Refused over unsaved edits unless discard",
        r#"{"map?":"string","path?":"string","package?":"string","variant?":{"builtin?":"integer","saved?":"integer","new?":"string (slayer, ctf, ...)","script?":"string (a starter's name from rules.get, or open)"},"playlists?":"string (a path, default or blank)","discard?":"boolean"}"#,
        r#"{"doc":"string","name":"string|null","log":"[string]"}"#,
        r#"{"map":"example"}"#,
    ),
    def(
        m::DOC_SAVE,
        "Save to the document's destination; for a source map the edits are lines in the edit target, and with none set the call answers NO_TARGET",
        r#"{"doc?":"map|variant|playlists (default: the tool shown)"}"#,
        r#"{"saved":"boolean","path?":"string","bytes?":"integer","edit_target?":"string","log":"[string]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::DOC_SAVE_AS,
        "Save to a path: a map as a patched .map (never over its source, never into a Git checkout), a playlist file, a variant as a portable .h2variant",
        r#"{"path":"string","doc?":"map|variant|playlists (default: the tool shown)"}"#,
        r#"{"saved":"boolean","path":"string","bytes?":"integer","log":"[string]"}"#,
        r#"{"doc":"map","path":"$OUT/edited.map"}"#,
    ),
    def(
        m::DOC_DIFF,
        "What a save would write, against the document as opened: a map's patched byte runs with the fields they hold, a playlist's changed playlists, a variant's settings off their defaults",
        r#"{"doc?":"map|variant|playlists (default: the tool shown)"}"#,
        r#"{"doc":"string","dirty":"boolean","patches?":"[{address,len,block?,index?,fields?}]","grown?":"[...]","changed?":"[...]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::DOC_UNDO,
        "Undo the document's last edit; a tag tab's id undoes in that tag's own stack",
        r#"{"doc?":"map|variant|playlists|... or a tag tab's id (default: the tool shown)"}"#,
        r#"{"done":"boolean","can_undo":"boolean","can_redo":"boolean","log":"[string]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::DOC_REDO,
        "Redo the document's last undone edit; a tag tab's id redoes in that tag's own stack",
        r#"{"doc?":"map|variant|playlists|... or a tag tab's id (default: the tool shown)"}"#,
        r#"{"done":"boolean","can_undo":"boolean","can_redo":"boolean","log":"[string]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::DOC_CLOSE,
        "Close a document. Refused over unsaved edits unless discard",
        r#"{"doc":"map|variant|playlists","discard?":"boolean"}"#,
        r#"{"closed":"boolean"}"#,
        r#"{"doc":"playlists","discard":true}"#,
    ),
    def(
        m::SHELL_TABS,
        "The document tabs in order and the one in front: each tab's id (what doc.* takes), kind, title, the tool it shows in, its editor for a tag, dirty, and its own undo and redo",
        "{}",
        r#"{"front":"string|null","tool":"string","tabs":"[{doc,kind,title,tool,dirty,can_undo,can_redo,front,editor?,id?,group?,map?}]"}"#,
        "null",
    ),
    def(
        m::SHELL_OPEN,
        "Open a tag of the open map as a tab, in the editor its group gets (the scenario tag is the map's own tab), and bring it to the front. With map, that map opens first (refused over unsaved edits unless discard); a map alone brings its tab forward. `asset` takes an asset id: `voice:<id>` opens the voice document (tab `voice:<id>`, on Setup while the voice has no dataset, else Lines), `tag:<group>:<path>` the tag as the Browser does",
        r#"{"asset?":"string (voice:<id> or tag:<group>:<path>)","tag?":"string|integer (an id, group:path, path.group or a group with one tag)","map?":"string","discard?":"boolean"}"#,
        r#"{"tab":"{doc,kind,title,tool,dirty,can_undo,can_redo,front,...}","front":"string|null","log":"[string]"}"#,
        r#"{"tag":"itmc:objects\\test\\kit"}"#,
    ),
    def(
        m::SHELL_FOCUS,
        "Bring a tab to the front: its document shows in its editor, and Undo takes its own stack",
        r#"{"doc":"string (a tab id from shell.tabs)"}"#,
        r#"{"tab":"{doc,...}","front":"string|null","log":"[string]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::SHELL_CLOSE,
        "Close a tab. A tool's document closes (refused over unsaved edits unless discard); a tag's tab closes and its edits stay the map's. The tab to its left comes to the front",
        r#"{"doc":"string (a tab id from shell.tabs)","discard?":"boolean"}"#,
        r#"{"closed":"boolean","front":"string|null","log":"[string]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::TAG_LIST,
        "The open map's tags, by group and a name substring",
        r#"{"group?":"string (vehi, scnr, ...)","name?":"string","limit?":"integer (default 200)"}"#,
        r#"{"total":"integer","tags":"[{id,group,name}]"}"#,
        r#"{"group":"scnr"}"#,
    ),
    def(
        m::TAG_READ,
        "A tag as structured JSON: its blocks, references and string IDs from the layout schema, named fields where a registry knows them",
        r#"{"tag":"string (0x... id, or name[.group])","depth?":"integer (default 1)","elements?":"integer (per block, default 8)","hex?":"boolean"}"#,
        r#"{"id":"string","group":"string","name":"string|null","size":"integer","fields":"[field]"}"#,
        r#"{"tag":"scnr"}"#,
    ),
    def(
        m::TAG_FOLLOW,
        "Follow one reference (a tag.refs path, or its position in that list) to the tag it names",
        r#"{"tag":"string","ref":"string|integer"}"#,
        r#"{"id":"string","group":"string","name":"string|null","size":"integer"}"#,
        r#"{"tag":"scnr","ref":0}"#,
    ),
    def(
        m::TAG_SET,
        "Set a field of a tag. The scenario's placement fields (path <block>[<i>].<field>) work now; other tags take the studio-tags registry",
        r#"{"tag":"string","path":"string","value":"any"}"#,
        r#"{"value":"any"}"#,
        r#"{"tag":"scnr","path":"netgame_flags[0].position","value":[1,2,3]}"#,
    ),
    def(
        m::TAG_BLOCK_ADD,
        "Add an element to a tag block. The scenario's placement blocks work now (refused where they can't grow safely); other tags take the studio-tags registry",
        r#"{"tag":"string","path":"string (a block)","like?":"integer"}"#,
        r#"{"index":"integer"}"#,
        r#"{"tag":"scnr","path":"starting_locations","like":0}"#,
    ),
    def(
        m::TAG_BLOCK_REMOVE,
        "Remove an element of a tag block (the last element takes its slot), where removal is safe",
        r#"{"tag":"string","path":"string (a block)","index":"integer"}"#,
        r#"{"removed":"boolean"}"#,
        r#"{"tag":"scnr","path":"netgame_flags","index":0}"#,
    ),
    def(
        m::TAG_BLOCK_DUPLICATE,
        "Duplicate an element of a tag block",
        r#"{"tag":"string","path":"string (a block)","index":"integer"}"#,
        r#"{"index":"integer"}"#,
        r#"{"tag":"scnr","path":"starting_locations","index":0}"#,
    ),
    def(
        m::SCENARIO_BLOCKS,
        "The placement blocks: counts, room before growing, fields, and why adding or removing is refused",
        "{}",
        r#"[{"block":"string","label":"string","count":"integer","capacity":"integer","stride":"integer","fields":"[{name,type,role,editable}]","add_refusal":"string|null","remove_refusal":"string|null"}]"#,
        "null",
    ),
    def(
        m::SCENARIO_LIST,
        "The placements of one block, or of every block",
        r#"{"block?":"string"}"#,
        r#"[{"block":"string","index":"integer","label":"string","position?":"[x,y,z]"}]"#,
        r#"{"block":"netgame_flags"}"#,
    ),
    def(
        m::SCENARIO_GET,
        "One placement: its label, object tag and every field",
        r#"{"block":"string","index":"integer"}"#,
        r#"{"block":"string","index":"integer","label":"string","tag?":"{id,group,name}","fields":"{name: value}"}"#,
        r#"{"block":"netgame_flags","index":0}"#,
    ),
    def(
        m::SCENARIO_SET,
        "Set one field of a placement, checked first (read-only fields, ranges, palette indices, tag groups). One undo step",
        r#"{"block":"string","index":"integer","field":"string","value":"any (number, [x,y,z], {id} or null for a tag)"}"#,
        r#"{"value":"any"}"#,
        r#"{"block":"netgame_flags","index":0,"field":"position","value":[4,5,6]}"#,
    ),
    def(
        m::SCENARIO_ADD,
        "Add a placement: a copy of `like`, or a new one with the palette's first item, at `position` (default: the camera focus). Refused where the block can't grow safely. `block: \"label\"` places a ground label (ff_drop, ff_base, ff_cover, siege_site, siege_goal or siege_line, as `label`) as one netgame flag with its label, as the Scene tab's AI and objectives group does; `facing` is radians, `team` 0 or 1 (a base and a siege line), `phase` a siege line's",
        r#"{"block":"string","like?":"integer","position?":"[x,y,z]","label?":"string","facing?":"number","team?":"integer","phase?":"integer"}"#,
        r#"{"index":"integer"}"#,
        r#"{"block":"starting_locations","like":0,"position":[1,1,1]}"#,
    ),
    def(
        m::SCENARIO_MOVE,
        "Move a placement, and turn it given yaw, pitch and roll in radians (a single-facing block takes the yaw). One undo step",
        r#"{"block":"string","index":"integer","position":"[x,y,z]","ypr?":"[yaw,pitch,roll]"}"#,
        r#"{"position":"[x,y,z]"}"#,
        r#"{"block":"netgame_flags","index":0,"position":[7,8,9]}"#,
    ),
    def(
        m::SCENARIO_DELETE,
        "Delete a placement (the block's last takes its slot). Refused where something refers to entries by index",
        r#"{"block":"string","index":"integer"}"#,
        r#"{"removed":"boolean"}"#,
        r#"{"block":"starting_locations","index":1}"#,
    ),
    def(
        m::SCENARIO_DUPLICATE,
        "Duplicate a placement",
        r#"{"block":"string","index":"integer"}"#,
        r#"{"index":"integer"}"#,
        r#"{"block":"starting_locations","index":0}"#,
    ),
    def(
        m::SCENARIO_SELECT,
        "Select a placement (and frame it with focus), select a tag as the inspector's root, or clear the selection",
        r#"{"block?":"string","index?":"integer","focus?":"boolean","tag?":"string|integer"}"#,
        r#"{"selected":"{block,index}|{tag}|null"}"#,
        r#"{"block":"netgame_flags","index":0}"#,
    ),
    def(
        m::VOLUME_LIST,
        "The open map's trigger volumes as oriented boxes (corner, forward, up, full size), what each is drawn as, and the kill triggers and BSP switches that name it",
        r#"{"kind?":"trigger|kill|bsp_switch"}"#,
        r#"{"volumes":"[{index,name,label,kinds,origin,forward,up,size,center,parent,world,named_by:[{kind,index,from?,to?}]}]","kill_triggers":"integer","bsp_switches":"integer","selected":"integer|null","error":"string|null"}"#,
        r#"{"kind":"kill"}"#,
    ),
    def(
        m::VOLUME_SELECT,
        "Select a trigger volume by index (clearing the placements) and frame it with focus, or clear the selection",
        r#"{"index?":"integer","focus?":"boolean"}"#,
        r#"{"selected":"integer|null"}"#,
        r#"{"index":0,"focus":true}"#,
    ),
    def(
        m::VOLUME_SET,
        "Give a trigger volume a new box as one undo step: forward, up and size over its own, then the corner origin or the centre (its own when neither is given). Forward and up are squared",
        r#"{"index":"integer","origin?":"[x,y,z]","center?":"[x,y,z]","forward?":"[x,y,z]","up?":"[x,y,z]","size?":"[x,y,z]"}"#,
        r#"{"volume":"{index,name,label,kinds,origin,forward,up,size,center,parent,world,named_by}"}"#,
        r#"{"index":0,"center":[102,102,3],"size":[4,4,6]}"#,
    ),
    def(
        m::VOLUME_ADD,
        "Add a trigger volume (a 2-unit cube at the camera focus unless the box is given) and select it; kill adds a kill trigger naming it in the same undo step (Make a kill volume). Refused past the block's source size while growth is off",
        r#"{"kill?":"boolean","center?":"[x,y,z]","origin?":"[x,y,z]","forward?":"[x,y,z]","up?":"[x,y,z]","size?":"[x,y,z]"}"#,
        r#"{"index":"integer","kill_trigger":"integer|null","volume":"{index,...}"}"#,
        r#"{"kill":true,"center":[0,0,2]}"#,
    ),
    def(
        m::VOLUME_REMOVE,
        "Remove a trigger volume or a kill trigger (the last entry takes its slot; kill triggers and BSP switches follow a moved volume). A volume a kill trigger or BSP switch names is refused, naming them, as is a source volume of a scripted map",
        r#"{"kind?":"trigger|kill","index":"integer"}"#,
        r#"{"removed":"{kind,index}","triggers":"integer","kill_triggers":"integer","selected":"integer|null"}"#,
        r#"{"kind":"kill","index":0}"#,
    ),
    def(
        m::COLLISION_SURFACES,
        "The open map's level collision surfaces (each structure BSP's collision BSP): plane, raw flags, material, ring, and class, collision_only when no drawn triangle lies on the surface's plane over it (the invisible walls). Filter by class or BSP; offset and limit (200) page the list",
        r#"{"class?":"drawn|collision_only","bsp?":"integer","offset?":"integer","limit?":"integer"}"#,
        r#"{"bsps":"[string]","counts":"{drawn,collision_only}","matching":"integer","surfaces":"[{index,bsp,surface,class,plane,flags,breakable,breakable_surface,material,covered,area,center,ring}]","selected":"integer|null","errors":"[{bsp,error}]"}"#,
        r#"{"class":"collision_only","limit":10}"#,
    ),
    def(
        m::COLLISION_AT,
        "The level collision surface a ray meets first (origin and dir) or the one a point lies on (within 0.05 units of its plane, over it), and its class, whatever layers are shown; select selects it (or clears the selection on a miss)",
        r#"{"origin?":"[x,y,z]","dir?":"[x,y,z]","point?":"[x,y,z]","within?":"number","select?":"boolean"}"#,
        r#"{"surface":"{index,bsp,surface,class,...}|null","distance":"number|null","selected":"integer|null"}"#,
        r#"{"origin":[205,205,1],"dir":[1,0,0]}"#,
    ),
    def(
        m::COLLISION_SELECT,
        "Select a level collision surface by index (clearing the placements and the volume) and frame it with focus, or clear the selection. Read only",
        r#"{"index?":"integer","focus?":"boolean"}"#,
        r#"{"selected":"integer|null"}"#,
        r#"{"index":0,"focus":true}"#,
    ),
    def(
        m::COLLISION_OBJECTS,
        "The placed objects' collision: each object tag's model, coll and phmo, and every shape a placement places (coll BSPs, phmo spheres, pills, boxes and polyhedra), posed by its node's rest transform and the placement's, in world space. Filter by block and index",
        r#"{"block?":"string","index?":"integer"}"#,
        r#"{"models":"[{object,resolved,model,coll,phmo,nodes,shapes,errors}]","shapes":"[{block,index,object,kind,tag,shape,node,center,radius,edges}]","error":"string|null"}"#,
        r#"{"block":"scenery","index":0}"#,
    ),
    def(
        m::VARIANT_LIST,
        "The built-in and saved game variants, and the one open",
        "{}",
        r#"{"builtins":"[{index,name,type}]","saved":"[{index,name,dir}]","open":"{name,origin}|null"}"#,
        "null",
    ),
    def(
        m::VARIANT_GET,
        "The open variant: name, type, origin, every setting with its value and label, its issues, and the script it names (when the editor has it)",
        "{}",
        r#"{"name":"string","type":"string","origin":"string","read_only":"boolean","dirty":"boolean","settings":"[{id,title,value,label}]","issues":"[string]","script":"{name,about,scoring_carrier,options:[{index,name,label,description,kind,value,text,default,quick}],quick:[{label,option,text}]}|{prefix,found:false}|null"}"#,
        "null",
    ),
    def(
        m::VARIANT_SET,
        "Set a setting of the open variant, or one option of its script by name (built-ins are read-only: copy first)",
        r#"{"setting?":"integer","option?":"string","value":"integer"}"#,
        r#"{"setting?":"integer","option?":"string","value":"integer","label":"string"}"#,
        r#"{"setting":"$SETTING","value":"$VALUE"}"#,
    ),
    def(
        m::VARIANT_RENAME,
        "Rename the open variant",
        r#"{"name":"string"}"#,
        r#"{"name":"string"}"#,
        r#"{"name":"Agent Slayer"}"#,
    ),
    def(
        m::VARIANT_COPY,
        "Copy the open built-in under a name, as a new unsaved variant",
        r#"{"name":"string"}"#,
        r#"{"name":"string"}"#,
        r#"{"name":"My Slayer"}"#,
    ),
    def(
        m::VARIANT_EXPORT,
        "Write the open variant as a portable .h2variant file; with maps (variant ids from variant.maps, at most 8) a version-2 file that suggests them",
        r#"{"path":"string","maps?":["string"]}"#,
        r#"{"path":"string","maps":"number"}"#,
        r#"{"path":"$OUT/v.h2variant"}"#,
    ),
    def(
        m::VARIANT_MAPS,
        "List this machine's map variants a game variant may suggest (any map, saved then fetched)",
        r#"{}"#,
        r#"{"maps":[{"map":"string","name":"string","base":"sha256","variant":"sha256","source":"saved|live"}]}"#,
        r#"{}"#,
    ),
    def(
        m::VARIANT_IMPORT,
        "Open a portable .h2variant (a file, or its text) as a new unsaved variant",
        r#"{"path?":"string","text?":"string","discard?":"boolean"}"#,
        r#"{"name":"string"}"#,
        r#"{"path":"$OUT/v.h2variant","discard":true}"#,
    ),
    def(
        m::VARIANT_DELETE,
        "Delete the open saved variant",
        "{}",
        r#"{"deleted":"boolean"}"#,
        "null",
    ),
    def(
        m::PLAYLIST_LIST,
        "The open playlist file's 16 playlists, its path and its errors",
        "{}",
        r#"{"path":"string|null","dirty":"boolean","playlists":"[{playlist,title,type,entries}]","errors":"[string]"}"#,
        "null",
    ),
    def(
        m::PLAYLIST_GET,
        "One playlist (0..15): its text and entries",
        r#"{"playlist":"integer"}"#,
        r#"{"playlist":"integer","title":"string","description":"string","entries":"[{entry,map,weight,chance,variant}]"}"#,
        r#"{"playlist":0}"#,
    ),
    def(
        m::PLAYLIST_SET,
        "Set a playlist's title or description. One undo step each",
        r#"{"playlist":"integer","title?":"string","description?":"string"}"#,
        r#"{"playlist":"integer"}"#,
        r#"{"playlist":0,"title":"Agent Playlist"}"#,
    ),
    def(
        m::PLAYLIST_ENTRY,
        "Edit a playlist's entries: add, duplicate, remove, move (to) or weight (1..1000). One undo step",
        r#"{"playlist":"integer","op":"add|duplicate|remove|move|weight","entry?":"integer","to?":"integer","weight?":"integer"}"#,
        r#"{"entries":"integer"}"#,
        r#"{"playlist":0,"op":"weight","entry":0,"weight":7}"#,
    ),
    def(
        m::PLAYLIST_ENTRY_VARIANT,
        "Give an entry the variant open in the variant tool",
        r#"{"playlist":"integer","entry":"integer"}"#,
        r#"{"variant":"string"}"#,
        r#"{"playlist":0,"entry":0}"#,
    ),
    def(
        m::UI_PALETTE,
        "Search the Go to anything palette, or run a shown entry by its zero-based index",
        r#"{"query":"string","run?":"integer"}"#,
        r#"{"entries":"[{kind,label,detail}]","totals":"{map,tag,placement,command}","ran?":"integer"}"#,
        r#"{"query":"blaster"}"#,
    ),
    def(
        m::UI_SCREENS,
        "Our UI screens and their states (and the disc's retail screens when a disc is open)",
        "{}",
        r#"{"screens":"[{screen,states}]","retail":"[string]","pick":"{screen,state}|{retail}|null","folders":"{ours,retail,browse}"}"#,
        "null",
    ),
    def(
        m::UI_PICK,
        "Show a screen in a state (default: its first), or a retail screen by tag (with a named sample variant)",
        r#"{"screen?":"string","state?":"string","retail?":"string","sample?":"string"}"#,
        r#"{"pick":"any","summary":"string|null"}"#,
        r#"{"screen":"$SCREEN"}"#,
    ),
    def(
        m::UI_PRESS,
        "A pad press on the screen shown (up down left right a b x y start back ...)",
        r#"{"input":"string"}"#,
        r#"{"line":"string|null"}"#,
        r#"{"input":"down"}"#,
    ),
    def(
        m::UI_WIDGETS,
        "The screen's widgets with their rects (640 by 480, top left origin) and the layout checks' findings",
        "{}",
        r#"{"widgets":"[{id,rect,...}]","findings":"[{text,ids}]"}"#,
        "null",
    ),
    def(
        m::UI_RENDER,
        "Render the screen shown to a PNG (default: the cache directory's studio/ui/; never into a Git checkout)",
        r#"{"out?":"string"}"#,
        r#"{"path":"string","width":"integer","height":"integer","findings":"integer"}"#,
        r#"{"out":"$OUT/screen.png"}"#,
    ),
    def(
        m::UI_CONSISTENCY,
        "Check our screens (or one) in every state against the design system: palette, fonts by role, title and legend places, legend format, capitals, row pitch, fit",
        r#"{"screen?":"string"}"#,
        r#"{"findings":"[{screen,state,ids,rule,text}]","checked":"integer"}"#,
        r#"{"screen":"$SCREEN"}"#,
    ),
    def(
        m::UI_COMPONENTS,
        "The design system's catalogue: each component with its role, font, colour, variants and specimen page; each layout page; the palette and fonts",
        "{}",
        r#"{"components":"[{name,role,font,colour,justify,variants,specimen}]","layouts":"[{name,kind,states}]","palette":"[{name,rgb}]","fonts":"{role:integer}"}"#,
        "null",
    ),
    def(
        m::UI_SELECT_FOLDER,
        "Show the grid of previews of a root or folder in the UI Screens tool (folder: ours/<name> or retail/<path>, as ui.screens lists under folders)",
        r#"{"folder?":"string","root?":"ours|retail"}"#,
        r#"{"browse":"{root,folder,path}","tiles":"integer","states":"boolean"}"#,
        r#"{"root":"ours"}"#,
    ),
    def(
        m::UI_SHEET,
        "Render a folder's or root's previews to one labelled contact-sheet PNG (the grid's tiles; default: the grid shown; never into a Git checkout)",
        r#"{"folder?":"string","root?":"ours|retail","states?":"boolean","columns?":"integer","out?":"string"}"#,
        r#"{"path":"string","width":"integer","height":"integer","tiles":"[{pick,name,x,y,w,h,findings,checked}]"}"#,
        r#"{"root":"ours","out":"$OUT/sheet.png"}"#,
    ),
    def(
        m::VIEW_CAMERA,
        "Read or set the viewport camera (degrees and world units), set fly speed, or frame a placement",
        r#"{"yaw?":"number","pitch?":"number","distance?":"number","target?":"[x,y,z]","focus?":{"block":"string","index":"integer"},"all?":"boolean","fly_speed?":"number (0.5..1000 units/s)"}"#,
        r#"{"yaw":"number","pitch":"number","distance":"number","target":"[x,y,z]","fly_speed":"number"}"#,
        r#"{"yaw":45,"pitch":30,"distance":40}"#,
    ),
    def(
        m::VIEW_LAYERS,
        "Read the viewport legend, or apply a preset, layer switches and X-ray to the window and snapshots. diff draws the playtest layers as the difference between the shown run and another (true: its compare_to, else the baseline; or a run id; false: the result itself); refused when there is nothing to compare, and the sightlines have no difference layer",
        r#"{"set?":"{layer name: boolean}","preset?":"art|gameplay|collision|navigation|playtest|everything","xray?":"boolean","diff?":"boolean|run id|baseline"}"#,
        r#"{"layers":"[{name,label,group,on,count,color,refused?}]","xray":"boolean","diff":"{a,b,game_type}|null"}"#,
        r#"{"preset":"everything","set":{"fog":false},"xray":true}"#,
    ),
    def(
        m::VIEW_LIGHTING,
        "Read the viewport's Lighting mode, or set it, for the window and snapshots: game (base map times lightmap), flat (no lightmap), lightmap (the lightmap on white), collision (flat at a third)",
        r#"{"mode?":"game|flat|lightmap|collision"}"#,
        r#"{"mode":"string","modes":"[string]"}"#,
        r#"{"mode":"flat"}"#,
    ),
    def(
        m::VIEW_SNAPSHOT,
        "Render the open map from a camera (default: the viewport's) offscreen to a PNG (default: the cache directory's studio/view/; never into a Git checkout)",
        r#"{"out?":"string","size?":"[w,h] (default [960,540])","camera?":{"yaw?":"number","pitch?":"number","distance?":"number","target?":"[x,y,z]"}}"#,
        r#"{"path":"string","width":"integer","height":"integer"}"#,
        r#"{"out":"$OUT/view.png","size":[64,48]}"#,
    ),
    def(
        m::VIEW_STATS,
        "What the viewport draws for the open map: lightmap pages and each BSP's lighting, the world's triangles by blend, every material (template, blend, picture), the sky's sun and fog",
        "{}",
        r#"{"scene":"boolean","lightmap_pages":"integer","bsps":"[{bsp,lighting,detail}]","triangles":"{blend: integer}","solid_triangles":"integer","lightmapped_triangles":"integer","materials":"[{shader,template,blend,blend_from_passes,emissive,base,details,mask}]","sky":"string|null","atmosphere":"{sky,sun,fog,fallback_rgb}"}"#,
        "null",
    ),
    // Planned namespaces: designed seams with no provider yet. `model.list`,
    // `read`, `render` and `check` are the models provider's (api/models.rs).
    planned(
        m::MODEL_COLORS,
        "A player's primary, secondary, detail and emblem colours",
        r#"{"player?":"integer","colors?":"any"}"#,
        "player models (not started)",
    ),
    planned(
        m::MODEL_EMBLEM,
        "A player's emblem: foreground, background, colours",
        r#"{"player?":"integer","emblem?":"any"}"#,
        "player models (not started)",
    ),
    planned(
        m::CUSTOM_LIST,
        "Customisation choices: armour permutations per region and what unlocks them",
        r#"{"biped?":"string"}"#,
        "customisation (later)",
    ),
    planned(
        m::CUSTOM_SET,
        "Pick an armour permutation for a region",
        r#"{"biped":"string","region":"string","permutation":"string"}"#,
        "customisation (later)",
    ),
    planned(
        m::WORKSPACE_IMPORT,
        "Store a map as an import layer, so only what differs from the tree is held",
        r#"{"map?":"string (a .map file)","path?":"string (a folder of them)","name":"string"}"#,
        "imports as layers (W9)",
    ),
];

/// The parsed shape: field name, required.
fn shape_keys(shape: &str) -> Vec<(String, bool)> {
    match serde_json::from_str::<Value>(shape) {
        Ok(Value::Object(o)) => o
            .keys()
            .map(|k| match k.strip_suffix('?') {
                Some(k) => (k.to_owned(), false),
                None => (k.clone(), true),
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// An error with a code from [`studio_proto::code`].
pub fn err(code: i64, message: impl Into<String>) -> RpcError {
    RpcError::new(code, message)
}

/// A call's outcome.
pub type Reply = Result<Value, RpcError>;

/// The API's state over an [`App`]: its providers and a GPU for renders.
pub struct Api {
    /// Answer ahead of the built-in methods (the seams' lanes).
    providers: Vec<Box<dyn Provider>>,
    /// The offscreen renderer, made on the first render.
    gpu: Option<Result<view::Gpu, String>>,
    /// Whether a window shows the edits (`session.status`).
    pub window: bool,
}

impl Default for Api {
    fn default() -> Self {
        Self::new()
    }
}

impl Api {
    /// The API with every lane's provider registered ([`seams::register`]).
    pub fn new() -> Self {
        let mut api = Api {
            providers: Vec::new(),
            gpu: None,
            window: false,
        };
        seams::register(&mut api);
        api
    }

    /// Adds a provider: it answers its methods ahead of the built-in ones
    /// (and of the planned stubs), and `session.methods` lists its
    /// definitions in place of theirs.
    pub fn register(&mut self, p: Box<dyn Provider>) {
        self.providers.push(p);
    }

    /// Every method: the table, with providers' definitions over it.
    pub fn methods(&self) -> Vec<MethodDef> {
        let mut out: Vec<MethodDef> = METHODS.to_vec();
        for p in &self.providers {
            for d in p.methods() {
                match out.iter_mut().find(|x| x.name == d.name) {
                    Some(x) => *x = d,
                    None => out.push(d),
                }
            }
        }
        out
    }

    /// Answers one request; `None` for a notification (no id).
    pub fn handle(&mut self, app: &mut App, req: Request) -> Option<Response> {
        let id = req.id.clone();
        let r = self.call(app, &req.method, req.params);
        let id = id?;
        Some(match r {
            Ok(v) => Response::ok(id, v),
            Err(e) => Response::err(id, e),
        })
    }

    /// Runs one call: checks its params against the method's shape, then
    /// asks the providers, then the built-in methods.
    pub fn call(&mut self, app: &mut App, method: &str, params: Value) -> Reply {
        let _scope = mem_watch::scope("agent.request");
        let methods = self.methods();
        let Some(d) = methods.iter().find(|d| d.name == method) else {
            return Err(err(
                code::METHOD_NOT_FOUND,
                format!("no method {method} (session.methods lists them)"),
            ));
        };
        let params = match params {
            Value::Null => Value::Object(Default::default()),
            Value::Object(o) => Value::Object(o),
            other => {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("{method}: params are an object, not {other}"),
                ));
            }
        };
        // A planned method (no provider has defined it) is not built
        // whatever its params: say so first.
        if let Some(lane) = d.planned {
            let mut e = err(
                code::NOT_YET,
                format!("{method} is planned, not built yet: {lane} builds it"),
            );
            e.data = Some(json!({ "lane": lane, "method": method }));
            return Err(e);
        }
        check_params(d, &params)?;
        if agent_proto::memory_serve::supports(method) {
            return agent_proto::memory_serve::call(method, params);
        }
        for p in &mut self.providers {
            if let Some(r) = p.call(app, method, &params) {
                return r;
            }
        }
        let p = P(&params);
        match method.split('.').next().unwrap_or("") {
            "session" => self.session(app, method, &p),
            "source" | "doc" => docs::call(app, method, &p),
            "shell" => shell::call(app, method, &p),
            "asset" => assets::call(app, method, &p),
            "package" => packages::call(app, method, &p),
            "problems" => problems::call(app, method, &p),
            "tag" => tags::call(app, method, &p),
            "scenario" => scenario::call(app, method, &p),
            "volume" => scenario::volume(app, method, &p),
            "collision" => collision::call(app, method, &p),
            "variant" => variants::call(app, method, &p),
            "playlist" => playlists::call(app, method, &p),
            "ui" if matches!(method, m::UI_CONSISTENCY | m::UI_COMPONENTS) => {
                ui_kit::call(app, method, &p)
            }
            "ui" => ui_screens::call(app, method, &p),
            "workspace" => workspace::call(app, method, &p),
            "view" => self.view(app, method, &p),
            _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
        }
    }

    fn session(&mut self, app: &mut App, method: &str, p: &P) -> Reply {
        match method {
            m::STATUS => Ok(json!({
                "protocol": studio_proto::PROTOCOL,
                "window": self.window,
                "tool": tool_name(app.tool),
                "source": app.source().map(|s| s.path().display().to_string()),
                "docs": docs::list(app),
            })),
            m::METHODS => Ok(Value::Array(self.methods().iter().map(describe).collect())),
            m::LOG => {
                let n = p.opt_u64("last")?.unwrap_or(20) as usize;
                let log = app.log();
                Ok(Value::Array(
                    log[log.len().saturating_sub(n)..]
                        .iter()
                        .map(|l| {
                            json!({
                                "at": l.at,
                                "error": l.status.is_error(),
                                "text": l.status.text(),
                            })
                        })
                        .collect(),
                ))
            }
            _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
        }
    }

    fn view(&mut self, app: &mut App, method: &str, p: &P) -> Reply {
        match method {
            m::VIEW_CAMERA => view::camera(app, p),
            m::VIEW_LAYERS => view::layers(app, p),
            m::VIEW_STATS => view::stats(app, p),
            m::VIEW_LIGHTING => view::lighting(app, p),
            m::VIEW_SNAPSHOT => {
                let gpu = self
                    .gpu
                    .get_or_insert_with(|| view::Gpu::new().map_err(|e| format!("{e:#}")));
                match gpu {
                    Ok(gpu) => view::snapshot(gpu, app, p),
                    Err(e) => Err(err(code::NO_GPU, format!("no renderer here: {e}"))),
                }
            }
            _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
        }
    }

    /// One turn of `h2-editor --serve`: waits up to `wait` for a request,
    /// answers every one queued, then runs the game link as the window does
    /// between frames, so `pair.*` and `mirror.*` work with no window.
    /// False once the socket's queue has closed.
    pub fn serve_once(
        &mut self,
        app: &mut App,
        play: &mut crate::play::Play,
        rx: &std::sync::mpsc::Receiver<server::Incoming>,
        wait: std::time::Duration,
    ) -> bool {
        use std::sync::mpsc::RecvTimeoutError;
        match rx.recv_timeout(wait) {
            Ok(inc) => {
                inc.answer(self.handle(app, inc.request.clone()));
                self.pump(app, rx);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
        play.update(app);
        app.poll_bot_nav();
        app.poll_ground();
        // The workspace's background extraction, as the window polls it
        // each frame.
        app.poll_workspace();
        // Outside editors' saves, as the window polls them each frame.
        app.poll_external(std::time::Instant::now());
        true
    }

    /// Runs the **Preview in game** ask the window's button left, as the
    /// `pair.preview` call it stands for: the job is `pair.result`'s, and
    /// a refusal (paused game, no pairing) goes to the log.
    pub fn run_preview_asks(&mut self, app: &mut App) {
        let Some(doc) = app.preview.ask.take() else {
            return;
        };
        let params = match app.game_preview(doc) {
            Ok(g) => g.params(),
            Err(why) => return app.error(why),
        };
        match self.call(app, m::PAIR_PREVIEW, params) {
            Ok(v) => {
                app.preview.last_job = v["id"].as_u64();
                app.info(format!(
                    "previewing {} in the game (pair.result {})",
                    app.doc_title(doc),
                    v["id"]
                ));
            }
            Err(e) => app.error(e.message),
        }
    }

    /// Answers every request waiting on `rx`; returns how many.
    pub fn pump(
        &mut self,
        app: &mut App,
        rx: &std::sync::mpsc::Receiver<server::Incoming>,
    ) -> usize {
        self.run_preview_asks(app);
        let mut n = 0;
        while let Ok(inc) = rx.try_recv() {
            inc.answer(self.handle(app, inc.request.clone()));
            n += 1;
        }
        n
    }
}

/// A method's `session.methods` entry.
pub fn describe(d: &MethodDef) -> Value {
    let parse = |s: &str| serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::from(s));
    let mut v = json!({
        "name": d.name,
        "summary": d.summary,
        "params": parse(d.params),
        "result": parse(d.result),
        "example": parse(d.example),
        "status": if d.planned.is_some() { "planned" } else { "ready" },
    });
    if let Some(lane) = d.planned {
        v["lane"] = json!(lane);
    }
    v
}

/// Refuses params the method's shape does not name, and missing required
/// ones.
fn check_params(d: &MethodDef, params: &Value) -> Result<(), RpcError> {
    let keys = shape_keys(d.params);
    let obj = params.as_object().expect("an object by now");
    let unknown: Vec<&String> = obj
        .keys()
        .filter(|k| !keys.iter().any(|(n, _)| n == *k))
        .collect();
    if !unknown.is_empty() {
        let names: Vec<_> = keys.iter().map(|(n, _)| n.as_str()).collect();
        return Err(err(
            code::INVALID_PARAMS,
            format!(
                "{}: unknown param {} (it takes: {})",
                d.name,
                unknown
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                if names.is_empty() {
                    "nothing".to_owned()
                } else {
                    names.join(", ")
                }
            ),
        ));
    }
    if let Some((missing, _)) = keys
        .iter()
        // Present counts as given: null is a value (a reference set to none).
        .find(|(k, req)| *req && !obj.contains_key(k.as_str()))
    {
        return Err(err(
            code::INVALID_PARAMS,
            format!("{}: {missing} is required", d.name),
        ));
    }
    Ok(())
}

pub fn tool_name(t: crate::ToolKind) -> &'static str {
    match t {
        crate::ToolKind::Scenario => "scenario",
        crate::ToolKind::Variants => "variants",
        crate::ToolKind::Playlists => "playlists",
        crate::ToolKind::UiScreens => "ui-screens",
        crate::ToolKind::Tags => "tags",
        crate::ToolKind::Hud => "hud",
        crate::ToolKind::Scripts => "scripts",
        crate::ToolKind::Textures => "textures",
    }
}
