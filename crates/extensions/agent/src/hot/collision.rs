//! Collision live: `edit.collision.stage` and `edit.collision.commit`
//! (docs/editor/realtime-edits.md, "Collision"; docs/runtime/agent-api.md,
//! "Collision edits").
//!
//! The lifted game reads collision in place, through each BSP's eight
//! `(count, address)` pairs, so a live collision edit is a pointer swap:
//!
//! 1. **Stage** writes a BSP's new blocks (and a structure's per-leaf
//!    block and MOPP) into new edit arena extents (`hot_tags::arena`).
//!    Nothing points at them, so a stage touches no byte the game reads.
//! 2. **Commit** checks every pair's `was`, then writes them all at one
//!    tick boundary, or none. With `activate` (always for `rebuild`) it
//!    then runs the game's BSP activation ([`Activation`]) and answers
//!    after it, so Havok's structure shape is rebuilt over the new blocks.
//! 3. The arena extents the old pairs named are freed after the next
//!    activation: until then Havok's shape may still read them. A `coll`
//!    BSP's are never freed before the map changes, since a live body's
//!    shape points at them until the object is made again.
//!
//! A structure commit also hands the new BSP to the debug views
//! ([`crate::debug_views::CollisionEdits`]), whose collision lines are
//! built from the map file and would otherwise show the old level.
//!
//! The bots' line-of-sight walls (`bots::los::Walls`, built offline per
//! map and BSP) are not told: docs/editor/realtime-edits.md, "Not
//! verified".

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::hot::{COLLISION_BLOCKS, COLLISION_LEAF_CLUSTERS, COLLISION_MOPP, method};
use agent_proto::sim::{REFUSED, SessionView};
use agent_proto::{RpcError, code};
use hot_tags::Editor;
use hot_tags::arena::{self, Claim};
use runtime::Guest;
use serde_json::{Value, json};
use tags::collision::{BSP_BLOCKS_SIZE, BSP_STRIDES, Bsp};

use super::{Stages, WithGuest, address, edit_error, invalid, object, permit, raw_bytes, whole, with};
use crate::{Access, Agent, ConnId};

/// A structure BSP's fields (docs/formats/structure-bsp.md,
/// `tags::pathfinding`, `geometry_edit::level_collision`).
pub mod sbsp {
    /// The collision material block: surfaces index it.
    pub const MATERIALS: u32 = 0x0c;
    /// A block of one collision BSP: eight `(count, address)` pairs.
    pub const COLLISION: u32 = 0x14;
    /// The per-leaf block, [`LEAF_SIZE`] bytes a leaf.
    pub const LEAVES: u32 = 0x2c;
    pub const LEAF_SIZE: u32 = 8;
    /// The pathfinding data, which names collision surfaces by index.
    pub const PATHFINDING: u32 = 0xc4;
    /// The instanced geometry definitions, [`INSTANCE_DEF_SIZE`] bytes each;
    /// a definition's eight pairs are at [`INSTANCE_DEF_COLLISION`].
    pub const INSTANCE_DEFS: u32 = 0x138;
    pub const INSTANCE_DEF_SIZE: u32 = 0xc8;
    pub const INSTANCE_DEF_COLLISION: u32 = 0x70;
    /// The structure MOPP's byte block.
    pub const MOPP: u32 = 0x1e0;
}

/// A collision model's fields (`tags::collision::Collision::decode_at`).
pub mod coll {
    pub const MATERIALS: u32 = 0x14;
    /// Regions, [`REGION_SIZE`] bytes each, permutations at `+4`.
    pub const REGIONS: u32 = 0x1c;
    pub const REGION_SIZE: u32 = 12;
    /// Permutations, [`PERMUTATION_SIZE`] bytes each, BSPs at `+4`.
    pub const PERMUTATION_SIZE: u32 = 0x14;
    /// A BSP element: a leading word, then the eight pairs.
    pub const BSP_SIZE: u32 = 0x44;
}

/// The game's BSP activation: a BSP switch to the active BSP
/// (`BSP_SWITCH`, `scenario_switch_structure_bsp`), which rebuilds the
/// Havok structure over the collision the pairs name.
pub trait Activation: Send {
    /// Asks for the switch; the game runs it at its next tick.
    fn start(&mut self, g: &mut Guest) -> Result<(), String>;
    /// `None` until the switch has run, then how it went.
    fn poll(&mut self, g: &Guest) -> Option<Result<(), String>>;
}

