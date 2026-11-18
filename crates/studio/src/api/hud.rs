//! The HUD tool as the agent API's `hud.*` ([`super::Provider`],
//! registered in [`super::seams`]): a map's HUDs, one HUD's widgets with
//! their fields and where they land, edits, the picture and its checks,
//! the save. Edits go through the HUD tool's actions (`Action::Hud*`), so
//! an agent's edit is the window's undo step and the window's save;
//! docs/editor/hud-editor.md, docs/editor/agent-api.md.

use std::path::PathBuf;

use agent_proto::RpcError;
use hud_edit::checks::Severity;
use hud_edit::layout::{Field, Kind};
use hud_edit::{HudState, Mode, Value as HudValue, WidgetKey, WidgetKind};
use map::{FourCC, TagId};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::scenario::{group_text, refused, tag_id};
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::Action;

/// The HUD tool's provider.
#[derive(Default)]
pub struct Hud;

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

/// `hud` picks the HUD (its tag name, or id); the HUD shown otherwise.
/// `widget` is `bitmap:<i>`, `text:<i>` or the widget's name.
const METHODS: &[MethodDef] = &[
    def(
        m::HUD_LIST,
        "The HUDs (nhdt) of the map open in the HUD tool, who names them and which are edited. `map` (a map of the source) or `package` (a Maps list key) opens one first; with neither and nothing open, the map open in the scenario tool",
        r#"{"map?":"string","package?":"string","discard?":"boolean"}"#,
        r#"{"map":"string","current":"string|null","huds":"[{id,name,category,halo1,owners:[{group,name,at}],edited}]","dirty":"boolean"}"#,
        r#"{"map":"hudtest"}"#,
    ),
    def(
        m::HUD_READ,
        "One HUD's widgets (shown and edited from now on): key, name, every field by its key, and where each lands at the HUD state (point, rect, shown, drawn, text)",
        r#"{"hud?":"string","state?":"object"}"#,
        r#"{"hud":"string","id":"string","state":"object","widgets":"[{widget,kind,index,name,fields:{key: value},placed}]","fields":"{bitmap:[{key,label,group,kind,mode}],text:[...]}"}"#,
        r#"{"hud":"invented\\hud\\player"}"#,
    ),
    def(
        m::HUD_SET,
        "Set a widget's field by its key (hud.read lists them): integers, numbers, tag references as a tag name, id or null. One undo step; `merge` joins the last step of the same field",
        r#"{"hud?":"string","widget":"string","field":"string","value":"any","merge?":"boolean"}"#,
        r#"{"widget":"string","field":"string","value":"any"}"#,
        r#"{"widget":"bitmap:0","field":"anchor","value":2}"#,
    ),
    def(
        m::HUD_MOVE,
        "Move a widget to an offset in a screen mode (full, half, quarter; default the state's), one undo step; `merge` joins a drag's moves",
        r#"{"hud?":"string","widget":"string","x":"integer","y":"integer","mode?":"string","merge?":"boolean"}"#,
        r#"{"widget":"string","mode":"string","offset":"[x,y]"}"#,
        r#"{"widget":"bitmap:0","x":12,"y":-40}"#,
    ),
    def(
        m::HUD_RENDER,
        "Draw the HUD at a state (mode, shield, body, grenade, frags, plasmas, rounds_loaded, magazine, rounds_reserve, heat, battery, zoomed, gate, words, inputs) to a PNG (default: the cache directory's studio/hud/; never into a Git checkout)",
        r#"{"hud?":"string","state?":"object","out?":"string"}"#,
        r#"{"path":"string","width":"integer","height":"integer","findings":"integer","fonts_found":"boolean"}"#,
        r#"{"out":"$OUT/hud.png","state":{"mode":"half"}}"#,
    ),
    def(
        m::HUD_CHECK,
        "The layout checks at a state: overlaps, art off the screen, text spilling out of its art",
        r#"{"hud?":"string","state?":"object"}"#,
        r#"[{"severity":"note|warning","text":"string","widgets":"[{hud,widget}]"}]"#,
        r#"{"state":{"gate":true}}"#,
    ),
    def(
        m::HUD_SAVE_AS,
        "Write the map with the HUD edits as a new .map (never over its source, never into a Git checkout); doc.save_as {\"doc\":\"hud\"} is the same",
        r#"{"path":"string"}"#,
        r#"{"saved":"boolean","path":"string","bytes":"integer|null","log":"[string]"}"#,
        r#"{"path":"$OUT/hud.map"}"#,
    ),
];

