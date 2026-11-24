//! `recording.list` and `recording.read`: the open map's recorded
//! animations (docs/editor/rich-editors.md, "Recordings", slice K1). A
//! read of the scenario's `recorded_animation` block with each stream
//! decoded as the player reads it (`hs::recording`); `played_by` names the
//! scripts that play each one, and needs the scripts to read.

use agent_proto::RpcError;
use hs::recording::{Decoded, Named, Value as Val};
use serde_json::{Map as Object, Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::recordings::{NONE, Op, Refusal, Row};

const LIST: MethodDef = def(
    m::RECORDING_LIST,
    "The open map's recorded animations: name, length in ticks, versions, stream size, how many events its stream holds (or why it does not read), and the scripts that play each",
    "{}",
    r#"{"recordings":"[{name,ticks,version,control_version,stream_bytes,events,problem?,played_by}]","played_by_unavailable?":"string","note?":"string"}"#,
    "{}",
);

const READ: MethodDef = def(
    m::RECORDING_READ,
    "One recording's stream, typed: the initial unit control state and each event with its tick, offset, delay, type name, values and payload bytes. A stream that does not read is REFUSED with the offset of the event that broke it",
    r#"{"name":"string (case-insensitive)","limit?":"integer (events, default 1000, max 20000)","offset?":"integer (events skipped)"}"#,
    r#"{"name":"string","version":"integer","control_version":"integer","ticks":"integer (the block's)","walked_ticks":"integer (the stream's)","stream_bytes":"integer","initial":"{fields:{name:value},trailer:hex}","events":"[{tick,at,delay,kind,name,fields:{name:value},payload:hex}]","total":"integer","offset":"integer","limit":"integer","truncated":"boolean","trailing":"integer"}"#,
    r#"{"name":"arrive"}"#,
);

const PLAY: MethodDef = def(
    m::RECORDING_PLAY,
    "Play one of the open map's recordings on a player's unit in the paired game (the game's recording.play: the engine's own recording player), and start keeping the path that unit takes from the streamed positions (recording.path; the Scene viewport draws it). REFUSED without a paired, running (not paused) game that lists recording.play; the game must be on a map that has the recording",
    r#"{"name":"string (case-insensitive)","unit?":"player.<n> (default player.0)","mode?":"play|delete|hover (how it ends; default play)"}"#,
    r#"{"player":"string","recording":"string","index":"integer","unit":"string","path":"{recording,player,points}"}"#,
    r#"{"name":"arrive"}"#,
);

const PATH: MethodDef = def(
    m::RECORDING_PATH,
    "The path the played recording's unit has taken: the unit's position from each streamed tick since recording.play, as the Scene viewport draws it. With clear, drops it",
    r#"{"clear?":"boolean","limit?":"integer (points, default 1000, from the end)"}"#,
    r#"{"recording":"string|null","player":"integer","count":"integer","points":"[[x,y,z]]","truncated":"boolean"}"#,
    "{}",
);

const WRITE: MethodDef = def(
    m::RECORDING_WRITE,
    "Edit one recording's stream (rich editors K3), all or nothing, as one undo step: set the initial unit control state by field, set or move events, insert events, remove events. Events keep their ticks: removing one gives its delay to the next, inserting one takes its delay from the next, moving one takes and gives the difference. The block's tick count becomes the stream's. The edit is the map's unsaved change; a stream longer than the source's needs Edit > Allow growing full blocks. REFUSED for a stream that does not read or is not version 4; INVALID_PARAMS for an edit that does not fit (the index of the one that failed is in the message)",
    r#"{"name":"string (case-insensitive)","initial?":"{field:value} (the names recording.read gives: numbers, or an array of reals)","trailer?":"hex (12 bytes)","events?":"[{op:'set',index,delay?,payload?|fields?} | {op:'insert',index,kind,delay?,payload?|fields?} | {op:'remove',index}] (applied in order; index is the event's number in recording.read; insert goes before it, the end event included; fields are the names recording.read gives: value, or a and b)"}"#,
    r#"{"name":"string","ticks":"integer","stream_bytes":"integer","events":"integer","grows":"boolean (the stream outgrew the source's and is appended on save)"}"#,
    r#"{"name":"arrive","events":[{"op":"remove","index":0}]}"#,
);

const REMOVE: MethodDef = def(
    m::RECORDING_REMOVE,
    "Remove a recording from the block (rich editors K3): the ones after it move down, one undo step. REFUSED while a script plays it, plays a later one by its number, or when the scripts do not read (who plays it cannot be known)",
    r#"{"name":"string (case-insensitive)"}"#,
    r#"{"removed":"string","recordings":"integer (left)"}"#,
    r#"{"name":"empty"}"#,
);

const UNDO: MethodDef = def(
    m::RECORDING_UNDO,
    "Undo the last recording edit, or with redo, redo the last undone one",
    r#"{"redo?":"boolean"}"#,
    r#"{"changed":"boolean (false: nothing to undo or redo)"}"#,
    "{}",
);

const DEFAULT_LIMIT: usize = 1000;
const MAX_LIMIT: usize = 20_000;

/// The Recordings tool's provider.
pub struct Recordings;

impl Provider for Recordings {
    fn methods(&self) -> Vec<MethodDef> {
        vec![LIST, READ, PLAY, PATH, WRITE, REMOVE, UNDO]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        match method {
            m::RECORDING_LIST => Some(list(app)),
            m::RECORDING_READ => Some(read(app, &P(params))),
            m::RECORDING_PLAY => Some(play(app, &P(params))),
            m::RECORDING_PATH => Some(path(app, &P(params))),
            m::RECORDING_WRITE => Some(write(app, &P(params))),
            m::RECORDING_REMOVE => Some(remove(app, &P(params))),
            m::RECORDING_UNDO => Some(undo(app, &P(params))),
            _ => None,
        }
    }
}

fn rows(app: &mut App) -> Result<Vec<Row>, RpcError> {
    app.sync_scripts();
    app.recordings_loaded().map(<[Row]>::to_vec).map_err(|why| {
        let c = if app.session.is_open() {
            code::REFUSED
        } else {
            code::NO_DOCUMENT
        };
        err(c, why)
    })
}

fn list(app: &mut App) -> Reply {
    let rows = rows(app)?;
    // Who plays each: the scripts' read, when there is one.
    let scripts = app.scripts_loaded();
    let mut v = Object::new();
    let recordings: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut o = json!({
                "name": r.recorded.name,
                "ticks": r.recorded.ticks,
                "version": r.recorded.version,
                "control_version": r.recorded.control_version,
                "stream_bytes": r.recorded.stream.len(),
                "events": r.decoded.as_ref().map(|d| d.events.len()).ok(),
                "played_by": match &scripts {
                    Ok(l) => Value::Array(
                        l.recording_uses(&rows, i)
                            .into_iter()
                            .map(|(item, name, count)| {
                                json!({"name": name, "item": item_kind(item), "count": count})
                            })
                            .collect(),
                    ),
                    Err(_) => Value::Null,
                },
            });
            if let Err(why) = &r.decoded {
                o["problem"] = json!(why);
            }
            o
        })
        .collect();
    if rows.is_empty() {
        v.insert("note".into(), json!(NONE));
    }
    if let Err(why) = &scripts {
        v.insert("played_by_unavailable".into(), json!(why));
    }
    v.insert("recordings".into(), Value::Array(recordings));
    Ok(Value::Object(v))
}