/// Where the game's [`Activation`] is installed
/// ([`Agent::set_collision_activation`]).
pub type Activator = Arc<Mutex<Option<Box<dyn Activation>>>>;

/// How long a commit waits for its activation.
const ACTIVATION_TICKS: u64 = 120;

/// Which BSP: `{structure: tag}`, `{instance_def: {tag, index}}` or
/// `{coll: {tag, region, permutation, bsp?}}`.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Target {
    Structure(String),
    InstanceDef { tag: String, index: u32 },
    Coll { tag: String, region: u32, permutation: u32, bsp: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Structure,
    InstanceDef,
    Coll,
}

/// A pair a commit can repoint: its name in `was`, its address, and the
/// size of its elements.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Slot {
    name: &'static str,
    root: u32,
    stride: u32,
}

/// Where a target's pairs are now.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Place {
    kind: Kind,
    /// The owning tag's datum.
    tag: u32,
    /// The owning tag's address.
    root: u32,
    /// The eight BSP pairs, then for a structure the per-leaf block's and
    /// the MOPP's.
    slots: Vec<Slot>,
    /// Collision materials the surfaces index.
    materials: u32,
}

/// One staged block: the slot it is for, its count and its address (0
/// for an empty block).
#[derive(Clone, Debug)]
struct New {
    slot: usize,
    count: u32,
    address: u32,
}

/// A stage not yet committed.
struct Staged {
    conn: ConnId,
    map_epoch: u64,
    target: Target,
    place: Place,
    new: Vec<New>,
    /// The arena extents the stage holds.
    extents: Vec<u32>,
    bsp: Bsp,
}

/// A commit waiting for its activation.
struct Activating {
    reply: Value,
    place: Place,
    /// The pairs before the commit, to put back if the switch fails.
    old: Vec<(u32, (u32, u32))>,
    /// The stage's extents, freed if the switch fails.
    extents: Vec<u32>,
    /// What the old pairs named, retired once the switch has run.
    retire: Vec<u32>,
    bsp: Bsp,
}

#[derive(Default)]
struct State {
    next: u64,
    staged: BTreeMap<u64, Staged>,
    /// Arena extents old pairs named: freed after the next activation.
    retired: Vec<u32>,
    activating: Option<Activating>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn refused(why: impl Into<String>, mut data: Value) -> RpcError {
    let why = why.into();
    data["why"] = json!(why);
    RpcError {
        code: REFUSED,
        message: why,
        data: Some(data),
    }
}

fn word(g: &Guest, at: u32) -> Result<u32, RpcError> {
    g.u32(at)
        .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("guest fault at {:#010x}", e.addr)))
}

fn pair_at(g: &Guest, at: u32) -> Result<(u32, u32), RpcError> {
    Ok((word(g, at)?, word(g, at + 4)?))
}

fn pair_bytes(count: u32, address: u32) -> [u8; 8] {
    let mut b = [0u8; 8];
    b[..4].copy_from_slice(&count.to_le_bytes());
    b[4..].copy_from_slice(&address.to_le_bytes());
    b
}

fn write(g: &mut Guest, at: u32, bytes: &[u8]) -> Result<(), RpcError> {
    g.write_bytes(at, bytes, 1)
        .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("guest fault at {:#010x}", e.addr)))
}

fn tag_text(v: Option<&Value>, what: &str) -> Result<String, RpcError> {
    match v {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(format!("{:#010x}", n.as_u64().unwrap_or(0))),
        _ => Err(invalid(format!("{what}: a tag datum or name"))),
    }
}

fn index_of(o: &Value, key: &str, default: Option<u32>) -> Result<u32, RpcError> {
    match (o.get(key), default) {
        (None | Some(Value::Null), Some(d)) => Ok(d),
        (Some(v), _) => whole(v, &format!("target: {key}")),
        (None | Some(Value::Null), None) => Err(invalid(format!("target: {key} is required"))),
    }
}