impl Provider for Hud {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !METHODS.iter().any(|d| d.name == method) {
            return None;
        }
        Some(call(app, method, &P(params)))
    }
}

fn run(app: &mut App, a: Action, discard: bool) -> Result<Vec<String>, RpcError> {
    app.perform(a, discard).map_err(refused)
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    if method == m::HUD_LIST {
        return list(app, p);
    }
    if method == m::HUD_SAVE_AS {
        return save_as(app, PathBuf::from(p.str("path")?));
    }
    need_open(app)?;
    pick(app, p)?;
    state(app, p)?;
    match method {
        m::HUD_READ => read(app),
        m::HUD_SET => {
            let key = widget(app, p.str("widget")?)?;
            let name = p.str("field")?;
            let f = *key.kind.fields().iter().find(|f| f.key == name).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!(
                        "field: no {name:?} on a {} widget ({})",
                        kind_name(key.kind),
                        key.kind.fields().iter().map(|f| f.key).collect::<Vec<_>>().join(", ")
                    ),
                )
            })?;
            let v = to_value(app, &f, p.get("value").unwrap_or(&Value::Null))?;
            run(app, Action::HudSet(key, name.to_owned(), v, p.bool("merge")?), false)?;
            let w = current_widget(app, key)?;
            Ok(json!({
                "widget": widget_name(key),
                "field": name,
                "value": field_json(app, &f, w.get_field(&f)),
            }))
        }
        m::HUD_MOVE => {
            let key = widget(app, p.str("widget")?)?;
            let coord = |k: &str| -> Result<i16, RpcError> {
                i16::try_from(p.i64(k)?)
                    .map_err(|_| err(code::INVALID_PARAMS, format!("{k}: out of range")))
            };
            let to = (coord("x")?, coord("y")?);
            if let Some(mode) = p.opt_str("mode")? {
                let mode = parse_mode(mode)?;
                if mode != app.hud.state.mode {
                    let mut st = app.hud.state.clone();
                    st.mode = mode;
                    run(app, Action::HudState(Box::new(st)), false)?;
                }
            }
            run(app, Action::HudMove(key, to, p.bool("merge")?), false)?;
            let mode = app.hud.state.mode;
            let (x, y) = app.hud.offset(key).unwrap_or(to);
            Ok(json!({ "widget": widget_name(key), "mode": mode_name(mode), "offset": [x, y] }))
        }
        m::HUD_RENDER => {
            let out = match p.opt_str("out")? {
                Some(o) => PathBuf::from(o),
                None => {
                    let s = app.hud.session.as_ref().expect("open");
                    let stem = s
                        .hud()
                        .map(|h| h.name.replace(['\\', '/'], "_"))
                        .unwrap_or_else(|| "hud".into());
                    studio_proto::output_dir().join("hud").join(format!("{stem}.png"))
                }
            };
            super::docs::outside_git(&out)?;
            if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)
                    .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
            }
            let Some(shot) = app.hud.shot() else {
                return Err(err(code::FAILED, "the HUD could not be drawn"));
            };
            shot.picture
                .canvas
                .write_png(&out)
                .map_err(|e| err(code::FAILED, format!("{}: {e:#}", out.display())))?;
            Ok(json!({
                "path": out.display().to_string(),
                "width": shot.picture.canvas.width,
                "height": shot.picture.canvas.height,
                "findings": shot.findings.len(),
                "fonts_found": shot.picture.fonts_found,
            }))
        }
        m::HUD_CHECK => {
            let Some(shot) = app.hud.shot() else {
                return Err(err(code::FAILED, "the HUD could not be drawn"));
            };
            Ok(Value::Array(
                shot.findings
                    .iter()
                    .map(|f| {
                        json!({
                            "severity": match f.severity {
                                Severity::Note => "note",
                                Severity::Warning => "warning",
                            },
                            "text": f.text,
                            "widgets": f.widgets.iter().map(|(hud, k)| json!({"hud": hud, "widget": widget_name(*k)})).collect::<Vec<_>>(),
                        })
                    })
                    .collect(),
            ))
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

fn need_open(app: &App) -> Result<(), RpcError> {
    if app.hud.session.is_some() {
        Ok(())
    } else {
        Err(err(
            code::NO_DOCUMENT,
            "no map is open in the HUD tool: hud.list {\"map\": ...}",
        ))
    }
}

fn list(app: &mut App, p: &P) -> Reply {
    let discard = p.bool("discard")?;
    let open = match (p.opt_str("map")?, p.opt_str("package")?) {
        (Some(_), Some(_)) => {
            return Err(err(code::INVALID_PARAMS, "hud.list takes map or package, not both"));
        }
        (Some(map), None) => Some(Action::HudOpenMap(map.to_owned())),
        (None, Some(key)) => Some(Action::HudOpenPackage(key.to_owned())),
        (None, None) if app.hud.session.is_none() => match app.loaded_map() {
            Some(map) => Some(Action::HudOpenMap(map.to_owned())),
            None => {
                return Err(err(
                    code::NO_DOCUMENT,
                    "no map is open: hud.list {\"map\": ...} (source.maps lists them)",
                ));
            }
        },
        (None, None) => None,
    };
    if let Some(a) = open {
        run(app, a, discard)?;
    }
    need_open(app)?;
    let s = app.hud.session.as_ref().expect("open");
    let edited = s.edited();
    let huds: Vec<Value> = s
        .huds
        .iter()
        .map(|h| {
            json!({
                "id": tag_id(h.id),
                "name": h.name,
                "category": h.category(),
                "halo1": h.halo1,
                "owners": h.owners.iter().map(|o| json!({
                    "group": group_text(o.group),
                    "name": o.name,
                    "at": o.at,
                })).collect::<Vec<_>>(),
                "edited": edited.contains(&h.id),
            })
        })
        .collect();
    Ok(json!({
        "map": s.name,
        "current": s.hud().map(|h| h.name.clone()),
        "huds": huds,
        "dirty": s.is_dirty(),
    }))
}

/// Shows the HUD `hud` names, if given.
fn pick(app: &mut App, p: &P) -> Result<(), RpcError> {
    let Some(want) = p.opt_str("hud")? else {
        if app.hud.session.as_ref().is_some_and(|s| s.current.is_none()) {
            return Err(err(code::NO_DOCUMENT, "the map has no HUD shown: give hud"));
        }
        return Ok(());
    };
    let s = app.hud.session.as_ref().expect("open");
    let parsed = want
        .strip_prefix("0x")
        .and_then(|h| u32::from_str_radix(h, 16).ok());
    let id = s
        .huds
        .iter()
        .find(|h| h.name == want || Some(h.id.0) == parsed)
        .or_else(|| {
            let lower = want.to_ascii_lowercase();
            let mut hits = s.huds.iter().filter(|h| h.name.to_ascii_lowercase().contains(&lower));
            match (hits.next(), hits.next()) {
                (Some(h), None) => Some(h),
                _ => None,
            }
        })
        .map(|h| h.id)
        .ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!(
                    "hud: no one HUD {want:?} ({})",
                    s.huds.iter().map(|h| h.name.as_str()).collect::<Vec<_>>().join(", ")
                ),
            )
        })?;
    if s.current != Some(id) {
        run(app, Action::HudPick(id), false)?;
    }
    Ok(())
}