fn item_kind(item: hs::Item) -> &'static str {
    match item {
        hs::Item::Script(_) => "script",
        hs::Item::Global(_) => "global",
    }
}

fn read(app: &mut App, p: &P) -> Reply {
    let name = p.str("name")?;
    let limit = p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(err(
            code::INVALID_PARAMS,
            format!("limit: 1..{MAX_LIMIT}, not {limit}"),
        ));
    }
    let offset = p.opt_usize("offset")?.unwrap_or(0);
    let rows = rows(app)?;
    let row = rows
        .iter()
        .find(|r| r.recorded.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("name: no recording {name:?} (recording.list lists them)"),
            )
        })?;
    let d = row
        .decoded
        .as_ref()
        .map_err(|why| err(code::REFUSED, format!("recording {}: {why}", row.recorded.name)))?;
    Ok(read_json(row, d, offset, limit))
}

fn read_json(row: &Row, d: &Decoded, offset: usize, limit: usize) -> Value {
    let events: Vec<Value> = d
        .events
        .iter()
        .skip(offset)
        .take(limit)
        .map(|e| {
            json!({
                "tick": e.tick,
                "at": e.at,
                "delay": e.delay,
                "kind": e.kind,
                "name": e.name,
                "fields": fields(&e.fields),
                "payload": hex(&e.payload),
            })
        })
        .collect();
    json!({
        "name": row.recorded.name,
        "version": row.recorded.version,
        "control_version": row.recorded.control_version,
        "ticks": row.recorded.ticks,
        "walked_ticks": d.ticks,
        "stream_bytes": row.recorded.stream.len(),
        "initial": {"fields": fields(&d.initial.fields), "trailer": hex(&d.initial.trailer)},
        "events": events,
        "total": d.events.len(),
        "offset": offset,
        "limit": limit,
        "truncated": offset + limit < d.events.len(),
        "trailing": d.trailing,
    })
}

