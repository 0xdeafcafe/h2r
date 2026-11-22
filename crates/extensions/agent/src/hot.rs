//! Hot iteration as host methods (docs/graphics/hot-iteration.md): live
//! tag edits (`edit.*`, over `hot-tags`) and the host renderer's overrides,
//! parameters and toggles (`render.*`, over `hot-render`'s hub). Names are
//! in `agent_proto::hot::method`.
//!
//! Like every host method, these run on the game's thread at a tick
//! boundary (or between slices), so a tag write lands between two ticks,
//! never inside one. The render methods only talk to the hub; the renderer
//! picks changes up at its next frame.
//!
//! Raw edits (`raw` and `evict` in `edit.batch`, `edit.raw.append`,
//! `edit.stage.put`, `edit.overlay`; docs/editor/realtime-edits.md) go
//! through the editor's `hot_tags::RawFiles`, which the game's glue
//! installs; bytes are base64 or `{stage, at, len}` naming staged bytes.
//!
//! Block moves (`moves` in `edit.batch`, `edit.arena`) copy a grown tag
//! block into the editor's `hot_tags::arena`, which the glue installs and
//! `world.placement.edit` shares.

#[cfg(feature = "render-overrides")]
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use agent_proto::hot::method;
use agent_proto::sim::SessionView;
use agent_proto::{RpcError, code};
#[cfg(feature = "render-overrides")]
use hot_render::{DumpRequest, Hub};
use hot_tags::{Editor, ErrorKind, TagSel};
use runtime::Guest;
use serde_json::Value;
use serde_json::json;

use crate::{Access, Agent, ConnId};

fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, m)
}

fn edit_error(e: hot_tags::Error) -> RpcError {
    let c = match e.kind {
        ErrorKind::Invalid => code::INVALID_PARAMS,
        ErrorKind::Unavailable => code::UNAVAILABLE,
    };
    RpcError::new(c, e.message)
}

fn object(p: &Value) -> Result<&serde_json::Map<String, Value>, RpcError> {
    static EMPTY: std::sync::OnceLock<serde_json::Map<String, Value>> = std::sync::OnceLock::new();
    match p {
        Value::Null => Ok(EMPTY.get_or_init(serde_json::Map::new)),
        Value::Object(o) => Ok(o),
        _ => Err(invalid("params must be an object")),
    }
}

fn opt_str<'a>(
    o: &'a serde_json::Map<String, Value>,
    k: &str,
) -> Result<Option<&'a str>, RpcError> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(other) => Err(invalid(format!("{k}: {other} is not a string"))),
    }
}

fn limit(o: &serde_json::Map<String, Value>, default: usize) -> Result<usize, RpcError> {
    match o.get("limit") {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| invalid("limit: a whole number")),
    }
}

fn tag_sel(o: &serde_json::Map<String, Value>) -> Result<TagSel, RpcError> {
    Ok(TagSel {
        tag: opt_str(o, "tag")?
            .ok_or_else(|| {
                invalid("tag: a datum (\"0xe1230045\") or a name (edit.tags lists them)")
            })?
            .to_owned(),
        group: opt_str(o, "group")?.map(str::to_owned),
    })
}

#[cfg(feature = "render-overrides")]
fn to_json<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(|e| RpcError::new(code::UNAVAILABLE, e.to_string()))
}

/// Most bytes staged at once, over every connection.
const STAGE_MAX: usize = 256 << 20;

/// Bytes `edit.stage.put` holds for a later request: each stage belongs to
/// the connection that made it and is dropped when a request that names
/// it applies, or when the map changes.
#[derive(Default)]
struct Stages {
    next: u64,
    bufs: std::collections::BTreeMap<u64, (ConnId, Vec<u8>)>,
}

impl Stages {
    fn total(&self) -> usize {
        self.bufs.values().map(|(_, b)| b.len()).sum()
    }

    fn put(&mut self, conn: ConnId, stage: Option<u64>, bytes: Vec<u8>) -> Result<Value, RpcError> {
        if self.total() + bytes.len() > STAGE_MAX {
            return Err(RpcError::new(
                code::INVALID_REQUEST,
                format!("staged bytes would pass {STAGE_MAX}: use a stage first"),
            ));
        }
        let id = match stage {
            Some(id) => {
                let (owner, buf) = self.bufs.get_mut(&id).ok_or_else(|| {
                    invalid(format!("stage {id}: no such stage (a request that names it uses it up)"))
                })?;
                if *owner != conn {
                    return Err(invalid(format!("stage {id} belongs to another connection")));
                }
                buf.extend_from_slice(&bytes);
                id
            }
            None => {
                self.next += 1;
                self.bufs.insert(self.next, (conn, bytes));
                self.next
            }
        };
        Ok(json!({"stage": id, "len": self.bufs[&id].1.len()}))
    }
}