/// Applies `state` over the tool's state, when given.
fn state(app: &mut App, p: &P) -> Result<(), RpcError> {
    let Some(v) = p.get("state") else {
        return Ok(());
    };
    let Some(o) = v.as_object() else {
        return Err(err(code::INVALID_PARAMS, "state: an object"));
    };
    let mut st = app.hud.state.clone();
    for (k, v) in o {
        let bad = || err(code::INVALID_PARAMS, format!("state.{k}: {v} does not fit"));
        let unit = || -> Result<f32, RpcError> {
            v.as_f64()
                .filter(|x| (0.0..=1.0).contains(x))
                .map(|x| x as f32)
                .ok_or_else(|| err(code::INVALID_PARAMS, format!("state.{k}: 0 to 1")))
        };
        let int = |max: u64| v.as_u64().filter(|n| *n <= max).ok_or_else(bad);
        match k.as_str() {
            "mode" => st.mode = parse_mode(v.as_str().ok_or_else(bad)?)?,
            "shield" => st.shield = unit()?,
            "body" => st.body = unit()?,
            "heat" => st.heat = unit()?,
            "battery" => st.battery = unit()?,
            "grenade" => {
                st.grenade = match v.as_str() {
                    Some("frag") => hud_edit::state::Grenade::Frag,
                    Some("plasma") => hud_edit::state::Grenade::Plasma,
                    _ => return Err(err(code::INVALID_PARAMS, "state.grenade: frag or plasma")),
                }
            }
            "frags" => st.frags = int(u8::MAX.into())? as u8,
            "plasmas" => st.plasmas = int(u8::MAX.into())? as u8,
            "rounds_loaded" => st.rounds_loaded = int(u16::MAX.into())? as u16,
            "magazine" => st.magazine = int(u16::MAX.into())? as u16,
            "rounds_reserve" => st.rounds_reserve = int(u16::MAX.into())? as u16,
            "zoomed" => st.zoomed = v.as_bool().ok_or_else(bad)?,
            "gate" => st.gate = v.as_bool().ok_or_else(bad)?,
            "words" => {
                let a = v.as_array().filter(|a| a.len() == 4).ok_or_else(bad)?;
                for (i, w) in a.iter().enumerate() {
                    st.words[i] = w
                        .as_u64()
                        .and_then(|n| u16::try_from(n).ok())
                        .ok_or_else(bad)?;
                }
            }
            "inputs" => {
                let m = v.as_object().ok_or_else(bad)?;
                st.inputs.clear();
                for (i, x) in m {
                    let i: u8 = i.parse().map_err(|_| bad())?;
                    st.inputs.insert(i, x.as_f64().ok_or_else(bad)? as f32);
                }
            }
            _ => {
                return Err(err(
                    code::INVALID_PARAMS,
                    format!("state: no {k:?} (mode, shield, body, grenade, frags, plasmas, rounds_loaded, magazine, rounds_reserve, heat, battery, zoomed, gate, words, inputs)"),
                ));
            }
        }
    }
    if st != app.hud.state {
        run(app, Action::HudState(Box::new(st)), false)?;
    }
    Ok(())
}

