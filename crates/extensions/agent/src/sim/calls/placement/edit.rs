//! `world.placement.edit`: one transaction on one scenario placement block
//! (docs/editor/realtime-edits.md, "Placements").
//!
//! The editor computes the transaction from its document diff, so an add, a
//! remove and their undos are the same call. At the tick boundary, in this
//! order: every check on guest bytes (the root count, each record's `was`,
//! the objects the transaction names), then [`run`] deletes the live
//! occupants of `delete`, gives the occupants of `relink` their new
//! placement index, writes the records, the object-name repoints and the
//! count, and makes an object for each index of `create`.
//!
//! - **Room.** The scenario block is a run of records in tag memory with
//!   nothing reserved after it. A block has room while its count is under
//!   its capacity: the count the loaded map gave it, which the host records
//!   at the block's first edit of a map activation ([`BlockState`]), or the
//!   capacity of the edit arena extent it was moved to
//!   (`hot_tags::arena`). A removed entry's slot stays in the block, so an
//!   add after a remove reuses it. A count past the capacity moves the
//!   block into the arena with twice the capacity (at least the new count,
//!   at most the per-type limit): the extent is allocated before anything
//!   is written, the live records and the dead tail are copied into it, and
//!   the root's address is repointed in the same write as the records and
//!   the count. An arena extent the block left is freed. A full arena, or a
//!   game without one, refuses with `data.need` and `data.capacity` and the
//!   edit falls back to a reload.
//! - **Creation is linked.** The object is made by the placement-data and
//!   `object_new` sequence that `world.object.respawn` uses, with the
//!   placement index written into the placement data at `+0x10` before the
//!   call, from where `object_new` copies it into the object (`0x000b7c87`,
//!   live-telemetry's `layout::BODY_PLACEMENT_INDEX`). **S**: that a linked
//!   object equals a loaded one; the result's `linked` is read back from
//!   the object, not assumed.
//! - **What a created object honours** is what [`super::read`] reads. A
//!   record it refuses is still written, and its index is reported with
//!   `created: false` and the reason.
//! - **Peers.** A count change is refused with other machines in the game
//!   (docs/editor/scenario-editor.md section 6: network object creation sends
//!   the placement index, which a client checks against its own count).

use std::collections::{BTreeMap, BTreeSet};

use agent_proto::sim::{
    PlacementCreated, PlacementEditParams, Placement, Was,
};
use agent_proto::{Kind, ObjectId, RpcError, code};
use guest_layout::GuestMemory;
use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::ObjectType;
use map::scenario_layout::{self as layout, BlockKind};
use serde_json::{Value, json};

use super::super::{Outcome, RespawnTarget, Shared, addr, lock, object_delete, prepare_spawn};
use hot_tags::arena;
use super::{BLOCKS, fault, read, resolve};
use crate::sim::edits::refused;

/// What a record or an entry held before the edit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Bytes(Vec<u8>),
    /// FNV-1a 64 of the bytes.
    Hash(u64),
}

impl Expect {
    fn holds(&self, bytes: &[u8]) -> bool {
        match self {
            Expect::Bytes(b) => b == bytes,
            Expect::Hash(h) => fnv1a(bytes) == *h,
        }
    }
}

/// FNV-1a 64, the hash `Was::Hash` carries.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unhex(s: &str, what: &str) -> Result<Vec<u8>, RpcError> {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(format!("{what}: hex digits in pairs")));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| invalid(format!("{what}: bad hex"))))
        .collect()
}

/// The block a transaction edits: where it sits in the scenario root, what a
/// record weighs, and which objects its records make.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockSpec {
    pub name: String,
    /// Offset of the block's `(count, address)` pair in the scenario root.
    pub offset: u32,
    pub stride: u32,
    /// The most entries the engine accepts (`map::scenario_layout`).
    pub max: Option<usize>,
    pub add_refusal: Option<&'static str>,
    pub remove_refusal: Option<&'static str>,
    /// The kind and type of the objects the block's records make, for a
    /// `placement::BLOCKS` block. Other blocks are records only: the engine
    /// walks them itself.
    pub objects: Option<(Kind, ObjectType)>,
}