fn target(v: Option<&Value>) -> Result<Target, RpcError> {
    let bad = || invalid("target: {structure: tag}, {instance_def: {tag, index}} or {coll: {tag, region, permutation, bsp?}}");
    let o = v.and_then(Value::as_object).ok_or_else(bad)?;
    if let Some(t) = o.get("structure") {
        return Ok(Target::Structure(tag_text(Some(t), "target: structure")?));
    }
    if let Some(d) = o.get("instance_def") {
        return Ok(Target::InstanceDef {
            tag: tag_text(d.get("tag"), "target: instance_def.tag")?,
            index: index_of(d, "index", None)?,
        });
    }
    if let Some(c) = o.get("coll") {
        return Ok(Target::Coll {
            tag: tag_text(c.get("tag"), "target: coll.tag")?,
            region: index_of(c, "region", None)?,
            permutation: index_of(c, "permutation", None)?,
            bsp: index_of(c, "bsp", Some(0))?,
        });
    }
    Err(bad())
}

/// The element `i` of the block whose pair is at `at`.
fn element(g: &Guest, at: u32, stride: u32, i: u32, what: &str) -> Result<u32, RpcError> {
    let (count, address) = pair_at(g, at)?;
    if i >= count {
        return Err(invalid(format!("target: {what} {i} is past the block's {count}")));
    }
    Ok(address + i * stride)
}

fn bsp_slots(at: u32) -> Vec<Slot> {
    COLLISION_BLOCKS
        .iter()
        .zip(BSP_STRIDES)
        .enumerate()
        .map(|(i, (&name, stride))| Slot {
            name,
            root: at + 8 * i as u32,
            stride: stride as u32,
        })
        .collect()
}

/// Where `t`'s pairs are in the guest now.
fn resolve(ed: &mut Editor, g: &Guest, t: &Target) -> Result<Place, RpcError> {
    let find = |ed: &mut Editor, tag: &str, group: &str| -> Result<hot_tags::index::Entry, RpcError> {
        let (e, _) = ed.find_tag_in(g, tag, &[group]).map_err(edit_error)?;
        if tags::defs::fourcc_text(e.group) != group {
            return Err(invalid(format!(
                "target: {tag} is a {} tag, not {group}",
                tags::defs::fourcc_text(e.group)
            )));
        }
        Ok(e)
    };
    match t {
        Target::Structure(tag) => {
            let e = find(ed, tag, "sbsp")?;
            let (n, at) = pair_at(g, e.address + sbsp::COLLISION)?;
            if n != 1 {
                return Err(refused(
                    format!("the structure has {n} collision BSPs, not one"),
                    json!({}),
                ));
            }
            let mut slots = bsp_slots(at);
            slots.push(Slot {
                name: COLLISION_LEAF_CLUSTERS,
                root: e.address + sbsp::LEAVES,
                stride: sbsp::LEAF_SIZE,
            });
            slots.push(Slot {
                name: COLLISION_MOPP,
                root: e.address + sbsp::MOPP,
                stride: 1,
            });
            Ok(Place {
                kind: Kind::Structure,
                tag: e.datum,
                root: e.address,
                slots,
                materials: word(g, e.address + sbsp::MATERIALS)?,
            })
        }
        Target::InstanceDef { tag, index } => {
            let e = find(ed, tag, "sbsp")?;
            let def = element(g, e.address + sbsp::INSTANCE_DEFS, sbsp::INSTANCE_DEF_SIZE, *index, "instance_def")?;
            Ok(Place {
                kind: Kind::InstanceDef,
                tag: e.datum,
                root: e.address,
                slots: bsp_slots(def + sbsp::INSTANCE_DEF_COLLISION),
                materials: word(g, e.address + sbsp::MATERIALS)?,
            })
        }
        Target::Coll { tag, region, permutation, bsp } => {
            let e = find(ed, tag, "coll")?;
            let r = element(g, e.address + coll::REGIONS, coll::REGION_SIZE, *region, "region")?;
            let p = element(g, r + 4, coll::PERMUTATION_SIZE, *permutation, "permutation")?;
            let b = element(g, p + 4, coll::BSP_SIZE, *bsp, "bsp")?;
            Ok(Place {
                kind: Kind::Coll,
                tag: e.datum,
                root: e.address,
                slots: bsp_slots(b + 4),
                materials: word(g, e.address + coll::MATERIALS)?,
            })
        }
    }
}

/// One `{count, bytes}` of the request, its bytes `stride * count`.
fn counted(
    v: &Value,
    what: &str,
    stride: u32,
    conn: ConnId,
    stages: &Stages,
    used: &mut Vec<u64>,
) -> Result<(u32, Vec<u8>), RpcError> {
    let count = whole(v.get("count").unwrap_or(&Value::Null), &format!("{what}.count"))?;
    let bytes = raw_bytes(v.get("bytes").unwrap_or(&Value::Null), conn, stages, used)?;
    if bytes.len() as u64 != u64::from(count) * u64::from(stride) {
        return Err(invalid(format!(
            "{what}: {} bytes is not count {count} times {stride}",
            bytes.len()
        )));
    }
    Ok((count, bytes))
}

