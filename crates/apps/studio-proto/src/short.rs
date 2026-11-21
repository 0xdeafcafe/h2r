//! Short forms: `h2 studio set vehicles:0 position [1,2,3]` for
//! `scenario.set {"block":"vehicles","index":0,"field":"position","value":[1,2,3]}`.
//! The same conventions as `h2 agent`: a method and JSON params, or a short
//! form; a JSON value is one word whatever spaces it holds.

use serde_json::{Value, json};

use crate::method as m;

/// The short forms, for the usage text: (form, what it calls).
pub const FORMS: &[(&str, &str)] = &[
    ("status", "session.status"),
    (
        "methods",
        "session.methods: every method, its params and result",
    ),
    ("log [N]", "session.log: the editor's last N log lines"),
    (
        "source <path>",
        "source.open: a disc image, a .map or a maps folder",
    ),
    ("maps", "source.maps"),
    ("docs", "doc.list: what is open, dirty, undoable"),
    ("open <map>", "doc.open {\"map\": ...}"),
    ("save [doc]", "doc.save (variant, playlists)"),
    ("save-as <path> [doc]", "doc.save_as (map, playlists)"),
    ("diff [doc]", "doc.diff: what a save would write"),
    ("undo [doc] | redo [doc]", "doc.undo, doc.redo"),
    ("tags [group] [name]", "tag.list"),
    ("tag <id|name>", "tag.read"),
    ("refs <id|name>", "tag.refs: the tags it references"),
    ("blocks", "scenario.blocks"),
    ("objects [block]", "scenario.list"),
    ("object <block>:<i>", "scenario.get"),
    ("set <block>:<i> <field> <JSON>", "scenario.set"),
    ("add <block> [like]", "scenario.add"),
    ("move <block>:<i> <x,y,z>", "scenario.move"),
    ("delete <block>:<i>", "scenario.delete"),
    ("duplicate <block>:<i>", "scenario.duplicate"),
    ("select <block>:<i>", "scenario.select"),
    ("variants", "variant.list"),
    ("variant", "variant.get"),
    ("setting <id> <value>", "variant.set"),
    ("playlists", "playlist.list"),
    ("playlist <0..15>", "playlist.get"),
    ("screens", "ui.screens"),
    ("screen <screen>[/<state>]", "ui.pick"),
    (
        "press <input>",
        "ui.press (a b x y up down left right start back ...)",
    ),
    ("widgets", "ui.widgets"),
    ("render-ui [out.png]", "ui.render"),
    ("snapshot [out.png]", "view.snapshot"),
    (
        "render-stats",
        "view.stats: lightmaps, materials, the sky's sun and fog",
    ),
    ("huds [map]", "hud.list: open a map in the HUD tool, list its HUDs"),
    ("hud [name]", "hud.read: a HUD's widgets and fields"),
    ("render-hud [out.png]", "hud.render"),
];

/// Splits a script line into words; a JSON value (`{…}` or `[…]`) is one
/// word whatever spaces it holds.
pub fn split(line: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut rest = line.trim_start();
    while !rest.is_empty() {
        if rest.starts_with('{') || rest.starts_with('[') {
            let mut de = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
            match de.next() {
                Some(Ok(_)) => {}
                _ => return Err(format!("bad JSON in {line}")),
            }
            let n = de.byte_offset();
            out.push(rest[..n].to_owned());
            rest = rest[n..].trim_start();
        } else {
            let n = rest.find(char::is_whitespace).unwrap_or(rest.len());
            out.push(rest[..n].to_owned());
            rest = rest[n..].trim_start();
        }
    }
    Ok(out)
}

/// `vehicles:0` as `{"block":"vehicles","index":0}`.
fn entry(w: Option<&String>, form: &str) -> Result<Value, String> {
    let w = w.ok_or_else(|| format!("{form} needs <block>:<index>"))?;
    let (block, index) = w
        .split_once(':')
        .ok_or_else(|| format!("{form}: {w} is not <block>:<index>"))?;
    let index: u64 = index
        .parse()
        .map_err(|_| format!("{form}: {index} is not an index"))?;
    Ok(json!({ "block": block, "index": index }))
}