impl BlockSpec {
    pub fn named(name: &str) -> Option<Self> {
        let def = BlockKind::from_name(name).map(layout::block);
        if let Some(&(_, kind, ty)) = BLOCKS.iter().find(|b| b.0 == name) {
            let (offset, _, stride) = tags::object::OBJECT_TYPES[ty as usize].placement?;
            return Some(Self {
                name: name.into(),
                offset: offset.into(),
                stride: stride.into(),
                max: def.and_then(|d| d.max_count),
                add_refusal: def.and_then(|d| d.add_refusal),
                remove_refusal: def.and_then(|d| d.remove_refusal),
                objects: Some((kind, ty)),
            });
        }
        let def = def?;
        Some(Self {
            name: name.into(),
            offset: def.offset,
            stride: def.stride,
            max: def.max_count,
            add_refusal: def.add_refusal,
            remove_refusal: def.remove_refusal,
            objects: None,
        })
    }

    fn objects(&self, what: &str) -> Result<(Kind, ObjectType), RpcError> {
        self.objects.ok_or_else(|| {
            invalid(format!(
                "{what}: {} holds records only; the engine reads them itself",
                self.name
            ))
        })
    }
}

/// A transaction checked against the block's shape, with nothing read from
/// the game yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub block: String,
    pub was_count: u32,
    pub count: u32,
    /// `(index, bytes, was)`.
    pub records: Vec<(u16, Vec<u8>, Option<Expect>)>,
    /// `(object name index, placement)`.
    pub names: Vec<(u16, i16)>,
    pub delete: Vec<u16>,
    /// `(from, to)`.
    pub relink: Vec<(u16, u16)>,
    pub create: Vec<u16>,
}

fn distinct<T: Ord + Copy>(items: impl Iterator<Item = T>, what: &str) -> Result<(), RpcError> {
    let mut seen = BTreeSet::new();
    for item in items {
        if !seen.insert(item) {
            return Err(invalid(format!("{what}: an index appears twice")));
        }
    }
    Ok(())
}

impl Plan {
    /// Whether the root count changes.
    pub fn changes_count(&self) -> bool {
        self.count != self.was_count
    }