/// Raw bytes of a request: base64, or `{stage, at, len}` (the stage's id
/// goes in `used`).
fn raw_bytes(v: &Value, conn: ConnId, stages: &Stages, used: &mut Vec<u64>) -> Result<Vec<u8>, RpcError> {
    match v {
        Value::String(s) => agent_proto::hot::decode_base64(s).map_err(invalid),
        Value::Object(o) => {
            let num = |k: &str| {
                o.get(k)
                    .and_then(Value::as_u64)
                    .ok_or_else(|| invalid(format!("bytes: {k} must be a whole number")))
            };
            let id = num("stage")?;
            let (owner, buf) = stages
                .bufs
                .get(&id)
                .ok_or_else(|| invalid(format!("stage {id}: no such stage")))?;
            if *owner != conn {
                return Err(invalid(format!("stage {id} belongs to another connection")));
            }
            let at = o.get("at").and_then(Value::as_u64).unwrap_or(0) as usize;
            let len = match o.get("len") {
                None | Some(Value::Null) => buf.len().saturating_sub(at),
                Some(_) => num("len")? as usize,
            };
            let bytes = buf.get(at..at.saturating_add(len)).ok_or_else(|| {
                invalid(format!("stage {id}: {at}+{len} is past its {} bytes", buf.len()))
            })?;
            used.push(id);
            Ok(bytes.to_vec())
        }
        _ => Err(invalid("bytes: base64, or {stage, at, len}")),
    }
}

/// `was` of a raw write: base64 bytes, or `{hash}` (FNV-1a 64, a number or
/// a hex string).
fn raw_was(v: &Value) -> Result<hot_tags::Was, RpcError> {
    let bad = || invalid("was: base64 bytes or {hash}");
    match v {
        Value::String(s) => agent_proto::hot::decode_base64(s)
            .map(hot_tags::Was::Bytes)
            .map_err(invalid),
        Value::Object(o) => match o.get("hash") {
            Some(Value::Number(n)) => n.as_u64().map(hot_tags::Was::Hash).ok_or_else(bad),
            Some(Value::String(h)) => u64::from_str_radix(h.trim_start_matches("0x"), 16)
                .map(hot_tags::Was::Hash)
                .map_err(|_| bad()),
            _ => Err(bad()),
        },
        _ => Err(bad()),
    }
}

/// A 32-bit address: a number or a `0x` string.
fn address(v: &Value, what: &str) -> Result<u32, RpcError> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| u32::from_str_radix(v.as_str()?.strip_prefix("0x")?, 16).ok())
        .ok_or_else(|| invalid(format!("{what} must be a 32-bit address (a number or a 0x string)")))
}

fn whole(v: &Value, what: &str) -> Result<u32, RpcError> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| invalid(format!("{what} must be a 32-bit unsigned integer")))
}

/// One `moves` entry: `{root: {tag, group?, path} | {at}, stride, count,
/// capacity?, bytes, was: {count, address}}`.
fn block_move(
    w: &Value,
    conn: ConnId,
    stages: &Stages,
    used: &mut Vec<u64>,
) -> Result<hot_tags::Move, RpcError> {
    let root = match w.get("root") {
        Some(Value::Object(r)) if r.contains_key("at") => {
            if r.contains_key("tag") || r.contains_key("path") {
                return Err(invalid("moves: root is {tag, path} or {at}"));
            }
            hot_tags::MoveRoot::At(address(&r["at"], "moves: root.at")?)
        }
        Some(Value::Object(r)) => hot_tags::MoveRoot::Path {
            tag: tag_sel(r)?,
            path: opt_str(r, "path")?
                .ok_or_else(|| invalid("moves: root.path names the block field"))?
                .to_owned(),
        },
        _ => return Err(invalid("moves: root is {tag, path} or {at}")),
    };
    let was = w
        .get("was")
        .filter(|v| v.is_object())
        .ok_or_else(|| invalid("moves: was is the root's {count, address} before the move"))?;
    let capacity = match w.get("capacity") {
        None | Some(Value::Null) => None,
        Some(v) => Some(whole(v, "moves: capacity")?),
    };
    Ok(hot_tags::Move {
        root,
        stride: whole(&w["stride"], "moves: stride")?,
        count: whole(&w["count"], "moves: count")?,
        capacity,
        bytes: raw_bytes(w.get("bytes").unwrap_or(&Value::Null), conn, stages, used)?,
        was: (whole(&was["count"], "moves: was.count")?, address(&was["address"], "moves: was.address")?),
    })
}

