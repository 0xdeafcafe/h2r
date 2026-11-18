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
//!   methods ahead of the built-in ones; `hud.*`, `model.*` and
//!   `custom.*` are planned namespaces answering NOT_YET until one does.
//!
//! [`Action`]: crate::app::Action
//! [`App::dispatch`]: crate::App::dispatch

use agent_proto::{Request, Response, RpcError};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use crate::App;

mod docs;
mod params;
mod playlists;
mod scenario;
pub mod seams;
pub mod server;
mod tag_fields;
mod tags;
mod ui_screens;
mod variants;
mod view;

#[cfg(test)]
mod tests;

pub use params::P;
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
        "The source's maps, the map loaded, and the packages under the data root",
        "{}",
        r#"{"source":"string|null","maps":"[string]","loaded":"string|null","packages":"[{key,label}]"}"#,
        "null",
    ),
    def(
        m::DOC_LIST,
        "Every document: open, its name, dirty, undo and redo",
        "{}",
        r#"[{"doc":"map|variant|playlists|ui","open":"boolean","name":"string|null","dirty":"boolean","can_undo":"boolean","can_redo":"boolean","path?":"string"}]"#,
        "null",
    ),
    def(
        m::DOC_OPEN,
        "Open a document: a map of the source (or a .map path), a package, a variant (built-in, saved or new) or a playlist file (a path, default or blank). Refused over unsaved edits unless discard",
        r#"{"map?":"string","path?":"string","package?":"string","variant?":{"builtin?":"integer","saved?":"integer","new?":"string (slayer, ctf, ...)"},"playlists?":"string (a path, default or blank)","discard?":"boolean"}"#,
        r#"{"doc":"string","name":"string|null","log":"[string]"}"#,
        r#"{"map":"example"}"#,
    ),
    def(
        m::DOC_SAVE,
        "Save in place: a variant to its save, a playlist file to its path, a map to the file it was last saved as",
        r#"{"doc?":"map|variant|playlists (default: the tool shown)"}"#,
        r#"{"saved":"boolean","path?":"string","log":"[string]"}"#,
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
        "Undo the document's last edit",
        r#"{"doc?":"map|variant|playlists (default: the tool shown)"}"#,
        r#"{"done":"boolean","can_undo":"boolean","can_redo":"boolean","log":"[string]"}"#,
        r#"{"doc":"map"}"#,
    ),
    def(
        m::DOC_REDO,
        "Redo the document's last undone edit",
        r#"{"doc?":"map|variant|playlists (default: the tool shown)"}"#,
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
        m::TAG_REFS,
        "Every tag reference in a tag (through its blocks), with where it sits and what it names",
        r#"{"tag":"string","limit?":"integer (default 500)"}"#,
        r#"[{"path":"string","group":"string","id":"string","name":"string|null"}]"#,
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
        "Add a placement: a copy of `like`, or a new one with the palette's first item, at `position` (default: the camera focus). Refused where the block can't grow safely",
        r#"{"block":"string","like?":"integer","position?":"[x,y,z]"}"#,
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
        "Select a placement in the window (and frame it with focus)",
        r#"{"block?":"string","index?":"integer","focus?":"boolean"}"#,
        r#"{"selected":"{block,index}|null"}"#,
        r#"{"block":"netgame_flags","index":0}"#,
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
        "The open variant: name, type, origin, every setting with its value and label, and its issues",
        "{}",
        r#"{"name":"string","type":"string","origin":"string","read_only":"boolean","dirty":"boolean","settings":"[{id,title,value,label}]","issues":"[string]"}"#,
        "null",
    ),
    def(
        m::VARIANT_SET,
        "Set a setting of the open variant (built-ins are read-only: copy first)",
        r#"{"setting":"integer","value":"integer"}"#,
        r#"{"setting":"integer","value":"integer","label":"string"}"#,
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
        "Write the open variant as a portable .h2variant file",
        r#"{"path":"string"}"#,
        r#"{"path":"string"}"#,
        r#"{"path":"$OUT/v.h2variant"}"#,
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
        m::UI_SCREENS,
        "Our UI screens and their states (and the disc's retail screens when a disc is open)",
        "{}",
        r#"{"screens":"[{screen,states}]","retail":"[string]","pick":"{screen,state}|{retail}|null"}"#,
        "null",
    ),
    def(
        m::UI_PICK,
        "Show a screen in a state (default: its first), or a retail screen by tag",
        r#"{"screen?":"string","state?":"string","retail?":"string"}"#,
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
        m::VIEW_CAMERA,
        "Read or set the viewport camera (degrees and world units), or frame a placement",
        r#"{"yaw?":"number","pitch?":"number","distance?":"number","target?":"[x,y,z]","focus?":{"block":"string","index":"integer"},"all?":"boolean"}"#,
        r#"{"yaw":"number","pitch":"number","distance":"number","target":"[x,y,z]"}"#,
        r#"{"yaw":45,"pitch":30,"distance":40}"#,
    ),
    def(
        m::VIEW_SNAPSHOT,
        "Render the open map from a camera (default: the viewport's) offscreen to a PNG (default: the cache directory's studio/view/; never into a Git checkout)",
        r#"{"out?":"string","size?":"[w,h] (default [960,540])","camera?":{"yaw?":"number","pitch?":"number","distance?":"number","target?":"[x,y,z]"}}"#,
        r#"{"path":"string","width":"integer","height":"integer"}"#,
        r#"{"out":"$OUT/view.png","size":[64,48]}"#,
    ),
    // Planned namespaces: designed seams with no provider yet.
    planned(
        m::HUD_LIST,
        "The open map's HUDs (nhdt) and who names them",
        r#"{}"#,
        "studio-hud",
    ),
    planned(
        m::HUD_READ,
        "One HUD's widgets with their placement, state gates and fields",
        r#"{"hud":"string"}"#,
        "studio-hud",
    ),
    planned(
        m::HUD_SET,
        "Set a HUD widget's field (offset, anchor, bitmap, colour)",
        r#"{"hud":"string","widget":"string","field":"string","value":"any"}"#,
        "studio-hud",
    ),
    planned(
        m::HUD_RENDER,
        "Render a HUD at a game state to a PNG",
        r#"{"hud":"string","state?":"any","out?":"string"}"#,
        "studio-hud",
    ),
    planned(
        m::MODEL_LIST,
        "Bipeds and their model variants (hlmt, mode permutations)",
        r#"{"group?":"string"}"#,
        "player models (not started)",
    ),
    planned(
        m::MODEL_READ,
        "A model: regions, permutations, variants, change colours",
        r#"{"tag":"string"}"#,
        "player models (not started)",
    ),
    planned(
        m::MODEL_RENDER,
        "Render a model variant with colours from a camera to a PNG",
        r#"{"tag":"string","variant?":"string","colors?":"any","camera?":"any","out?":"string"}"#,
        "player models (not started)",
    ),
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
        check_params(d, &params)?;
        for p in &mut self.providers {
            if let Some(r) = p.call(app, method, &params) {
                return r;
            }
        }
        if let Some(lane) = d.planned {
            let mut e = err(
                code::NOT_YET,
                format!("{method} is planned, not built yet: {lane} builds it"),
            );
            e.data = Some(json!({ "lane": lane, "method": method }));
            return Err(e);
        }
        let p = P(&params);
        match method.split('.').next().unwrap_or("") {
            "session" => self.session(app, method, &p),
            "source" | "doc" => docs::call(app, method, &p),
            "tag" => tags::call(app, method, &p),
            "scenario" => scenario::call(app, method, &p),
            "variant" => variants::call(app, method, &p),
            "playlist" => playlists::call(app, method, &p),
            "ui" => ui_screens::call(app, method, &p),
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

    /// Answers every request waiting on `rx`; returns how many.
    pub fn pump(
        &mut self,
        app: &mut App,
        rx: &std::sync::mpsc::Receiver<server::Incoming>,
    ) -> usize {
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
        .find(|(k, req)| *req && obj.get(k).is_none_or(Value::is_null))
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
    }
}
