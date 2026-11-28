//! The opt-in slot-17 creature entity used by P7. The retail database owns
//! allocation, peer views, identity checks, replication batches and deletion.

mod definition;
mod wire;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use guest_layout::{DatumHandle, GuestMemory, MemFault, points, view::objects};
use guest_layout::ObjectType;
use hooks::{BuildError, Builder, Flow, GuestCall, Manifest};
use runtime::guest::{Access, FaultReason, GuestFault, Permissions};

pub const FEATURE: &str = "creature-probe";
pub const SLOT: u32 = 17;
const CREATURE: u8 = ObjectType::Creature as u8;
const NO_ENTITY: u32 = u32::MAX;
const HOST: u32 = 4;
const CLIENT: u32 = 5;
const POSITION: u32 = 1;
const BODY_ENTITY: u32 = 0xd4;

const ENTITY_POINTER: u32 = 4 + 4 * SLOT;
const VTABLE: u32 = 0x20;
const VTABLE_LEN: u32 = 0x94;
const NAME: u32 = 0xc0;
const PLACEMENT: u32 = 0x100;
const COUNTERS: u32 = 0x200;
const COUNTER_STRIDE: u32 = 16;
const MAGIC: u32 = u32::from_le_bytes(*b"crea");

const DATABASE: u32 = 0x2098;
const RECORDS: u32 = 0x14;
const RECORD_STRIDE: u32 = 0x20;
const INSTANCE_COUNT: u32 = 1024;
const RECORD_TYPE: u32 = 4;
const RECORD_LIVE: u32 = 6;
const RECORD_OBJECT: u32 = 8;
const RECORD_DIRTY: u32 = 0xc;
const RECORD_STATE: u32 = 0x1c;
const STORAGE_LEN: u32 = COUNTERS + COUNTER_STRIDE * INSTANCE_COUNT;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CreatureRow {
    /// Full entity identity, including the generation.
    pub entity: u32,
    /// Full salted handle of the local creature object.
    pub object: u32,
    /// Committed movement operations on the sender or applied on the receiver.
    pub updates: u64,
}

fn registered_definition(m: &dyn GuestMemory) -> Option<u32> {
    let registry = points::SIM_REPLICATION_REGISTRY.pointer(m).ok()??;
    if !(SLOT + 1..=32).contains(&m.u32(registry).ok()?) {
        return None;
    }
    let def = m.u32(registry + ENTITY_POINTER).ok()?;
    (def != 0 && m.u32(def).ok()? == def + VTABLE && m.u32(def + 4).ok()? == MAGIC).then_some(def)
}

/// Whether the simulation registry contains the probe's definition.
pub fn registered(m: &dyn GuestMemory) -> bool {
    registered_definition(m).is_some()
}

fn mode(m: &dyn GuestMemory) -> Option<u32> {
    let sim = points::SIM_GLOBALS.pointer(m).ok()??;
    m.u32(sim + 8).ok()
}

fn database(m: &dyn GuestMemory) -> Option<u32> {
    let sim = points::SIM_GLOBALS.pointer(m).ok()??;
    if !matches!(m.u32(sim + 8).ok()?, HOST | CLIENT) {
        return None;
    }
    let world = m.u32(sim + 4).ok()?;
    (world != 0).then_some(world + DATABASE)
}

fn instance_slot(entity: u32) -> u32 {
    entity & (INSTANCE_COUNT - 1)
}

fn record_at(database: u32, slot: u32) -> u32 {
    database + RECORDS + slot * RECORD_STRIDE
}

fn creature_record(m: &dyn GuestMemory, database: u32, entity: u32) -> Option<u32> {
    let record = record_at(database, instance_slot(entity));
    (entity != NO_ENTITY
        && m.u32(record).ok()? == entity
        && m.u16(record + RECORD_TYPE).ok()? == SLOT as u16)
        .then_some(record)
}

fn body(m: &dyn GuestMemory, object: u32) -> Result<Option<objects::Body>, MemFault> {
    let Some(handle) = DatumHandle::from_raw(object) else {
        return Ok(None);
    };
    Ok(objects::get(m, handle)?.and_then(|(kind, body)| (kind == CREATURE).then_some(body)))
}

fn live_object(m: &dyn GuestMemory, record: u32, entity: u32) -> Option<u32> {
    if m.u8(record + RECORD_LIVE).ok()? == 0 {
        return None;
    }
    let object = m.u32(record + RECORD_OBJECT).ok()?;
    let body = body(m, object).ok()??;
    (m.u32(body.addr + BODY_ENTITY).ok()? == entity).then_some(object)
}