    pub fn new(p: PlacementEditParams) -> Result<Self, RpcError> {
        let spec = BlockSpec::named(&p.block).ok_or_else(|| {
            invalid(format!(
                "block: {:?} is not a placement block (a placement::BLOCKS name, \
                 starting_locations, netgame_equipment, netgame_flags or starting_equipment)",
                p.block
            ))
        })?;
        // Placement indices travel as i16 (object placement index, name table).
        let ceiling = u32::from(i16::MAX as u16);
        if p.count > ceiling || p.was_count > ceiling {
            return Err(invalid("count: past 32767 entries"));
        }
        if let Some(max) = spec.max
            && p.count as usize > max
        {
            let mut e = refused(format!(
                "{} holds at most {max} entries; the edit asks for {}",
                spec.name, p.count
            ));
            e.data = Some(json!({ "why": e.message, "limit": max }));
            return Err(e);
        }
        if p.count > p.was_count
            && let Some(why) = spec.add_refusal
        {
            return Err(refused(format!("{}: {why}", spec.name)));
        }
        if p.count < p.was_count
            && let Some(why) = spec.remove_refusal
        {
            return Err(refused(format!("{}: {why}", spec.name)));
        }

        let stride = spec.stride as usize;
        let mut records = Vec::with_capacity(p.records.len());
        for r in &p.records {
            if u32::from(r.index) >= p.count {
                return Err(invalid(format!(
                    "records: index {} is outside the new count {}",
                    r.index, p.count
                )));
            }
            let bytes = unhex(&r.bytes, "records.bytes")?;
            if bytes.len() != stride {
                return Err(invalid(format!(
                    "records: {} holds {stride}-byte records, index {} has {} bytes",
                    spec.name,
                    r.index,
                    bytes.len()
                )));
            }
            let was = match &r.was {
                None => None,
                Some(Was::Bytes(h)) => {
                    let b = unhex(h, "records.was")?;
                    if b.len() != stride {
                        return Err(invalid(format!(
                            "records: was of index {} is not {stride} bytes",
                            r.index
                        )));
                    }
                    Some(Expect::Bytes(b))
                }
                Some(Was::Hash { hash }) => Some(Expect::Hash(
                    (hash.len() == 16)
                        .then(|| u64::from_str_radix(hash, 16).ok())
                        .flatten()
                        .ok_or_else(|| invalid("records.was.hash: 16 hex digits"))?,
                )),
            };
            records.push((r.index, bytes, was));
        }
        distinct(records.iter().map(|r| r.0), "records")?;

        let names: Vec<(u16, i16)> = p.names.iter().map(|n| (n.name, n.placement)).collect();
        if !names.is_empty() {
            spec.objects("names")?;
        }
        for &(name, placement) in &names {
            if placement < -1 || (placement >= 0 && placement as u32 >= p.count) {
                return Err(invalid(format!(
                    "names: name {name} cannot point at placement {placement} of {} entries",
                    p.count
                )));
            }
        }
        distinct(names.iter().map(|n| n.0), "names")?;

        if !p.delete.is_empty() {
            spec.objects("delete")?;
        }
        if let Some(i) = p.delete.iter().find(|&&i| u32::from(i) >= p.was_count) {
            return Err(invalid(format!("delete: index {i} is outside the old count {}", p.was_count)));
        }
        distinct(p.delete.iter().copied(), "delete")?;

        if !p.relink.is_empty() {
            spec.objects("relink")?;
        }
        for r in &p.relink {
            if u32::from(r.from) >= p.was_count || u32::from(r.to) >= p.count {
                return Err(invalid(format!(
                    "relink: {} to {} is outside the old count {} or the new count {}",
                    r.from, r.to, p.was_count, p.count
                )));
            }
            if p.delete.contains(&r.from) {
                return Err(invalid(format!("relink: {} is also deleted", r.from)));
            }
        }
        distinct(p.relink.iter().map(|r| r.from), "relink.from")?;
        distinct(p.relink.iter().map(|r| r.to), "relink.to")?;

        if !p.create.is_empty() {
            spec.objects("create")?;
        }
        if let Some(i) = p.create.iter().find(|&&i| u32::from(i) >= p.count) {
            return Err(invalid(format!("create: index {i} is outside the new count {}", p.count)));
        }
        distinct(p.create.iter().copied(), "create")?;

        Ok(Self {
            block: p.block,
            was_count: p.was_count,
            count: p.count,
            records,
            names,
            delete: p.delete,
            relink: p.relink.iter().map(|r| (r.from, r.to)).collect(),
            create: p.create,
        })
    }
}

/// What the host knows of a block it has edited in this map activation: the
/// room the loaded map gave it. Kept in the call queue and dropped with the
/// activation (`Queue::map_activated`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockState {
    /// The scenario and cache the capacity was read under. A restored
    /// snapshot or another map starts the block over.
    scenario: u32,
    checksum: u32,
    /// Where the block sat. A block that is elsewhere now was moved by
    /// something this host did not record.
    address: u32,
    capacity: u32,
}

/// The reads of a transaction, finished before the first write.
struct Survey {
    header: u32,
    base: u32,
    capacity: u32,
    /// The capacity to grow into, when the count passes `capacity`.
    grow: Option<u32>,
    /// The scenario tag's datum, which an arena extent is booked to.
    scenario_tag: u32,
    /// The arena, when the game has one.
    arena: Option<arena::Link>,
    /// Live objects to delete, in the order of `plan.delete` then previews.
    delete: Vec<ObjectId>,
    /// `(new index, objects)` for each relink.
    relink: Vec<(u16, Vec<ObjectId>)>,
    /// The address of each name's placement field, in `plan.names` order.
    names: Vec<u32>,
}