/// A value as JSON: a float that is not finite shows as null.
pub fn value(v: &Val) -> Value {
    match v {
        Val::Unsigned(n) => json!(n),
        Val::Signed(n) => json!(n),
        Val::Reals(r) => Value::Array(r.iter().map(|&f| json!(f)).collect()),
    }
}

fn fields(f: &[Named]) -> Value {
    Value::Object(f.iter().map(|(k, v)| ((*k).to_owned(), value(v))).collect())
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `recording.play`: the paired game plays the recording on a player's
/// unit. The call is made on a connection of its own, as the previews'
/// are, and answers on the game's next tick.
fn play(app: &mut App, p: &P) -> Reply {
    let name = p.str("name")?;
    let unit = p.opt_str("unit")?.unwrap_or("player.0");
    let player = unit
        .strip_prefix("player.")
        .and_then(|n| n.parse::<u16>().ok())
        .ok_or_else(|| err(code::INVALID_PARAMS, format!("unit: player.<n>, not {unit:?}")))?;
    let mode = p.opt_str("mode")?;
    if let Some(mode) = mode
        && !matches!(mode, "play" | "delete" | "hover")
    {
        return Err(err(
            code::INVALID_PARAMS,
            format!("mode: play, delete or hover, not {mode:?}"),
        ));
    }
    // The name must be one of this map's: the game reads its own block.
    let rows = rows(app)?;
    let row = rows
        .iter()
        .find(|r| r.recorded.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("name: no recording {name:?} (recording.list lists them)"),
            )
        })?;
    let recording = row.recorded.name.clone();
    play_on(app, &recording, player, mode)
}