/// Build the extra enrollment call for the Agent API's direct object
/// allocation path. The retail selector and unbound check still apply.
pub fn spawn_enrollment(m: &dyn GuestMemory, object: u32) -> Option<GuestCall> {
    if mode(m) != Some(HOST) || !registered(m) {
        return None;
    }
    let body = body(m, object).ok()??;
    if m.u32(body.addr + BODY_ENTITY).ok()? != NO_ENTITY {
        return None;
    }
    definition::capture(m, body).ok()??;
    Some(GuestCall::new(points::SIM_ENROLL_OBJECT.base).args([object]))
}

fn counter_at(definition: u32, entity: u32) -> u32 {
    definition + COUNTERS + COUNTER_STRIDE * instance_slot(entity)
}

fn init_counter(
    m: &mut dyn GuestMemory,
    definition: u32,
    entity: u32,
    object: u32,
) -> Result<(), MemFault> {
    let counter = counter_at(definition, entity);
    m.set_u32(counter, entity)?;
    m.set_u32(counter + 4, object)?;
    m.write(counter + 8, &0u64.to_le_bytes())
}

fn read_counter(m: &dyn GuestMemory, definition: u32, entity: u32, object: u32) -> Option<u64> {
    let counter = counter_at(definition, entity);
    if m.u32(counter).ok()? != entity || m.u32(counter + 4).ok()? != object {
        return None;
    }
    let mut bytes = [0; 8];
    m.read(counter + 8, &mut bytes).ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn count_update(
    m: &mut dyn GuestMemory,
    definition: u32,
    entity: u32,
    object: u32,
) -> Result<(), MemFault> {
    if let Some(updates) = read_counter(m, definition, entity, object) {
        m.write(
            counter_at(definition, entity) + 8,
            &updates.saturating_add(1).to_le_bytes(),
        )?;
    }
    Ok(())
}

/// Read the current world's live slot-17 entities. Both sides of each
/// binding must still name the same live instance.
pub fn snapshot(m: &dyn GuestMemory) -> Vec<CreatureRow> {
    let Some(definition) = registered_definition(m) else {
        return Vec::new();
    };
    let Some(database) = database(m) else {
        return Vec::new();
    };
    (0..INSTANCE_COUNT)
        .filter_map(|slot| {
            let record = record_at(database, slot);
            let entity = m.u32(record).ok()?;
            if instance_slot(entity) != slot || creature_record(m, database, entity) != Some(record)
            {
                return None;
            }
            let object = live_object(m, record, entity)?;
            Some(CreatureRow {
                entity,
                object,
                updates: read_counter(m, definition, entity, object).unwrap_or(0),
            })
        })
        .collect()
}

fn install_definition(
    m: &mut dyn GuestMemory,
    registry: u32,
    entity_count: u32,
    definition: u32,
    overrides: &[(u32, u32)],
) -> Result<(), MemFault> {
    let entry = registry + ENTITY_POINTER;
    let existing = m.u32(entry)?;
    if m.u32(entity_count)? != SLOT || (existing != 0 && existing != definition) {
        return Ok(());
    }
    let table = definition + VTABLE;
    let mut bytes = [0; VTABLE_LEN as usize];
    m.read(points::SIM_OBJECT_ENTITY_VTABLE.base, &mut bytes)?;
    for &(offset, callback) in overrides {
        bytes[offset as usize..][..4].copy_from_slice(&callback.to_le_bytes());
    }
    m.write(table, &bytes)?;
    m.write(definition + NAME, b"creature\0")?;
    m.set_u32(definition + 4, MAGIC)?;
    m.set_u32(definition, table)?;
    m.set_u32(entry, definition)?;
    m.set_u32(entity_count, SLOT + 1)
}

/// The sender's fixed probe policy. The two scores apply to different
/// operations, and the update score becomes eligible after a peer-local
/// period measured in the title's network clock.
const CREATION_PRIORITY: f32 = 1.0;
const MOVEMENT_PRIORITY: f32 = 0.5;
const MOVEMENT_PERIOD_MS: u32 = 200;

fn creation_relevance(mask: u32, no_creation: bool) -> (f32, u32) {
    if no_creation {
        return (0.0, 0);
    }
    // Entity type, creation record, initial-delta presence bit, position.
    let bits = 5 + wire::CREATION_BITS + 1 + movement_bits(mask);
    (CREATION_PRIORITY, bits)
}

fn movement_bits(mask: u32) -> u32 {
    u32::from(mask & POSITION != 0) * wire::POSITION_BITS
}

fn movement_relevance(mask: u32, last: u32, now: u32) -> f32 {
    if mask & POSITION != 0 && (last == u32::MAX || now.wrapping_sub(last) >= MOVEMENT_PERIOD_MS) {
        MOVEMENT_PRIORITY
    } else {
        0.0
    }
}

fn creature_update_priority(
    m: &dyn GuestMemory,
    record: u32,
    context: u32,
) -> Result<Option<f32>, MemFault> {
    if m.u16(record + RECORD_TYPE)? != SLOT as u16 {
        return Ok(None);
    }
    let mask = m.u32(context)?;
    let last_update = m.u32(context + 4)?;
    Ok(Some(movement_relevance(
        mask,
        last_update,
        network_time(m)?,
    )))
}

fn network_time(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    let override_at = points::SIM_NETWORK_TIME_OVERRIDE.base;
    if m.u8(override_at)? != 0 {
        return m.u32(override_at + 4);
    }
    m.u32(m.u32(points::KERNEL_TICK_COUNT_IMPORT.base)?)
}

fn mark_moved_creatures(m: &mut dyn GuestMemory, database: u32) -> Result<(), MemFault> {
    for slot in 0..INSTANCE_COUNT {
        let record = record_at(database, slot);
        if m.u16(record + RECORD_TYPE)? != SLOT as u16 {
            continue;
        }
        let entity = m.u32(record)?;
        let Some(object) = live_object(m, record, entity) else {
            continue;
        };
        let Some(body) = body(m, object)? else {
            continue;
        };
        let pos = body.position(m)?;
        let state = m.u32(record + RECORD_STATE)?;
        if state != 0 && pos.into_iter().all(f32::is_finite) && pos != m.vec3(state)? {
            let pending = m.u32(record + RECORD_DIRTY)?;
            m.set_u32(record + RECORD_DIRTY, pending | POSITION)?;
        }
    }
    Ok(())
}

fn is_replica(m: &dyn GuestMemory, object: u32) -> Option<()> {
    if mode(m) != Some(CLIENT) {
        return None;
    }
    let body = body(m, object).ok()??;
    let entity = m.u32(body.addr + BODY_ENTITY).ok()?;
    let record = creature_record(m, database(m)?, entity)?;
    (m.u32(record + RECORD_OBJECT).ok()? == object).then_some(())
}

fn guest_fault(f: MemFault) -> GuestFault {
    GuestFault {
        addr: f.addr,
        len: f.len as usize,
        access: if f.write { Access::Write } else { Access::Read },
        reason: FaultReason::Unmapped,
    }
}

type Pending = Mutex<BTreeMap<(u32, u32), (u32, u32)>>;

fn pending(lock: &Pending) -> std::sync::MutexGuard<'_, BTreeMap<(u32, u32), (u32, u32)>> {
    lock.lock().unwrap_or_else(|poison| poison.into_inner())
}