fn mismatch(message: String, data: Value) -> RpcError {
    let mut e = RpcError::new(code::MISMATCH, message);
    e.data = Some(data);
    e
}

/// Every live, unheld object of `ty` by its placement index, and the indices
/// whose object is attached to another.
fn placed(
    m: &dyn GuestMemory,
    kind: Kind,
    ty: ObjectType,
) -> Result<(BTreeMap<u16, Vec<ObjectId>>, BTreeSet<u16>), RpcError> {
    let mut by_index: BTreeMap<u16, Vec<ObjectId>> = BTreeMap::new();
    let mut held = BTreeSet::new();
    for obj in sim_state::objects(m).map_err(fault)? {
        let obj = obj.map_err(fault)?;
        if obj.kind().map_err(fault)? != Some(ty) || obj.flags().map_err(fault)?.deleting {
            continue;
        }
        let Some(index) = obj.placement().map_err(fault)? else {
            continue;
        };
        if obj.parent().map_err(fault)?.is_some() {
            held.insert(index);
        }
        by_index.entry(index).or_default().push(ObjectId { kind, key: obj.id().0 });
    }
    Ok((by_index, held))
}

fn survey(
    m: &dyn GuestMemory,
    spec: &BlockSpec,
    plan: &Plan,
    queue: &Shared,
) -> Result<Survey, RpcError> {
    let scenario = m.u32(guest_layout::points::SCENARIO.base).map_err(fault)?;
    if scenario == 0 {
        return Err(refused("no loaded scenario"));
    }
    let header = scenario.checked_add(spec.offset).ok_or_else(|| refused("no loaded scenario block"))?;
    let live = m.u32(header).map_err(fault)?;
    let base = m.u32(header + 4).map_err(fault)?;
    if live != plan.was_count {
        return Err(mismatch(
            format!(
                "{} holds {live} entries, the edit started from {}",
                plan.block, plan.was_count
            ),
            json!({ "count": live }),
        ));
    }
    if live > 0 && base == 0 {
        return Err(refused(format!("{} has no block pointer", plan.block)));
    }
    let checksum = guest_layout::game::cache_checksum(m).unwrap_or(0);
    let link = lock(queue).arena();
    // A block in an arena extent made for this root has that extent's room.
    let moved = link.as_ref().and_then(|l| {
        arena::lock(&l.arena)
            .extent(base)
            .filter(|e| e.root == header && e.stride == spec.stride)
            .map(|e| e.capacity)
    });
    let capacity = match (moved, lock(queue).blocks.get(&plan.block)) {
        (Some(capacity), _) => capacity,
        (None, Some(s)) if s.scenario == scenario && s.checksum == checksum => {
            if s.address != base {
                return Err(refused(format!(
                    "{} moved from {:#x} to {base:#x} since the host last edited it",
                    plan.block, s.address
                )));
            }
            s.capacity
        }
        _ => live,
    };
    let grow = if plan.count > capacity {
        if link.is_none() {
            let mut e = refused(format!(
                "{} has room for {capacity} entries; the edit asks for {}, and this game has \
                 no edit arena to grow the block into",
                plan.block, plan.count
            ));
            e.data = Some(json!({ "why": e.message, "need": plan.count, "capacity": capacity }));
            return Err(e);
        }
        // Twice the room, at least the new count, at most the type's limit
        // (which `Plan::new` held the count to).
        let limit = spec.max.map_or(u32::MAX, |max| u32::try_from(max).unwrap_or(u32::MAX));
        Some(capacity.saturating_mul(2).max(plan.count).min(limit))
    } else {
        None
    };
    // The tag an arena extent is booked to: the loaded scenario's datum.
    let scenario_tag = match grow {
        Some(_) => m
            .u32(guest_layout::points::TAG_INDEX.base)
            .and_then(|index| m.u32(index.wrapping_add(hot_tags::index::HEADER_SCENARIO)))
            .map_err(fault)?,
        None => 0,
    };

    let stride = spec.stride;
    let slot = |index: u16| base.wrapping_add(u32::from(index) * stride);
    let mut stale = Vec::new();
    for (index, _, was) in &plan.records {
        // A slot is addressable below the capacity; a dead tail slot holds
        // what the last removal left in it. A slot past it is new room,
        // zeroed in the arena.
        let current = if u32::from(*index) >= capacity {
            vec![0; stride as usize]
        } else {
            m.bytes(slot(*index), stride as usize).map_err(|e| {
                refused(format!("record {index} is not addressable at {:#x}", e.addr))
            })?
        };
        if was.as_ref().is_some_and(|w| !w.holds(&current)) {
            stale.push(*index);
        }
    }
    if !stale.is_empty() {
        return Err(mismatch(
            format!("{} records changed since the editor read them: {stale:?}", plan.block),
            json!({ "records": stale.iter().map(|i| json!({ "index": i })).collect::<Vec<_>>() }),
        ));
    }

    let mut names = Vec::new();
    let mut delete = Vec::new();
    let mut relink = Vec::new();
    if let Some((kind, ty)) = spec.objects {
        if !plan.names.is_empty() {
            let count = m.u32(scenario + layout::OBJECT_NAMES).map_err(fault)?;
            let first = m.u32(scenario + layout::OBJECT_NAMES + 4).map_err(fault)?;
            for &(name, _) in &plan.names {
                if u32::from(name) >= count || first == 0 {
                    return Err(refused(format!(
                        "object name {name} is outside the scenario's {count} names"
                    )));
                }
                let entry = first + u32::from(name) * layout::OBJECT_NAME_STRIDE;
                let named = m.u16(entry + layout::OBJECT_NAME_TYPE as u32).map_err(fault)? as i16;
                if named != ty as i16 {
                    return Err(refused(format!(
                        "object name {name} names a placement of type {named}, not {}",
                        plan.block
                    )));
                }
                names.push(entry + layout::OBJECT_NAME_PLACEMENT as u32);
            }
        }
        if !plan.delete.is_empty() || !plan.relink.is_empty() {
            let (by_index, held) = placed(m, kind, ty)?;
            let queued = lock(queue);
            let previews = &queued.previews;
            let objects_of = |index: u16| -> Result<Vec<ObjectId>, RpcError> {
                if held.contains(&index) {
                    return Err(refused(format!(
                        "the object of {} {index} is held or attached",
                        plan.block
                    )));
                }
                Ok(by_index.get(&index).cloned().unwrap_or_default())
            };
            // A respawn preview stands for its placement but carries no index.
            let preview_of = |index: u16| -> Result<Option<ObjectId>, RpcError> {
                match previews.objects.get(&(kind, index)) {
                    None => Ok(None),
                    Some(p) => match p.current {
                        Some(key) => {
                            let id = ObjectId { kind, key };
                            Ok(resolve(m, id).is_ok().then_some(id))
                        }
                        None => Err(refused(format!(
                            "{} {index} may have an unlinked preview from before a checkpoint; \
                             reload the map",
                            plan.block
                        ))),
                    },
                }
            };
            for &index in &plan.delete {
                delete.extend(objects_of(index)?);
                delete.extend(preview_of(index)?);
            }
            for &(from, to) in &plan.relink {
                relink.push((to, objects_of(from)?));
            }
            // The previews that follow a relink must still exist at the end.
            for &(from, _) in &plan.relink {
                preview_of(from)?;
            }
        }
    }

    lock(queue).blocks.insert(
        plan.block.clone(),
        BlockState { scenario, checksum, address: base, capacity },
    );
    Ok(Survey {
        header,
        base,
        capacity,
        grow,
        scenario_tag,
        arena: link,
        delete,
        relink,
        names,
    })
}