/// The request's blocks, by slot of a place with `slots` slots.
struct Blocks {
    /// The eight BSP blocks, then the per-leaf block and the MOPP when
    /// given.
    blocks: Vec<(usize, u32, Vec<u8>)>,
    bsp: Bsp,
}

fn blocks(
    o: &serde_json::Map<String, Value>,
    structure: bool,
    conn: ConnId,
    stages: &Stages,
    used: &mut Vec<u64>,
) -> Result<Blocks, RpcError> {
    let given = o
        .get("blocks")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("blocks: the eight collision blocks by name, each {count, bytes}"))?;
    if let Some(other) = given.keys().find(|k| !COLLISION_BLOCKS.contains(&k.as_str())) {
        return Err(invalid(format!("blocks: {other:?} is not a collision block ({})", COLLISION_BLOCKS.join(", "))));
    }
    let mut out = Vec::new();
    // The eight blocks gathered into one payload the codec reads.
    const BASE: u32 = 0x1000_0000;
    let mut joined = vec![0u8; BSP_BLOCKS_SIZE];
    for (i, name) in COLLISION_BLOCKS.iter().enumerate() {
        let v = given
            .get(*name)
            .ok_or_else(|| invalid(format!("blocks: {name} is missing (a stage replaces all eight)")))?;
        let (count, bytes) = counted(v, &format!("blocks.{name}"), BSP_STRIDES[i] as u32, conn, stages, used)?;
        let here = if count == 0 { 0 } else { BASE + joined.len() as u32 };
        joined[i * 8..i * 8 + 4].copy_from_slice(&count.to_le_bytes());
        joined[i * 8 + 4..i * 8 + 8].copy_from_slice(&here.to_le_bytes());
        joined.extend_from_slice(&bytes);
        out.push((i, count, bytes));
    }
    let bsp = Bsp::decode_blocks(&joined, BASE, 0)
        .map_err(|e| invalid(format!("blocks: they do not decode as a collision BSP: {e}")))?;
    for (key, slot) in [("leaves", 8usize), ("mopp", 9)] {
        match o.get(key) {
            None | Some(Value::Null) => {}
            Some(_) if !structure => {
                return Err(invalid(format!("{key}: only a structure target has one")));
            }
            Some(v) if key == "mopp" => {
                let bytes = raw_bytes(v, conn, stages, used)?;
                out.push((slot, bytes.len() as u32, bytes));
            }
            Some(v) => {
                let (count, bytes) = counted(v, key, sbsp::LEAF_SIZE, conn, stages, used)?;
                if count as usize != bsp.leaves.len() {
                    return Err(invalid(format!(
                        "leaves: {count} elements for the BSP's {} leaves",
                        bsp.leaves.len()
                    )));
                }
                out.push((slot, count, bytes));
            }
        }
    }
    Ok(Blocks { blocks: out, bsp })
}