pub struct CreatureProbe;

impl hooks::Feature for CreatureProbe {
    fn manifest(&self) -> Manifest {
        Manifest::new(FEATURE, "Slot-17 creature replication probe")
            .doc("docs/game/ai-networking.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let definition = b.guest_alloc(STORAGE_LEN, Permissions::RW)?;
        let overrides = definition::entries(b, definition)?;
        b.raw().after(
            &points::SIM_REGISTER_REPLICATION_TYPES,
            "publish the creature definition and entity count",
            move |c| {
                let mut mem = c.ctx.runtime().memory();
                let Some(registry) = points::SIM_REPLICATION_REGISTRY
                    .pointer(&*mem as &dyn GuestMemory)
                    .map_err(guest_fault)?
                else {
                    return Ok(());
                };
                // The retail routine leaves the entity-count out pointer in EAX.
                install_definition(&mut *mem, registry, c.cpu.eax, definition, &overrides)
                    .map_err(guest_fault)
            },
        );
        b.raw().before(
            &points::SIM_OBJECT_ENTITY_TYPE,
            "enroll creature definitions on the distributed host",
            |c| {
                let tag = c.cpu.eax;
                let eligible = {
                    let m = c.ctx.runtime().memory();
                    mode(&*m) == Some(HOST)
                        && registered(&*m)
                        && definition::local_creature_tag(&*m, tag as u16)
                            .ok()
                            .flatten()
                            == Some(tag)
                };
                if eligible {
                    c.cpu.eax = SLOT;
                    return c.ret(0).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        );
        b.raw().before(
            &points::SIM_ENTITY_COLLECT_UPDATES,
            "dirty moved creatures before the host collects updates",
            |c| {
                let Some(db_arg) = c.arg_u32("database") else {
                    return Ok(Flow::Continue);
                };
                let mut m = c.ctx.runtime().memory();
                if mode(&*m) == Some(HOST) && database(&*m) == Some(db_arg) && registered(&*m) {
                    mark_moved_creatures(&mut *m, db_arg).map_err(guest_fault)?;
                }
                Ok(Flow::Continue)
            },
        );
        install_scheduler(b);
        install_telemetry(b, definition);
        for point in [&points::CREATURE_UPDATE, &points::CREATURE_POST_UPDATE] {
            b.raw()
                .before(point, "skip local creature simulation for a replica", |c| {
                    let Some(object) = c.arg_u32("object") else {
                        return Ok(Flow::Continue);
                    };
                    let replica = is_replica(&*c.ctx.runtime().memory(), object).is_some();
                    if replica {
                        c.cpu.eax &= !0xff;
                        return c.ret(4).map(Flow::Return);
                    }
                    Ok(Flow::Continue)
                });
        }
        Ok(())
    }
}

fn install_scheduler(b: &mut Builder) {
    b.raw().before(
        &points::SIM_ENTITY_CREATION_RELEVANCE,
        "schedule slot-17 creation",
        |c| {
            let Some(entity) = c.arg_u32("entity") else {
                return Ok(Flow::Continue);
            };
            let db = c.cpu.ecx;
            let creature = creature_record(&*c.ctx.runtime().memory(), db, entity).is_some();
            if !creature {
                return Ok(Flow::Continue);
            }
            let mask = c.arg_u32("mask").unwrap_or(0);
            let view = c.arg_u32("view").unwrap_or(0);
            let out_priority = c.arg_u32("priority").unwrap_or(0);
            let out_bits = c.arg_u32("bits").unwrap_or(0);
            {
                let mut guest = c.ctx.runtime().memory();
                let m: &mut dyn GuestMemory = &mut *guest;
                let (priority, bits) =
                    creation_relevance(mask, m.u8(view + 4).map_err(guest_fault)? != 0);
                m.set_f32(out_priority, priority).map_err(guest_fault)?;
                m.set_u32(out_bits, bits).map_err(guest_fault)?;
            }
            c.ret(20).map(Flow::Return)
        },
    );
    b.raw().before(
        &points::SIM_ENTITY_UPDATE_RELEVANCE,
        "schedule slot-17 movement",
        |c| {
            let Some(record) = c.arg_u32("record") else {
                return Ok(Flow::Continue);
            };
            let distance = c.cpu.eax;
            let context = c.arg_u32("context").unwrap_or(0);
            let period = c.arg_u32("period").unwrap_or(0);
            let priority = {
                let mut guest = c.ctx.runtime().memory();
                let m: &mut dyn GuestMemory = &mut *guest;
                let Some(priority) =
                    creature_update_priority(m, record, context).map_err(guest_fault)?
                else {
                    return Ok(Flow::Continue);
                };
                if distance != 0 {
                    m.set_f32(distance, 0.0).map_err(guest_fault)?;
                }
                if period != 0 {
                    m.set_u32(period, MOVEMENT_PERIOD_MS).map_err(guest_fault)?;
                }
                priority
            };
            c.cpu.xmm[0][..4].copy_from_slice(&priority.to_le_bytes());
            c.ret(12).map(Flow::Return)
        },
    );
}

fn install_telemetry(b: &mut Builder, definition: u32) {
    // The writer can encode a position and still roll back the operation if
    // the peer batch has too little space left. Only its successful return
    // counts. The entry frame identifies the candidate across the call.
    let updates = Arc::new(Pending::default());
    let entering = Arc::clone(&updates);
    b.raw().before(
        &points::SIM_VIEW_WRITE_ENTITY_UPDATE,
        "track a candidate creature batch operation",
        move |c| {
            let Some(view) = c.arg_u32("view") else {
                return Ok(Flow::Continue);
            };
            let slot = c.cpu.ecx;
            let candidate = {
                let m = c.ctx.runtime().memory();
                let candidate = || {
                    if slot >= INSTANCE_COUNT || mode(&*m) != Some(HOST) {
                        return None;
                    }
                    // Peer-view entry: its identity followed by 16 bytes of peer state.
                    let entity = m.u32(view + 0x24 + slot * 0x14).ok()?;
                    let record = creature_record(&*m, database(&*m)?, entity)?;
                    Some((entity, live_object(&*m, record, entity)?))
                };
                candidate()
            };
            if let Some(candidate) = candidate {
                pending(&entering).insert((c.ctx.thread_id, c.cpu.esp), candidate);
            }
            Ok(Flow::Continue)
        },
    );
    let returning = Arc::clone(&updates);
    b.raw().after(
        &points::SIM_VIEW_WRITE_ENTITY_UPDATE,
        "count a committed creature batch operation",
        move |c| {
            // The return address and three arguments have been popped.
            let entry_sp = c.cpu.esp.wrapping_sub(16);
            let candidate = pending(&returning).remove(&(c.ctx.thread_id, entry_sp));
            if let Some((entity, object)) = candidate
                && c.cpu.eax & 0xff != 0
            {
                let mut m = c.ctx.runtime().memory();
                count_update(&mut *m, definition, entity, object).map_err(guest_fault)?;
            }
            Ok(())
        },
    );
    b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
        (!pending(&updates).is_empty()).then(|| "creature replication writer active".into())
    }));
}

#[cfg(test)]
mod tests;