/// Run the transaction. A refusal is the outcome and nothing was written; a
/// guest call that faults ends the tick's work, as it does for every call.
pub(in super::super) async fn run(
    cx: &Cx,
    scratch: u32,
    queue: &Shared,
    plan: &Plan,
) -> Result<Outcome, EntryError> {
    let Some(spec) = BlockSpec::named(&plan.block) else {
        return Ok(Err(refused(format!("{} is not a placement block", plan.block))));
    };
    let survey = match cx.read(|m| survey(m, &spec, plan, queue)) {
        Ok(s) => s,
        Err(e) => return Ok(Err(e)),
    };

    // 0. A block that grows gets its arena extent before anything is
    // written: a full arena refuses with the block as it was.
    let grown = match (&survey.arena, survey.grow) {
        (Some(link), Some(capacity)) => {
            let claim = arena::Claim {
                tag: survey.scenario_tag,
                root: survey.header,
                stride: spec.stride,
                capacity,
            };
            match link.with(|a, pages| a.alloc(pages, claim)) {
                Ok(extent) => Some((link.clone(), extent)),
                Err(full) => {
                    let mut e = refused(format!(
                        "{} has room for {} entries and the edit arena has none for {capacity}: {}",
                        plan.block, survey.capacity, full.why
                    ));
                    e.data = Some(json!({
                        "why": e.message,
                        "need": plan.count,
                        "capacity": survey.capacity,
                        "need_bytes": full.need,
                        "largest_free": full.largest_free,
                    }));
                    return Ok(Err(e));
                }
            }
        }
        _ => None,
    };
    let applied = apply(cx, scratch, queue, plan, &spec, &survey, grown.as_ref().map(|g| g.1)).await;
    if let (Err(_), Some((link, extent))) = (&applied, &grown) {
        // The guest call faulted before the block was repointed, or after:
        // either way the tick's work ends here, and an extent nothing
        // points at is freed.
        let points = cx
            .read(|m| m.u32(survey.header + 4))
            .is_ok_and(|address| address == extent.at);
        if !points {
            arena::lock(&link.arena).free(extent.at);
        }
    }
    applied
}