/// `raw`, `evict` and `moves` of an `edit.batch`.
fn raw_batch(
    p: &Value,
    conn: ConnId,
    stages: &Stages,
    used: &mut Vec<u64>,
) -> Result<hot_tags::RawBatch, RpcError> {
    let mut batch = hot_tags::RawBatch::default();
    match p.get("raw") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for w in list {
                let offset = w["offset"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| invalid("raw: offset must be a 32-bit unsigned integer"))?;
                let owner = w.get("owner").cloned().ok_or_else(|| invalid("raw: owner"))?;
                let bytes = raw_bytes(w.get("bytes").unwrap_or(&Value::Null), conn, stages, used)?;
                let was = match w.get("was") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(raw_was(v)?),
                };
                batch.raw.push(hot_tags::RawWrite { owner, offset, bytes, was });
            }
        }
        Some(_) => return Err(invalid("raw must be an array")),
    }
    match p.get("evict") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => batch.evict = list.clone(),
        Some(_) => return Err(invalid("evict must be an array")),
    }
    match p.get("moves") {
        None | Some(Value::Null) => {}
        Some(Value::Array(list)) => {
            for w in list {
                batch.moves.push(block_move(w, conn, stages, used)?);
            }
        }
        Some(_) => return Err(invalid("moves must be an array")),
    }
    Ok(batch)
}

/// Runs a function on the game's memory, locked for the call
/// (`|f| f(&mut runtime.memory())` in the game).
pub type WithGuest = Arc<dyn Fn(&mut dyn FnMut(&mut Guest)) + Send + Sync>;

fn with<T>(w: &WithGuest, f: impl FnOnce(&mut Guest) -> T) -> T {
    let mut f = Some(f);
    let mut out = None;
    w(&mut |g| {
        if let Some(f) = f.take() {
            out = Some(f(g));
        }
    });
    out.expect("the guest function ran")
}

pub(crate) fn permit(s: SessionView, p: &Value, epoch: u64) -> Result<(), RpcError> {
    if !s.writes {
        return Err(RpcError::new(
            code::INVALID_REQUEST,
            s.why.unwrap_or_default(),
        ));
    }
    check_epoch(p, epoch)?;
    if s.kind == agent_proto::sim::SessionKind::MpHost {
        let peers = s.peers.ok_or_else(|| {
            RpcError::new(code::INVALID_REQUEST, "the multiplayer peer count is unknown")
        })?;
        if peers > 1 && p["confirm_peers"].as_u64() != Some(u64::from(peers)) {
            return Err(RpcError::new(
                code::INVALID_REQUEST,
                "confirm the current peer count for host-only edits",
            ));
        }
    }
    Ok(())
}

pub(crate) fn check_epoch(p: &Value, epoch: u64) -> Result<(), RpcError> {
    if p.get("epoch").is_some_and(|v| v.as_u64() != Some(epoch)) {
        return Err(RpcError::new(code::INVALID_REQUEST, "the map epoch changed"));
    }
    Ok(())
}