/// The game's `recording.play` for `recording` (a name of this map's
/// block) on `player`'s unit, then the path started: what the API and the
/// Recordings section's button both do. The call is made on a connection
/// of its own, as the previews' are, and answers on the game's next tick.
pub(crate) fn play_on(app: &mut App, recording: &str, player: u16, mode: Option<&str>) -> Reply {
    use agent_proto::{CallError, Client};
    use std::time::Duration;

    let unit = format!("player.{player}");
    let (endpoint, paused) = super::preview::paired(app)?;
    if paused {
        return Err(err(
            code::REFUSED,
            "the game is paused: the call answers on its next tick (resume or step it)",
        ));
    }
    let mut game_params = json!({"recording": recording, "player": unit});
    if let Some(mode) = mode {
        game_params["mode"] = json!(mode);
    }
    let refused = |why: String| err(code::REFUSED, why);
    let mut client = Client::connect_with_timeout(&endpoint, Some(Duration::from_secs(5)))
        .map_err(|e| refused(format!("connecting to the paired game at {endpoint}: {e}")))?;
    client
        .set_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| refused(format!("{endpoint}: {e}")))?;
    let methods = client
        .call(agent_proto::method::METHODS, Value::Null)
        .map_err(|e| refused(format!("session.methods: {e}")))?;
    if !methods
        .as_array()
        .is_some_and(|l| l.iter().any(|name| name == m::RECORDING_PLAY))
    {
        return Err(refused(
            "the paired game does not list recording.play (a build before it)".into(),
        ));
    }
    let mut answer = client.call(m::RECORDING_PLAY, game_params).map_err(|e| match e {
        CallError::Rpc(e) => refused(format!("recording.play: {}", e.message)),
        e => refused(format!("recording.play: {e}")),
    })?;
    app.pairing.path.start(recording, player);
    answer["path"] = json!({"recording": recording, "player": player, "points": 0});
    Ok(answer)
}

fn refusal(r: Refusal) -> RpcError {
    match r {
        Refusal::Params(why) => err(code::INVALID_PARAMS, why),
        Refusal::Refused(why) => err(code::REFUSED, why),
    }
}

/// The index of the recording `name` names.
fn index_of(app: &mut App, name: &str) -> Result<usize, RpcError> {
    rows(app)?
        .iter()
        .position(|r| r.recorded.name.eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("name: no recording {name:?} (recording.list lists them)"),
            )
        })
}

/// A JSON number or array of numbers as a value: whole numbers are
/// integers (the width is checked where it is known), anything else reals.
fn json_value(k: &str, v: &Value) -> Result<Val, RpcError> {
    let bad = || err(code::INVALID_PARAMS, format!("{k}: a number or an array of numbers, not {v}"));
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                if let Ok(u) = u32::try_from(i) {
                    Ok(Val::Unsigned(u))
                } else {
                    i32::try_from(i).map(Val::Signed).map_err(|_| bad())
                }
            } else {
                Err(bad())
            }
        }
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_f64().map(|f| f as f32).ok_or_else(bad))
            .collect::<Result<_, _>>()
            .map(Val::Reals),
        _ => Err(bad()),
    }
}

fn unhex(k: &str, s: &str) -> Result<Vec<u8>, RpcError> {
    let bad = || err(code::INVALID_PARAMS, format!("{k}: hex bytes, not {s:?}"));
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err(bad());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| bad()))
        .collect()
}

/// One entry of `events`.
fn op(i: usize, o: &Value) -> Result<Op, RpcError> {
    let p = P(o);
    let at = |e: RpcError| err(e.code, format!("events[{i}]: {}", e.message));
    let index = p.usize("index").map_err(at)?;
    let delay = p
        .opt_u64("delay")
        .map_err(at)?
        .map(|d| u16::try_from(d).map_err(|_| err(code::INVALID_PARAMS, format!("events[{i}].delay: 0..65535, not {d}"))))
        .transpose()?;
    // The payload as bytes, or built from named values.
    let payload = |kind: Option<u8>| -> Result<Option<Vec<u8>>, RpcError> {
        if let Some(h) = p.opt_str("payload").map_err(at)? {
            return Ok(Some(unhex("payload", h)?));
        }
        let Some(f) = p.get("fields") else {
            return Ok(None);
        };
        let obj = f.as_object().ok_or_else(|| {
            err(code::INVALID_PARAMS, format!("events[{i}].fields: an object, not {f}"))
        })?;
        let kind = kind.ok_or_else(|| {
            err(code::INVALID_PARAMS, format!("events[{i}].fields: set takes payload hex; fields name an insert's values"))
        })?;
        let values: Vec<(&str, Val)> = obj
            .iter()
            .map(|(k, v)| Ok((k.as_str(), json_value(k, v)?)))
            .collect::<Result<_, RpcError>>()?;
        hs::recording_edit::payload_from(kind, &values)
            .map(Some)
            .map_err(|why| err(code::INVALID_PARAMS, format!("events[{i}].fields: {why}")))
    };
    match p.str("op").map_err(at)? {
        "set" => {
            let payload = payload(None)?;
            if delay.is_none() && payload.is_none() {
                return Err(err(code::INVALID_PARAMS, format!("events[{i}]: set needs delay or payload")));
            }
            Ok(Op::Set { index, delay, payload })
        }
        "insert" => {
            let kind = p.u64("kind").map_err(at)?;
            let kind = u8::try_from(kind).map_err(|_| {
                err(code::INVALID_PARAMS, format!("events[{i}].kind: 0..255, not {kind}"))
            })?;
            let payload = payload(Some(kind))?.unwrap_or_default();
            Ok(Op::Insert { index, kind, delay: delay.unwrap_or(0), payload })
        }
        "remove" => Ok(Op::Remove { index }),
        other => Err(err(
            code::INVALID_PARAMS,
            format!("events[{i}].op: set, insert or remove, not {other:?}"),
        )),
    }
}