/// Steps 1 to 4 of [`run`], into `grown` when the block moves.
async fn apply(
    cx: &Cx,
    scratch: u32,
    queue: &Shared,
    plan: &Plan,
    spec: &BlockSpec,
    survey: &Survey,
    grown: Option<arena::Extent>,
) -> Result<Outcome, EntryError> {
    // 1. Delete the occupants of the removed entries. An object already gone
    // (a child of an earlier delete) is what the edit wanted.
    let mut deleted = Vec::new();
    for &id in &survey.delete {
        if object_delete(cx, queue, id).await?.is_ok() {
            deleted.push(id.to_string());
        }
    }

    // 2. Move the occupants of relinked entries to their new index, so the
    // engine's re-create pass on a BSP switch finds each under its placement.
    let mut relinked = Vec::new();
    for (to, ids) in &survey.relink {
        for &id in ids {
            let done = cx.write(|m| -> Result<(), RpcError> {
                let body = resolve(&*m, id)?;
                let at = body.addr + guest_layout::Object::PLACEMENT_INDEX.offset;
                m.write(at, &(*to as i16).to_le_bytes()).map_err(fault)
            });
            if done.is_ok() {
                relinked.push(id.to_string());
            }
        }
    }
    if let Some((kind, _)) = spec.objects
        && (!plan.delete.is_empty() || !plan.relink.is_empty())
    {
        lock(queue).previews.reindex(kind, &plan.delete, &plan.relink);
    }

    // 3. Move a grown block (its old room copied into the extent, the rest
    // zeroed, the root repointed), then write the records, the name
    // repoints and the count.
    let stride = spec.stride;
    let base = grown.map_or(survey.base, |e| e.at);
    cx.write(|m| -> Result<(), guest_layout::MemFault> {
        if let Some(extent) = grown {
            let mut block = if survey.capacity == 0 {
                Vec::new()
            } else {
                m.bytes(survey.base, (survey.capacity * stride) as usize)?
            };
            block.resize(extent.len as usize, 0);
            m.write(extent.at, &block)?;
            m.write(survey.header + 4, &extent.at.to_le_bytes())?;
        }
        for (index, bytes, _) in &plan.records {
            m.write(base + u32::from(*index) * stride, bytes)?;
        }
        for (at, (_, placement)) in survey.names.iter().zip(&plan.names) {
            m.write(*at, &placement.to_le_bytes())?;
        }
        m.write(survey.header, &plan.count.to_le_bytes())
    })?;
    let mut moved = None;
    if let Some(extent) = grown {
        // The block left an extent of its own: nothing points there now.
        if let Some(link) = &survey.arena {
            let mut held = arena::lock(&link.arena);
            if held.extent(survey.base).is_some_and(|e| e.root == survey.header) {
                held.free(survey.base);
            }
        }
        if let Some(state) = lock(queue).blocks.get_mut(&plan.block) {
            state.address = extent.at;
            state.capacity = extent.capacity;
        }
        moved = Some(json!({ "address": extent.at, "capacity": extent.capacity }));
    }

    // 4. Make an object for each created index.
    let mut created = Vec::with_capacity(plan.create.len());
    for &index in &plan.create {
        created.push(create(cx, scratch, queue, spec, index).await?);
    }

    let mut out = json!({
        "count": plan.count,
        "capacity": grown.map_or(survey.capacity, |e| e.capacity),
        "deleted": deleted,
        "relinked": relinked,
        "created": created,
    });
    if let Some(moved) = moved {
        out["moved"] = moved;
    }
    Ok(Ok(out))
}