/// Adds `edit.*`: `editor` reads and writes the guest through `guest`.
pub fn add_edit_methods(
    agent: &Agent,
    editor: Editor,
    guest: WithGuest,
    session: impl Fn(&Guest) -> SessionView + Send + Sync + 'static,
    file: impl Fn(&Guest) -> Option<String> + Send + 'static,
    replaying: bool,
) {
    let session = Arc::new(move |g: &Guest, ctx: &crate::GameCtx<'_>| {
        let mut s = session(g).settle();
        let why = if replaying {
            Some("Theater playback")
        } else if let Some(why) = crate::exec::live_edit_refusal(ctx.live_session, ctx.live_edit, g) {
            Some(why)
        } else if s.kind == agent_proto::sim::SessionKind::MpHost && s.peers.is_none() {
            Some("multiplayer host peer count is unknown")
        } else {
            None
        };
        if let Some(why) = why {
            s.writes = false;
            s.why = Some(why.into());
        }
        s
    });
    let editor = Arc::new(Mutex::new(editor));
    let lock = |e: &Arc<Mutex<Editor>>| -> Arc<Mutex<Editor>> { Arc::clone(e) };

    let (ed, g, probe) = (lock(&editor), guest.clone(), session.clone());
    let mut cached_file = None;
    agent.add_game_method(method::EDIT_MAP, Access::Read, move |p, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        let (_, checksum) = ctx.reader.live_metadata();
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            // A served map's file can be named after its first edit.map, so
            // keep asking until one is known for this epoch.
            if cached_file.as_ref().is_none_or(|(epoch, file): &(u64, Option<String>)| *epoch != ctx.epoch || file.is_none()) {
                cached_file = Some((ctx.epoch, file(g)));
            }
            let idx = hot_tags::index::index(g).map_err(|e| RpcError::new(code::UNAVAILABLE, e))?;
            let mut reply = json!({"at":ctx.at,"name":ctx.at.map,"file":cached_file.as_ref().and_then(|(_, file)| file.as_ref()),
                "checksum":checksum,"scenario":format!("{:#010x}",idx.scenario),"epoch":ctx.epoch,
                "session":probe(g,ctx)});
            if p.get("known_epoch").is_some_and(|e| e.as_u64() != Some(ctx.epoch)) {
                let entries = hot_tags::index::all(g,&idx).map_err(|e| RpcError::new(code::UNAVAILABLE,e))?;
                reply["loaded"] = json!(entries.into_iter().map(|e| e.datum).collect::<Vec<_>>());
            }
            Ok(reply)
        })
    });
    let stages = Arc::new(Mutex::new(Stages::default()));
    let (ed, g, probe, st) = (lock(&editor), guest.clone(), session.clone(), Arc::clone(&stages));
    agent.add_game_method(method::EDIT_BATCH, Access::Act, move |p, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        let mut stages = st.lock().unwrap_or_else(|e| e.into_inner());
        let mut used = Vec::new();
        let raw = raw_batch(&p, ctx.conn, &stages, &mut used)?;
        let empty = Vec::new();
        let writes = match p.get("writes") {
            None | Some(Value::Null)
                if !raw.raw.is_empty() || !raw.evict.is_empty() || !raw.moves.is_empty() =>
            {
                &empty
            }
            _ => p["writes"]
                .as_array()
                .ok_or_else(|| invalid("writes must be an array"))?,
        };
        let reply = with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            permit(probe(g, ctx), &p, ctx.epoch)?;
            ed.batch_with(g, writes, &raw, opt_str(object(&p)?, "label")?)
                .map_err(|e| match e {
                    hot_tags::BatchError::Field(e) => edit_error(e),
                    hot_tags::BatchError::Mismatch { fields, raw, moves } => RpcError {
                        code: code::MISMATCH,
                        message: "the game changed these fields, raw bytes or block roots".into(),
                        data: Some(json!({"fields":fields,"raw":raw,"moves":moves})),
                    },
                    hot_tags::BatchError::Full { root, need, largest_free, why } => RpcError {
                        code: agent_proto::sim::REFUSED,
                        message: format!("the edit arena is full: {why}"),
                        data: Some(json!({"why":why,"root":root,"need":need,
                            "largest_free":largest_free})),
                    },
                })
        })?;
        for id in used {
            stages.bufs.remove(&id);
        }
        Ok(reply)
    });
    let (ed, g, probe, st) = (lock(&editor), guest.clone(), session.clone(), Arc::clone(&stages));
    agent.add_game_method(method::EDIT_RAW_APPEND, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let file = hot_tags::raw::parse_file(opt_str(o, "file")?.unwrap_or("local"))
            .map_err(edit_error)?;
        let align = match o.get("align") {
            None | Some(Value::Null) => 16,
            Some(v) => v
                .as_u64()
                .filter(|a| a.is_power_of_two() && *a <= 1 << 20)
                .ok_or_else(|| invalid("align: a power of two up to 1 MiB"))?,
        };
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        let mut stages = st.lock().unwrap_or_else(|e| e.into_inner());
        let mut used = Vec::new();
        let bytes = raw_bytes(o.get("bytes").unwrap_or(&Value::Null), ctx.conn, &stages, &mut used)?;
        let reply = with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            permit(probe(g, ctx), &p, ctx.epoch)?;
            ed.raw_append(g, file, &bytes, align).map_err(edit_error)
        })?;
        for id in used {
            stages.bufs.remove(&id);
        }
        Ok(reply)
    });
    let st = Arc::clone(&stages);
    agent.add_game_method(method::EDIT_STAGE_PUT, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let stage = match o.get("stage") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_u64().ok_or_else(|| invalid("stage: a stage id"))?),
        };
        let bytes = match o.get("bytes") {
            Some(Value::String(s)) => agent_proto::hot::decode_base64(s).map_err(invalid)?,
            _ => return Err(invalid("bytes: base64")),
        };
        st.lock().unwrap_or_else(|e| e.into_inner()).put(ctx.conn, stage, bytes)
    });
    let (ed, g) = (lock(&editor), guest.clone());
    agent.add_game_method(method::EDIT_ARENA, Access::Read, move |_, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch)?;
            ed.arena(g)
        })
        .map_err(edit_error)
    });
    let (ed, g) = (lock(&editor), guest.clone());
    agent.add_game_method(method::EDIT_OVERLAY, Access::Read, move |_, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            Ok(ed.overlay())
        })
    });
    // A new map drops the overlay, the raw journal and the stages, and
    // releases the edit arena, at its first tick rather than at the next
    // edit call: the old offsets and blocks mean nothing in it.
    let (ed, g, st) = (lock(&editor), guest.clone(), Arc::clone(&stages));
    let mut seen = None;
    agent.add_ticker("edit.overlay", move |ctx| {
        if seen == Some(ctx.map_epoch) {
            return;
        }
        if seen.is_some() {
            st.lock().unwrap_or_else(|e| e.into_inner()).bufs.clear();
        }
        seen = Some(ctx.map_epoch);
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            let _ = ed.at_epoch(g, ctx.epoch, ctx.map_epoch);
            ed.bind_arena(g);
        });
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_game_method(method::EDIT_READ, Access::Read, move |p, ctx| {
        let o = object(&p)?;
        if !o.contains_key("epoch") {
            return Err(invalid("epoch: the epoch from edit.map"));
        }
        check_epoch(&p, ctx.epoch)?;
        let number = |key: &str| {
            o.get(key)
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid(format!("{key} must be a 32-bit unsigned integer")))
        };
        let (at, len) = (number("at")?, number("len")?);
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch)?;
            ed.read(g, at, len)
        })
        .map_err(edit_error)
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_method(method::EDIT_TAGS, Access::Read, move |p| {
        let o = object(&p)?;
        let (group, name, limit) = (opt_str(o, "group")?, opt_str(o, "name")?, limit(o, 200)?);
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| ed.tags(g, group, name, limit)).map_err(edit_error)
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_game_method(method::EDIT_GET, Access::Read, move |p, ctx| {
        check_epoch(&p, ctx.epoch)?;
        let o = object(&p)?;
        let sel = tag_sel(o)?;
        let path = opt_str(o, "path")?.unwrap_or("");
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch)?;
            ed.get(g, &sel, path)
        })
        .map_err(edit_error)
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    let probe = session.clone();
    agent.add_game_method(method::EDIT_SET, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let sel = tag_sel(o)?;
        let path = opt_str(o, "path")?.ok_or_else(|| invalid("path: the field to set"))?;
        let value = o
            .get("value")
            .ok_or_else(|| invalid("value: what to set it to"))?;
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            permit(probe(g, ctx), &p, ctx.epoch)?;
            ed.set(g, &sel, path, value).map_err(edit_error)
        })
    });
    let (ed, g) = (lock(&editor), Arc::clone(&guest));
    agent.add_game_method(method::EDIT_CHANGES, Access::Read, move |_, ctx| {
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            Ok(ed.changes(g))
        })
    });
    let (ed, g) = (lock(&editor), guest);
    agent.add_game_method(method::EDIT_REVERT, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let id = match o.get("id") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or_else(|| invalid("id: a change id (edit.changes)"))?,
            ),
        };
        let mut ed = ed.lock().unwrap_or_else(|e| e.into_inner());
        with(&g, |g| {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            permit(session(g, ctx), &p, ctx.epoch)?;
            ed.revert(g, id).map_err(edit_error)
        })
    });
}