fn state_json(st: &HudState) -> Value {
    json!({
        "mode": mode_name(st.mode),
        "shield": st.shield,
        "body": st.body,
        "grenade": match st.grenade {
            hud_edit::state::Grenade::Frag => "frag",
            hud_edit::state::Grenade::Plasma => "plasma",
        },
        "frags": st.frags,
        "plasmas": st.plasmas,
        "rounds_loaded": st.rounds_loaded,
        "magazine": st.magazine,
        "rounds_reserve": st.rounds_reserve,
        "heat": st.heat,
        "battery": st.battery,
        "zoomed": st.zoomed,
        "gate": st.gate,
        "words": st.words,
        "inputs": st.inputs.iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>(),
    })
}

fn parse_mode(s: &str) -> Result<Mode, RpcError> {
    match s {
        "full" | "fullscreen" => Ok(Mode::Full),
        "half" => Ok(Mode::Half),
        "quarter" => Ok(Mode::Quarter),
        _ => Err(err(code::INVALID_PARAMS, format!("mode: full, half or quarter, not {s:?}"))),
    }
}

fn mode_name(m: Mode) -> &'static str {
    match m {
        Mode::Full => "full",
        Mode::Half => "half",
        Mode::Quarter => "quarter",
    }
}

fn kind_name(k: WidgetKind) -> &'static str {
    match k {
        WidgetKind::Bitmap => "bitmap",
        WidgetKind::Text => "text",
    }
}

pub fn widget_name(k: WidgetKey) -> String {
    format!("{}:{}", kind_name(k.kind), k.index)
}