fn not_created(index: u16, why: impl Into<String>) -> PlacementCreated {
    PlacementCreated {
        index,
        object: None,
        linked: false,
        created: false,
        why: Some(why.into()),
    }
}

/// One linked object for entry `index`, or the reason the engine path cannot
/// make this record's object.
pub(in crate::sim::calls) async fn create(
    cx: &Cx,
    scratch: u32,
    queue: &Shared,
    spec: &BlockSpec,
    index: u16,
) -> Result<PlacementCreated, EntryError> {
    let target = RespawnTarget::Placement(Placement { block: spec.name.clone(), index });
    let entry = match cx.read(|m| read(m, &target, &lock(queue).previews)) {
        Ok(entry) => entry,
        Err(e) => return Ok(not_created(index, e.message)),
    };
    if entry.old.is_some() {
        return Ok(not_created(
            index,
            format!("a live object already holds {} {index}", spec.name),
        ));
    }
    prepare_spawn(cx, scratch, entry.tag, entry.pos, Some(entry.axes)).await?;
    // Between the placement data and `object_new`: the index `object_new`
    // copies into the object.
    cx.write(|m| m.write(scratch + addr::PLACEMENT_INDEX, &index.to_le_bytes()))?;
    let made = cx.call(GuestCall::new(addr::OBJECT_NEW).args([scratch])).await?.eax;
    if made == u32::MAX {
        return Ok(not_created(
            index,
            format!("object_new (0xb7b40) refused to make tag {:#010x}", entry.tag),
        ));
    }
    let id = ObjectId { kind: entry.kind, key: made };
    let linked = cx
        .read(|m| resolve(m, id).and_then(|b| b.placement_index(m).map_err(fault)))
        .is_ok_and(|p| p == index as i16);
    Ok(PlacementCreated {
        index,
        object: Some(id.to_string()),
        linked,
        created: true,
        why: (!linked).then(|| "the object did not take the placement index".into()),
    })
}

#[cfg(test)]
mod tests;