/// A word as JSON when it parses, else as a string.
fn loose(w: &str) -> Value {
    serde_json::from_str(w).unwrap_or_else(|_| Value::from(w))
}

fn json_arg(w: Option<&String>) -> Result<Value, String> {
    match w {
        None => Ok(Value::Null),
        Some(s) => serde_json::from_str(s).map_err(|e| format!("not JSON ({e}): {s}")),
    }
}

/// A call's method and params from its words.
pub fn parse_call(words: &[String]) -> Result<(String, Value), String> {
    let name = words.first().ok_or("no call")?.as_str();
    let a = words.get(1);
    let b = words.get(2);
    let max = |n: usize| -> Result<(), String> {
        match words.get(n) {
            Some(w) => Err(format!("{name}: unexpected {w}")),
            None => Ok(()),
        }
    };
    let doc = |w: Option<&String>| match w {
        Some(d) => json!({ "doc": d }),
        None => Value::Null,
    };
    let (method, params) = match name {
        "status" => (m::STATUS, Value::Null),
        "methods" => (m::METHODS, Value::Null),
        "log" => (
            m::LOG,
            match a {
                Some(n) => json!({ "last": n.parse::<u64>().map_err(|_| "log N: a number")? }),
                None => Value::Null,
            },
        ),
        "source" => (
            m::SOURCE_OPEN,
            json!({ "path": a.ok_or("source needs a path")? }),
        ),
        "maps" => (m::SOURCE_MAPS, Value::Null),
        "docs" => (m::DOC_LIST, Value::Null),
        "open" => (
            m::DOC_OPEN,
            json!({ "map": a.ok_or("open needs a map name")? }),
        ),
        "save" => (m::DOC_SAVE, doc(a)),
        "save-as" => {
            let mut p = json!({ "path": a.ok_or("save-as needs a path")? });
            if let Some(d) = b {
                p["doc"] = json!(d);
            }
            max(3)?;
            (m::DOC_SAVE_AS, p)
        }
        "diff" => (m::DOC_DIFF, doc(a)),
        "undo" => (m::DOC_UNDO, doc(a)),
        "redo" => (m::DOC_REDO, doc(a)),
        "tags" => {
            let mut p = json!({});
            if let Some(g) = a {
                p["group"] = json!(g);
            }
            if let Some(n) = b {
                p["name"] = json!(n);
            }
            max(3)?;
            (m::TAG_LIST, p)
        }
        "tag" => (
            m::TAG_READ,
            json!({ "tag": a.ok_or("tag needs an id or name")? }),
        ),
        "refs" => (
            m::TAG_REFS,
            json!({ "tag": a.ok_or("refs needs an id or name")? }),
        ),
        "blocks" => (m::SCENARIO_BLOCKS, Value::Null),
        "objects" => (
            m::SCENARIO_LIST,
            match a {
                Some(bk) => json!({ "block": bk }),
                None => Value::Null,
            },
        ),
        "object" => (m::SCENARIO_GET, entry(a, name)?),
        "set" => {
            let mut p = entry(a, name)?;
            p["field"] = json!(b.ok_or("set needs a field")?);
            p["value"] = loose(words.get(3).ok_or("set needs a value")?);
            max(4)?;
            (m::SCENARIO_SET, p)
        }
        "add" => {
            let mut p = json!({ "block": a.ok_or("add needs a block")? });
            if let Some(like) = b {
                p["like"] = json!(like.parse::<u64>().map_err(|_| "add: like is an index")?);
            }
            max(3)?;
            (m::SCENARIO_ADD, p)
        }
        "move" => {
            let mut p = entry(a, name)?;
            let v = b.ok_or("move needs x,y,z")?;
            let xyz: Vec<f64> = v
                .trim_matches(['[', ']'])
                .split(',')
                .map(|s| s.trim().parse::<f64>())
                .collect::<Result<_, _>>()
                .map_err(|_| format!("move: {v} is not x,y,z"))?;
            if xyz.len() != 3 {
                return Err(format!("move: {v} is not x,y,z"));
            }
            p["position"] = json!(xyz);
            max(3)?;
            (m::SCENARIO_MOVE, p)
        }
        "delete" => (m::SCENARIO_DELETE, entry(a, name)?),
        "duplicate" => (m::SCENARIO_DUPLICATE, entry(a, name)?),
        "select" => (m::SCENARIO_SELECT, entry(a, name)?),
        "variants" => (m::VARIANT_LIST, Value::Null),
        "variant" => (m::VARIANT_GET, Value::Null),
        "setting" => {
            let id: u64 = a
                .ok_or("setting needs an id")?
                .parse()
                .map_err(|_| "setting: the id is a number")?;
            let v: i64 = b
                .ok_or("setting needs a value")?
                .parse()
                .map_err(|_| "setting: the value is a number")?;
            max(3)?;
            (m::VARIANT_SET, json!({ "setting": id, "value": v }))
        }
        "playlists" => (m::PLAYLIST_LIST, Value::Null),
        "playlist" => {
            let n: u64 = a
                .ok_or("playlist needs 0..15")?
                .parse()
                .map_err(|_| "playlist: 0..15")?;
            (m::PLAYLIST_GET, json!({ "playlist": n }))
        }
        "screens" => (m::UI_SCREENS, Value::Null),
        "screen" => {
            let pick = a.ok_or("screen needs <screen>[/<state>]")?;
            let (screen, state) = pick.split_once('/').unwrap_or((pick, ""));
            let mut p = json!({ "screen": screen });
            if !state.is_empty() {
                p["state"] = json!(state);
            }
            (m::UI_PICK, p)
        }
        "press" => (
            m::UI_PRESS,
            json!({ "input": a.ok_or("press needs an input")? }),
        ),
        "widgets" => (m::UI_WIDGETS, Value::Null),
        "render-ui" => (
            m::UI_RENDER,
            match a {
                Some(p) => json!({ "out": p }),
                None => Value::Null,
            },
        ),
        "huds" => (
            m::HUD_LIST,
            match a {
                Some(map) => json!({ "map": map }),
                None => Value::Null,
            },
        ),
        "hud" => (
            m::HUD_READ,
            match a {
                Some(h) => json!({ "hud": h }),
                None => Value::Null,
            },
        ),
        "render-hud" => (
            m::HUD_RENDER,
            match a {
                Some(p) => json!({ "out": p }),
                None => Value::Null,
            },
        ),
        "render-stats" => (m::VIEW_STATS, Value::Null),
        "snapshot" => (
            m::VIEW_SNAPSHOT,
            match a {
                Some(p) => json!({ "out": p }),
                None => Value::Null,
            },
        ),
        // A method by name, with JSON params.
        other if other.contains('.') => {
            max(2)?;
            return Ok((other.to_owned(), json_arg(a)?));
        }
        other => return Err(format!("unknown call {other} (`methods` lists them)")),
    };
    // Bare forms take nothing more than they use.
    let used = match name {
        "status" | "methods" | "maps" | "docs" | "blocks" | "variants" | "variant"
        | "playlists" | "screens" | "widgets" | "render-stats" => 1,
        "log" | "source" | "open" | "save" | "diff" | "undo" | "redo" | "tag" | "refs"
        | "objects" | "object" | "delete" | "duplicate" | "select" | "playlist" | "screen"
        | "press" | "render-ui" | "snapshot" | "huds" | "hud" | "render-hud" => 2,
        _ => words.len(),
    };
    max(used)?;
    Ok((method.to_owned(), params))
}