/// `bitmap:<i>`, `text:<i>` or a widget's name, on the HUD shown.
fn widget(app: &App, text: &str) -> Result<WidgetKey, RpcError> {
    let hud = app
        .hud
        .session
        .as_ref()
        .and_then(|s| s.hud())
        .ok_or_else(|| err(code::NO_DOCUMENT, "no HUD shown"))?;
    let by_key = text.split_once(':').and_then(|(k, i)| {
        let kind = match k {
            "bitmap" => WidgetKind::Bitmap,
            "text" => WidgetKind::Text,
            _ => return None,
        };
        Some(WidgetKey { kind, index: i.parse().ok()? })
    });
    let found = match by_key {
        Some(key) => hud.widget(key).map(|w| w.key),
        None => {
            let mut hits = hud.widgets.iter().filter(|w| w.name == text);
            match (hits.next(), hits.next()) {
                (Some(w), None) => Some(w.key),
                _ => None,
            }
        }
    };
    found.ok_or_else(|| {
        err(
            code::INVALID_PARAMS,
            format!(
                "widget: no one {text:?} on {} ({})",
                hud.name,
                hud.widgets
                    .iter()
                    .map(|w| format!("{} {}", widget_name(w.key), w.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })
}

fn current_widget(app: &App, key: WidgetKey) -> Result<hud_edit::Widget, RpcError> {
    app.hud
        .session
        .as_ref()
        .and_then(|s| s.hud())
        .and_then(|h| h.widget(key))
        .cloned()
        .ok_or_else(|| err(code::NO_DOCUMENT, "no HUD shown"))
}

/// The group a reference field takes.
fn ref_group(f: &Field) -> Option<FourCC> {
    match f.key {
        "bitmap" => Some(FourCC(*b"bitm")),
        "shader" => Some(FourCC(*b"shad")),
        _ => None,
    }
}

fn kind_text(k: Kind) -> &'static str {
    match k {
        Kind::I8 => "i8",
        Kind::U8 => "u8",
        Kind::I16 => "i16",
        Kind::U16 => "u16",
        Kind::Hex16 => "flags16",
        Kind::F32 => "f32",
        Kind::TagRef => "tag_ref",
        Kind::StringId => "string_id",
    }
}

fn field_json(app: &App, _f: &Field, v: HudValue) -> Value {
    let s = app.hud.session.as_ref();
    match v {
        HudValue::Int(i) => json!(i),
        HudValue::Float(x) => json!(x),
        HudValue::Tag(id) if id == TagId::NULL => Value::Null,
        HudValue::Tag(id) => json!({
            "id": tag_id(id),
            "name": s.map(|s| s.tag_name(id)),
        }),
        HudValue::String(sid) => json!(s.and_then(|s| s.string(sid)).unwrap_or_else(|| format!("{sid:#x}"))),
    }
}

/// A JSON value as field `f`'s.
fn to_value(app: &App, f: &Field, v: &Value) -> Result<HudValue, RpcError> {
    let bad = |why: &str| err(code::INVALID_PARAMS, format!("{}: {why}", f.key));
    match f.kind {
        Kind::I8 | Kind::U8 | Kind::I16 | Kind::U16 | Kind::Hex16 => {
            let n = match v {
                Value::Number(n) => n.as_i64(),
                // Flag words read best in hex.
                Value::String(s) => s.strip_prefix("0x").and_then(|h| i64::from_str_radix(h, 16).ok()),
                _ => None,
            };
            n.map(HudValue::Int).ok_or_else(|| bad("a whole number"))
        }
        Kind::F32 => v
            .as_f64()
            .filter(|x| x.is_finite())
            .map(|x| HudValue::Float(x as f32))
            .ok_or_else(|| bad("a number")),
        Kind::TagRef => {
            if v.is_null() {
                return Ok(HudValue::Tag(TagId::NULL));
            }
            let s = app.hud.session.as_ref().expect("open");
            let tags = ref_group(f).map(|g| s.tags_of(g)).unwrap_or_default();
            let id = match v {
                Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()).map(TagId),
                Value::String(t) => t
                    .strip_prefix("0x")
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .map(TagId)
                    .or_else(|| tags.iter().find(|(_, n)| n == t).map(|(id, _)| *id)),
                _ => None,
            }
            .ok_or_else(|| bad("a tag name, id or null"))?;
            if !tags.is_empty() && !tags.iter().any(|(t, _)| *t == id) {
                return Err(bad(&format!(
                    "{} is not a tag of the field's group ({})",
                    tag_id(id),
                    tags.iter().map(|(_, n)| n.as_str()).collect::<Vec<_>>().join(", ")
                )));
            }
            Ok(HudValue::Tag(id))
        }
        Kind::StringId => Err(err(
            code::REFUSED,
            format!("{}: string IDs are not edited here (the Tags tool edits them)", f.key),
        )),
    }
}

fn read(app: &mut App) -> Reply {
    let Some(placed) = app.hud.shot().map(|s| s.picture.placed.clone()) else {
        return Err(err(code::FAILED, "the HUD could not be drawn"));
    };
    let s = app.hud.session.as_ref().expect("open");
    let hud = s.hud().ok_or_else(|| err(code::NO_DOCUMENT, "no HUD shown"))?;
    let widgets: Vec<Value> = hud
        .widgets
        .iter()
        .map(|w| {
            let fields: serde_json::Map<String, Value> = w
                .key
                .kind
                .fields()
                .iter()
                .map(|f| (f.key.to_owned(), field_json(app, f, w.get_field(f))))
                .collect();
            let at = placed.iter().find(|p| p.hud == 0 && p.key == w.key).map(|p| {
                json!({
                    "point": p.point,
                    "rect": p.rect.map(|r| [r.x0, r.y0, r.x1, r.y1]),
                    "shown": p.shown,
                    "drawn": p.drawn,
                    "text": p.text,
                })
            });
            json!({
                "widget": widget_name(w.key),
                "kind": kind_name(w.key.kind),
                "index": w.key.index,
                "name": w.name,
                "edited": w.is_dirty(),
                "fields": fields,
                "placed": at,
            })
        })
        .collect();
    let table = |k: WidgetKind| {
        k.fields()
            .iter()
            .map(|f| {
                json!({
                    "key": f.key,
                    "label": f.label,
                    "group": f.group,
                    "kind": kind_text(f.kind),
                    "mode": f.mode.map(mode_name),
                })
            })
            .collect::<Vec<_>>()
    };
    Ok(json!({
        "hud": hud.name,
        "id": tag_id(hud.id),
        "state": state_json(&app.hud.state),
        "widgets": widgets,
        "fields": { "bitmap": table(WidgetKind::Bitmap), "text": table(WidgetKind::Text) },
    }))
}

/// `hud.save_as`, and `doc.save_as {"doc":"hud"}`.
pub fn save_as(app: &mut App, path: PathBuf) -> Reply {
    need_open(app)?;
    super::docs::outside_git(&path)?;
    let s = app.hud.session.as_ref().expect("open");
    if super::docs::same_file(&s.source.path, &path) {
        return Err(err(
            code::REFUSED,
            "that is the map's source: the editor never overwrites it",
        ));
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)
            .map_err(|e| err(code::FAILED, format!("{}: {e}", dir.display())))?;
    }
    let log = run(app, Action::HudSaveAs(path.clone()), false)?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).ok();
    Ok(json!({ "saved": true, "path": path.display().to_string(), "bytes": bytes, "log": log }))
}