/// `recording.write`.
fn write(app: &mut App, p: &P) -> Reply {
    let name = p.str("name")?;
    let mut ops = Vec::new();
    if let Some(initial) = p.get("initial") {
        let o = initial
            .as_object()
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("initial: an object, not {initial}")))?;
        for (k, v) in o {
            ops.push(Op::Initial { name: k.clone(), value: json_value(k, v)? });
        }
    }
    if let Some(t) = p.opt_str("trailer")? {
        ops.push(Op::Trailer(unhex("trailer", t)?));
    }
    if let Some(events) = p.get("events") {
        let list = events
            .as_array()
            .ok_or_else(|| err(code::INVALID_PARAMS, format!("events: an array, not {events}")))?;
        for (i, e) in list.iter().enumerate() {
            ops.push(op(i, e)?);
        }
    }
    if ops.is_empty() {
        return Err(err(code::INVALID_PARAMS, "initial, trailer or events: say what to change"));
    }
    let index = index_of(app, name)?;
    app.recording_write(index, &ops).map_err(refusal)?;
    let all = rows(app)?;
    let row = &all[index];
    let d = row.decoded.as_ref().expect("a written stream reads");
    let grows = row.recorded.stream.len()
        > app
            .scripts
            .recording_doc
            .as_ref()
            .map_or(usize::MAX, |doc| doc.entries[index].original.len());
    Ok(json!({
        "name": row.recorded.name,
        "ticks": row.recorded.ticks,
        "stream_bytes": row.recorded.stream.len(),
        "events": d.events.len(),
        "grows": grows,
    }))
}

/// `recording.remove`.
fn remove(app: &mut App, p: &P) -> Reply {
    let name = p.str("name")?;
    let index = index_of(app, name)?;
    let removed = rows(app)?[index].recorded.name.clone();
    app.recording_remove(index).map_err(refusal)?;
    Ok(json!({"removed": removed, "recordings": rows(app)?.len()}))
}

/// `recording.undo`.
fn undo(app: &mut App, p: &P) -> Reply {
    let changed = app.recording_step(p.bool("redo")?).map_err(refusal)?;
    Ok(json!({"changed": changed}))
}

/// `recording.path`: the points kept since the last play.
fn path(app: &mut App, p: &P) -> Reply {
    let limit = p.opt_usize("limit")?.unwrap_or(DEFAULT_LIMIT);
    if limit == 0 || limit > MAX_LIMIT {
        return Err(err(
            code::INVALID_PARAMS,
            format!("limit: 1..{MAX_LIMIT}, not {limit}"),
        ));
    }
    let clear = p.bool("clear")?;
    let path = &mut app.pairing.path;
    let out = json!({
        "recording": path.recording,
        "player": path.player,
        "count": path.points.len(),
        "points": path.points[path.points.len().saturating_sub(limit)..],
        "truncated": path.points.len() > limit,
    });
    if clear {
        path.clear();
    }
    Ok(out)
}
