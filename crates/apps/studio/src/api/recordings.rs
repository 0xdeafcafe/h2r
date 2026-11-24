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
use crate::app::recordings::{NONE, Row};

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

const DEFAULT_LIMIT: usize = 1000;
const MAX_LIMIT: usize = 20_000;

/// The Recordings tool's provider.
pub struct Recordings;

impl Provider for Recordings {
    fn methods(&self) -> Vec<MethodDef> {
        vec![LIST, READ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        match method {
            m::RECORDING_LIST => Some(list(app)),
            m::RECORDING_READ => Some(read(app, &P(params))),
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