/// `doc.diff {"doc":"hud"}`: the patches a save writes, by widget.
pub fn diff(app: &App) -> Reply {
    need_open(app)?;
    let s = app.hud.session.as_ref().expect("open");
    let patches: Vec<Value> = s
        .patches()
        .iter()
        .map(|(address, bytes)| {
            let mut v = json!({
                "address": format!("{address:#010x}"),
                "len": bytes.len(),
                "bytes": super::docs::hex(bytes),
            });
            for id in s.edited() {
                if let Some(h) = s.hud_by(id)
                    && let Some(w) = h.widgets.iter().find(|w| {
                        *address >= w.address && (*address as usize) < w.address as usize + w.bytes.len()
                    })
                {
                    let off = (address - w.address) as usize;
                    let end = off + bytes.len();
                    v["hud"] = json!(h.name);
                    v["widget"] = json!(widget_name(w.key));
                    v["fields"] = json!(
                        w.key
                            .kind
                            .fields()
                            .iter()
                            .filter(|f| f.offset < end && off < f.offset + f.kind.size())
                            .map(|f| f.key)
                            .collect::<Vec<_>>()
                    );
                }
            }
            v
        })
        .collect();
    Ok(json!({ "doc": "hud", "dirty": s.is_dirty(), "patches": patches }))
}