/// Adds `edit.collision.stage` and `edit.collision.commit` beside the
/// other `edit.*` methods, sharing their editor, guest, session check and
/// staged bytes.
pub(super) fn add_methods<S>(
    agent: &Agent,
    editor: &Arc<Mutex<Editor>>,
    guest: &WithGuest,
    session: &Arc<S>,
    stages: &Arc<Mutex<Stages>>,
) where
    S: Fn(&Guest, &crate::GameCtx<'_>) -> SessionView + Send + Sync + 'static,
{
    let state = Arc::new(Mutex::new(State::default()));
    let activator = agent.collision_activator();
    let edits = agent.collision_edits();

    let (ed, g, probe, st, me, act) = (
        Arc::clone(editor),
        Arc::clone(guest),
        Arc::clone(session),
        Arc::clone(stages),
        Arc::clone(&state),
        Arc::clone(&activator),
    );
    agent.add_game_method(method::EDIT_COLLISION_STAGE, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let target = target(o.get("target"))?;
        if o.get("pathfinding").is_some_and(|v| !v.is_null()) {
            return Err(refused(
                "staging pathfinding is not built: its blocks hold absolute pointers the host would have to relocate",
                json!({}),
            ));
        }
        let mut ed = lock(&ed);
        let mut stages = lock(&st);
        let mut me = lock(&me);
        let mut used = Vec::new();
        let structure = matches!(target, Target::Structure(_));
        let req = blocks(o, structure, ctx.conn, &stages, &mut used)?;
        let activation = lock(&act).is_some();
        let reply = with(&g, |g| -> Result<Value, RpcError> {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            permit(probe(g, ctx), &p, ctx.epoch)?;
            let place = resolve(&mut ed, g, &target)?;
            if let Some(problem) = req.bsp.validate(place.materials as usize).into_iter().next() {
                return Err(invalid(format!("blocks: the BSP does not validate: {problem}")));
            }
            if place.kind == Kind::Structure && !req.blocks.iter().any(|b| b.0 == 8) {
                let (held, _) = pair_at(g, place.slots[8].root)?;
                if held as usize != req.bsp.leaves.len() {
                    return Err(invalid(format!(
                        "leaves: the structure's per-leaf block holds {held} elements and the BSP has {} leaves: \
                         stage the per-leaf block too",
                        req.bsp.leaves.len()
                    )));
                }
            }
            let shared = ed
                .shared_arena()
                .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "the edit arena is not installed in this game"))?;
            ed.bind_arena(g);
            // A stage of the same target that was never committed is
            // replaced: its extents go back now.
            let stale: Vec<u64> = me
                .staged
                .iter()
                .filter(|(_, s)| s.place.slots[0].root == place.slots[0].root)
                .map(|(id, _)| *id)
                .collect();
            let mut arena = arena::lock(&shared);
            for id in stale {
                if let Some(s) = me.staged.remove(&id) {
                    for at in s.extents {
                        arena.free(at);
                    }
                }
            }
            let mut new = Vec::new();
            let mut extents = Vec::new();
            let mut total = 0usize;
            for (slot, count, bytes) in &req.blocks {
                if *count == 0 {
                    new.push(New { slot: *slot, count: 0, address: 0 });
                    continue;
                }
                let s = &place.slots[*slot];
                let claim = Claim { tag: place.tag, root: s.root, stride: s.stride, capacity: *count };
                let e = match arena.alloc(g, claim) {
                    Ok(e) => e,
                    Err(full) => {
                        for at in &extents {
                            arena.free(*at);
                        }
                        return Err(refused(
                            format!("the edit arena is full: {}", full.why),
                            json!({"block": s.name, "need": full.need, "largest_free": full.largest_free}),
                        ));
                    }
                };
                extents.push(e.at);
                let mut padded = bytes.clone();
                padded.resize(e.len as usize, 0);
                if let Err(err) = write(g, e.at, &padded) {
                    for at in &extents {
                        arena.free(*at);
                    }
                    return Err(err);
                }
                total += bytes.len();
                new.push(New { slot: *slot, count: *count, address: e.at });
            }
            let summary = json!({"used": arena.used(), "largest_free": arena.largest_free(g)});
            drop(arena);
            me.next += 1;
            let id = me.next;
            let listed: serde_json::Map<String, Value> = new
                .iter()
                .map(|n| {
                    (place.slots[n.slot].name.to_owned(), json!({"count": n.count, "address": n.address}))
                })
                .collect();
            me.staged.insert(
                id,
                Staged {
                    conn: ctx.conn,
                    map_epoch: ctx.map_epoch,
                    target: target.clone(),
                    place,
                    new,
                    extents,
                    bsp: req.bsp.clone(),
                },
            );
            Ok(json!({"stage": id, "bytes": total, "blocks": listed, "arena": summary,
                "activation": activation}))
        })?;
        for id in used {
            stages.bufs.remove(&id);
        }
        Ok(reply)
    });

    let (ed, g, probe, state_c, act, ed_edits) = (
        Arc::clone(editor),
        Arc::clone(guest),
        Arc::clone(session),
        Arc::clone(&state),
        Arc::clone(&activator),
        Arc::clone(&edits),
    );
    agent.add_game_method(method::EDIT_COLLISION_COMMIT, Access::Act, move |p, ctx| {
        let o = object(&p)?;
        let id = o
            .get("stage")
            .and_then(Value::as_u64)
            .ok_or_else(|| invalid("stage: the id edit.collision.stage returned"))?;
        let rebuild = match o.get("mode").and_then(Value::as_str) {
            Some("append") => false,
            Some("rebuild") => true,
            _ => return Err(invalid("mode: append or rebuild")),
        };
        let activate = rebuild
            || match o.get("activate") {
                None | Some(Value::Null) => false,
                Some(Value::Bool(b)) => *b,
                Some(_) => return Err(invalid("activate: true or false")),
            };
        let was = o
            .get("was")
            .and_then(Value::as_object)
            .ok_or_else(|| invalid("was: each repointed pair's {count, address} before the commit, by name"))?;
        let mut ed = lock(&ed);
        let mut me = lock(&state_c);
        let shared = ed
            .shared_arena()
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "the edit arena is not installed in this game"))?;
        let done = with(&g, |g| -> Result<Value, RpcError> {
            ed.at_epoch(g, ctx.epoch, ctx.map_epoch).map_err(edit_error)?;
            permit(probe(g, ctx), &p, ctx.epoch)?;
            if me.activating.is_some() {
                return Err(refused("a collision commit's BSP activation is still running", json!({})));
            }
            let staged = me
                .staged
                .get(&id)
                .ok_or_else(|| invalid(format!("stage {id}: no such collision stage (a commit uses it up)")))?;
            if staged.conn != ctx.conn {
                return Err(invalid(format!("stage {id} belongs to another connection")));
            }
            if staged.map_epoch != ctx.map_epoch {
                return Err(refused("the map changed since the stage", json!({"stage": id})));
            }
            let place = resolve(&mut ed, g, &staged.target)?;
            if place != staged.place {
                return Err(refused("the target's pairs moved since the stage: stage again", json!({"stage": id})));
            }
            if activate && lock(&act).is_none() {
                return Err(refused(
                    "this game has no BSP activation: commit append without activate",
                    json!({"stage": id, "mode": if rebuild { "rebuild" } else { "append" }}),
                ));
            }
            if rebuild && place.kind == Kind::Structure {
                let (paths, _) = pair_at(g, place.root + sbsp::PATHFINDING)?;
                if paths > 0 {
                    return Err(refused(
                        "rebuild changes surface indices and the structure's pathfinding names them; \
                         the stage did not replace it",
                        json!({"stage": id, "pathfinding": paths}),
                    ));
                }
            }
            let mut old = Vec::new();
            let mut mismatches = Vec::new();
            for n in &staged.new {
                let s = &place.slots[n.slot];
                let holds = pair_at(g, s.root)?;
                let w = was.get(s.name).ok_or_else(|| {
                    invalid(format!("was.{}: the pair's {{count, address}} before the commit", s.name))
                })?;
                let expected = (
                    whole(w.get("count").unwrap_or(&Value::Null), &format!("was.{}.count", s.name))?,
                    address(w.get("address").unwrap_or(&Value::Null), &format!("was.{}.address", s.name))?,
                );
                if holds != expected {
                    mismatches.push(json!({"name": s.name, "root": s.root,
                        "was": {"count": expected.0, "address": expected.1},
                        "holds": {"count": holds.0, "address": holds.1}}));
                }
                // `append` keeps every index: no counted thing may shrink.
                if !rebuild
                    && matches!(s.name, "planes" | "leaves" | "surfaces" | "edges" | "vertices" | "leaf_clusters")
                    && n.count < holds.0
                {
                    return Err(refused(
                        format!(
                            "append keeps every existing index, but {} would hold {} elements, fewer than its {}",
                            s.name, n.count, holds.0
                        ),
                        json!({"stage": id, "block": s.name}),
                    ));
                }
                old.push((s.root, holds));
            }
            if !mismatches.is_empty() {
                return Err(RpcError {
                    code: code::MISMATCH,
                    message: "the game's collision pairs are not the ones the commit was made against".into(),
                    data: Some(json!({"pairs": mismatches})),
                });
            }
            for n in &staged.new {
                g.check(place.slots[n.slot].root, 8, runtime::guest::Access::Write, 1)
                    .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("guest fault at {:#010x}", e.addr)))?;
            }
            // Every check is done: the pairs, all at this boundary.
            let staged = me.staged.remove(&id).expect("checked above");
            let mut pairs = serde_json::Map::new();
            for n in &staged.new {
                let s = &place.slots[n.slot];
                write(g, s.root, &pair_bytes(n.count, n.address))?;
                pairs.insert(s.name.to_owned(), json!({"count": n.count, "address": n.address}));
            }
            // What the old pairs named in the arena is free once no shape
            // reads it.
            let retire: Vec<u32> = {
                let arena = arena::lock(&shared);
                old.iter()
                    .filter_map(|(root, (_, at))| arena.extent(*at).filter(|e| e.root == *root).map(|e| e.at))
                    .collect()
            };
            let mode = if rebuild { "rebuild" } else { "append" };
            let reply = json!({"at": ctx.at, "stage": id, "mode": mode, "pairs": pairs});
            if !activate {
                if place.kind != Kind::Coll {
                    me.retired.extend(retire);
                }
                if place.kind == Kind::Structure {
                    ed_edits.replace(place.tag, staged.bsp);
                }
                let mut reply = reply;
                reply["activated"] = json!(false);
                reply["freed"] = json!([]);
                return Ok(reply);
            }
            let started = match lock(&act).as_mut() {
                Some(a) => a.start(g),
                None => Err("this game has no BSP activation".to_owned()),
            };
            if let Err(why) = started {
                undo(g, &shared, &old, &staged.extents);
                return Err(refused(
                    format!("the BSP activation did not start ({why}); the pairs are put back"),
                    json!({"stage": id}),
                ));
            }
            me.activating = Some(Activating {
                reply,
                place,
                old,
                extents: staged.extents,
                retire,
                bsp: staged.bsp,
            });
            Ok(Value::Null)
        })?;
        drop(me);
        drop(ed);
        if !done.is_null() {
            return Ok(done);
        }
        let (state, g, act, edits) = (Arc::clone(&state_c), Arc::clone(&g), Arc::clone(&act), Arc::clone(&ed_edits));
        ctx.defer(
            ACTIVATION_TICKS,
            Box::new(move |_| settle(&state, &g, &act, &edits, &shared)),
        );
        Ok(Value::Null)
    });

    // A new map drops every stage and retired extent (the edit arena is
    // released at its own binding) and the debug views' replaced levels.
    let mut seen = None;
    agent.add_ticker("edit.collision", move |ctx| {
        if seen == Some(ctx.map_epoch) {
            return;
        }
        if seen.is_some() {
            let mut me = lock(&state);
            let next = me.next;
            *me = State { next, ..State::default() };
            edits.clear();
        }
        seen = Some(ctx.map_epoch);
    });
}