#[cfg(feature = "render-overrides")]
/// Adds `render.*` over `hub` (`hot_render::global()` in the game).
pub fn add_render_methods(agent: &Agent, hub: &'static Hub) {
    agent.add_method(method::RENDER_OVERRIDES, Access::Read, move |_| {
        to_json(hub.status())
    });
    agent.add_method(method::RENDER_RELOAD, Access::Act, move |p| {
        let o = object(&p)?;
        if let Some(dir) = opt_str(o, "dir")? {
            let dir = PathBuf::from(dir);
            std::fs::create_dir_all(&dir)
                .map_err(|e| invalid(format!("{}: {e}", dir.display())))?;
            hub.set_dir(Some(dir));
        }
        to_json(hub.reload())
    });
    agent.add_method(method::RENDER_PROGRAMS, Access::Read, move |p| {
        let o = object(&p)?;
        let vertex = opt_str(o, "vertex")?;
        let limit = limit(o, 500)?;
        let status = hub.status();
        let mut total = 0;
        let mut out = Vec::new();
        for (id, info) in hub.programs() {
            if vertex.is_some_and(|v| v != info.vertex) {
                continue;
            }
            total += 1;
            if out.len() < limit {
                let o = status.overrides.iter().find(|o| o.program == id);
                let mut v = to_json(&info)?;
                v["program"] = json!(id);
                v["override"] = match o {
                    Some(o) => to_json(&o.applied)?,
                    None => Value::Null,
                };
                out.push(v);
            }
        }
        Ok(json!({ "total": total, "programs": out }))
    });
    agent.add_method(method::RENDER_DUMP, Access::Act, move |p| {
        let o = object(&p)?;
        let program = match opt_str(o, "program")? {
            None | Some("all") => None,
            Some(s) => {
                let id = hot_render::parse_program(s)
                    .ok_or_else(|| invalid(format!("program: {s:?} is not 16 hex digits")))?;
                if !hub.has_program(id) {
                    return Err(invalid(format!(
                        "program {s}: not translated (render.programs lists them)"
                    )));
                }
                Some(id)
            }
        };
        let to = match opt_str(o, "to")? {
            Some(d) => PathBuf::from(d),
            None => hub
                .dir()
                .map(|d| d.join("translated"))
                .ok_or_else(|| invalid("to: where to write (no override directory is set)"))?,
        };
        hub.request_dump(DumpRequest {
            program,
            to: to.clone(),
            tries: 0,
        });
        Ok(json!({
            "queued": true,
            "program": program.map(hot_render::program_text),
            "to": to.display().to_string(),
            "note": "written at the next frame; render.overrides shows last_dump",
        }))
    });
    agent.add_method(method::RENDER_PARAM, Access::Act, move |p| {
        let o = object(&p)?;
        let Some(name) = opt_str(o, "name")? else {
            return to_json(json!({ "params": hub.params() }));
        };
        match o.get("value") {
            None => hub
                .params()
                .into_iter()
                .find(|p| p.name == name)
                .ok_or_else(|| invalid(format!("no parameter {name:?}")))
                .and_then(to_json),
            Some(Value::Null) => hub.set_param(name, None).map_err(invalid).and_then(to_json),
            Some(v) => {
                let v = v
                    .as_f64()
                    .ok_or_else(|| invalid("value: a number (or null to reset)"))?;
                hub.set_param(name, Some(v as f32))
                    .map_err(invalid)
                    .and_then(to_json)
            }
        }
    });
    agent.add_method(method::RENDER_TOGGLE, Access::Act, move |p| {
        let o = object(&p)?;
        let Some(name) = opt_str(o, "name")? else {
            return to_json(json!({ "toggles": hub.toggles() }));
        };
        match o.get("on") {
            None => hub
                .toggles()
                .into_iter()
                .find(|t| t.name == name)
                .ok_or_else(|| invalid(format!("no toggle {name:?}")))
                .and_then(to_json),
            Some(Value::Null) => hub
                .set_toggle(name, None)
                .map_err(invalid)
                .and_then(to_json),
            Some(Value::Bool(on)) => hub
                .set_toggle(name, Some(*on))
                .map_err(invalid)
                .and_then(to_json),
            Some(other) => Err(invalid(format!("on: {other} is not true, false or null"))),
        }
    });
}

#[cfg(feature = "render-overrides")]
/// Points `hub` at `dir`, loads it and watches it (a quarter-second look).
/// Returns the first load's report.
pub fn watch_overrides(hub: &'static Hub, dir: PathBuf) -> hot_render::ReloadReport {
    let _ = std::fs::create_dir_all(&dir);
    hub.set_dir(Some(dir));
    let r = hub.reload();
    if let Err(e) = hot_render::watch(hub, std::time::Duration::from_millis(250)) {
        eprintln!("render overrides: not watching: {e}");
    }
    r
}

#[cfg(test)]
mod tests;