/// Puts `old` pairs back and frees the stage's `extents`: a commit whose
/// activation did not run.
fn undo(g: &mut Guest, shared: &arena::Shared, old: &[(u32, (u32, u32))], extents: &[u32]) {
    for (root, (count, address)) in old {
        // The pairs were written a moment ago, so they are writable.
        let _ = g.write_bytes(*root, &pair_bytes(*count, *address), 1);
    }
    let mut arena = arena::lock(shared);
    for at in extents {
        arena.free(*at);
    }
}

/// A commit's deferred answer: `None` while its activation runs; then the
/// retired extents freed and the reply, or the pairs put back and why.
fn settle(
    state: &Mutex<State>,
    guest: &WithGuest,
    act: &Activator,
    edits: &crate::debug_views::CollisionEdits,
    shared: &arena::Shared,
) -> Option<Result<Value, RpcError>> {
    let mut me = lock(state);
    if me.activating.is_none() {
        return Some(Err(refused("the map changed before the BSP activation ran", json!({}))));
    }
    let outcome = with(guest, |g| match lock(act).as_mut() {
        Some(a) => a.poll(g),
        None => Some(Err("the BSP activation was removed".into())),
    })?;
    let a = me.activating.take().expect("checked above");
    match outcome {
        Ok(()) => {
            let mut retired = std::mem::take(&mut me.retired);
            if a.place.kind != Kind::Coll {
                retired.extend(a.retire);
            }
            let freed: Vec<u32> = {
                let mut arena = arena::lock(shared);
                retired.into_iter().filter(|at| arena.free(*at).is_some()).collect()
            };
            if a.place.kind == Kind::Structure {
                edits.replace(a.place.tag, a.bsp);
            }
            let mut reply = a.reply;
            reply["activated"] = json!(true);
            reply["freed"] = json!(freed);
            Some(Ok(reply))
        }
        Err(why) => {
            with(guest, |g| undo(g, shared, &a.old, &a.extents));
            Some(Err(refused(
                format!("the BSP activation failed ({why}); the pairs are put back"),
                json!({"stage": a.reply["stage"]}),
            )))
        }
    }
}

#[cfg(test)]
mod tests;
