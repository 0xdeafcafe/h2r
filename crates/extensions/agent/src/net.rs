//! `net.replication` (docs/runtime/agent-api.md, "net"): what the
//! distributed simulation is replicating, for the probes of
//! docs/game/ai-networking.md.
//!
//! At one tick boundary, read the simulation mode and live objects by
//! kind, including their creature entity bindings. Read the process's
//! event and datagram counters at that same boundary.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use agent_proto::{RpcError, code};
use guest_layout::{GuestMemory, MemFault, view::Datums};
use guest_layout::ObjectType;
use serde_json::{Value, json};

use crate::{Access, Agent};

pub const METHOD: &str = "net.replication";

/// Simulation globals `+8`: the world's mode.
const SIM_MODE: u32 = 0x8;
/// Object body `+0xd4`: the replication entity index, -1 without one
/// (docs/game/notes/ai-networking.md, "An object's entity").
const BODY_ENTITY: u32 = 0xd4;
/// The objects pool's size.
const MAX_OBJECTS: i32 = 2048;

/// The replication event types in the engine's order; the names are ours
/// (docs/game/notes/ai-networking.md).
pub const EVENT_NAMES: [&str; 26] = [
    "damage aftermath",
    "damage section response",
    "breakable surface damage",
    "projectile attached",
    "projectile effect",
    "projectile impact effect",
    "projectile object impact effect",
    "game engine event",
    "unit board vehicle",
    "unit pickup",
    "weapon effect",
    "unit exit vehicle",
    "weapon fire",
    "weapon reload",
    "grenade initiate",
    "melee initiate",
    "weapon pickup",
    "weapon put away",
    "weapon drop",
    "vehicle flip",
    "vehicle trick",
    "device touch",
    "grenade release",
    "melee damage",
    "unit enter vehicle",
    "request boot player",
];

/// One counter per event type and a last one for a type outside the table.
pub struct EventCounts([AtomicU64; EVENT_NAMES.len() + 1]);

impl EventCounts {
    pub const fn new() -> Self {
        Self([const { AtomicU64::new(0) }; EVENT_NAMES.len() + 1])
    }

    /// One event of `kind` was queued.
    pub fn count(&self, kind: u32) {
        let slot = (kind as usize).min(EVENT_NAMES.len());
        self.0[slot].fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> [u64; EVENT_NAMES.len() + 1] {
        std::array::from_fn(|i| self.0[i].load(Ordering::Relaxed))
    }
}

impl Default for EventCounts {
    fn default() -> Self {
        Self::new()
    }
}

/// The process's counts: [`NetFeature`] fills them.
pub static EVENTS: EventCounts = EventCounts::new();

/// Counts each call of the engine's replication event entry by type.
/// Observes only: the game runs the same with it on or off.
pub struct NetFeature;

impl hooks::Feature for NetFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            "agent-net",
            "Agent API: net.replication's event counts by type",
        )
        .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.raw()
            .observe_before(
                &guest_layout::points::SIM_EVENT_GENERATE,
                "count replication events",
                |o| {
                    if let Some(kind) = o.arg_u32("kind") {
                        EVENTS.count(kind);
                    }
                },
            )
            .optional();
        Ok(())
    }
}

guest_layout::record! {
    // The 0xc-byte object header: the type byte and the body's address.
    struct Header: 0xc {
        kind @ 0x3: u8,
        body @ 0x8: u32,
    }
}

/// The datagram counters of the title's sockets, when the host has them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Traffic {
    pub sent: u64,
    pub sent_bytes: u64,
    pub received: u64,
    pub received_bytes: u64,
    pub largest: u64,
}

/// Slot registration and the live, locally bound creature entities.
#[derive(Debug, Default)]
pub struct CreatureReplication {
    pub slot: Option<u32>,
    pub entities: Vec<CreatureEntity>,
}

/// A full replication identity, its local salted object handle and the
/// number of movement updates sent or applied during this binding.
#[derive(Debug)]
pub struct CreatureEntity {
    pub entity: u32,
    pub object: u32,
    pub updates: u64,
}

fn mode_name(mode: u32) -> &'static str {
    match mode {
        0 => "none",
        1 => "local",
        2 => "sync-server",
        3 => "sync-client",
        4 => "dist-server",
        5 => "dist-client",
        _ => "unknown",
    }
}

/// The world's mode, `None` before the simulation globals exist.
fn world_mode(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let globals = m.u32(guest_layout::points::SIM_GLOBALS.base)?;
    if globals == 0 {
        return Ok(None);
    }
    m.u32(globals.wrapping_add(SIM_MODE)).map(Some)
}

/// Per object kind: live objects and how many have an entity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct KindCount {
    objects: u32,
    with_entity: u32,
}

fn objects(m: &dyn GuestMemory) -> Result<BTreeMap<&'static str, KindCount>, MemFault> {
    let mut kinds = BTreeMap::new();
    let Some(pool) = Datums::<Header>::from_global(m, guest_layout::points::OBJECTS.base)? else {
        return Ok(kinds);
    };
    for (_, header) in pool.live(m, MAX_OBJECTS)? {
        let body = header.body(m)?;
        if body == 0 {
            continue;
        }
        let name = ObjectType::from_u8(header.kind(m)?).map_or("other", ObjectType::group);
        let count: &mut KindCount = kinds.entry(name).or_default();
        count.objects += 1;
        if m.u32(body.wrapping_add(BODY_ENTITY))? != u32::MAX {
            count.with_entity += 1;
        }
    }
    Ok(kinds)
}

fn reply(
    m: &dyn GuestMemory,
    events: &EventCounts,
    traffic: Option<Traffic>,
    creatures: CreatureReplication,
) -> Result<Value, MemFault> {
    let mode = world_mode(m)?;
    let kinds = objects(m)?;
    let entities: u32 = kinds.values().map(|k| k.with_entity).sum();
    let counts = events.snapshot();
    let by_type: Vec<Value> = counts
        .iter()
        .enumerate()
        .filter(|(_, n)| **n != 0)
        .map(|(i, n)| match EVENT_NAMES.get(i) {
            Some(name) => json!({ "type": i, "name": name, "count": n }),
            None => json!({ "type": Value::Null, "name": "other", "count": n }),
        })
        .collect();
    let objects: serde_json::Map<String, Value> = kinds
        .iter()
        .map(|(name, k)| {
            (
                (*name).to_owned(),
                json!({ "objects": k.objects, "with_entity": k.with_entity }),
            )
        })
        .collect();
    let creature_rows: Vec<Value> = creatures.slot.map_or_else(Vec::new, |slot| {
        creatures
            .entities
            .into_iter()
            .map(|row| json!({
                "entity": row.entity,
                "type": slot,
                "object": agent_proto::sim::objects::handle(row.object),
                "updates": row.updates,
            }))
            .collect()
    });
    Ok(json!({
        "world_mode": mode,
        "mode": mode.map(mode_name),
        "distributed": matches!(mode, Some(4 | 5)),
        "objects": objects,
        "entities": entities,
        "creature_type": creatures.slot,
        "creatures": creature_rows,
        "events": {
            "total": counts.iter().sum::<u64>(),
            "by_type": by_type,
        },
        "traffic": traffic.map(|t| json!({
            "sent": t.sent,
            "sent_bytes": t.sent_bytes,
            "received": t.received,
            "received_bytes": t.received_bytes,
            "largest": t.largest,
        })),
    }))
}

/// Registers `net.replication`. `traffic` answers the datagram counters
/// (`None` when the host has no socket layer). `creatures` reads the
/// active definition and live bindings at the same boundary as the objects.
pub fn install(
    agent: &Agent,
    traffic: impl Fn() -> Option<Traffic> + Send + 'static,
    creatures: impl Fn(&dyn GuestMemory) -> CreatureReplication + Send + 'static,
) {
    agent.add_game_method(METHOD, Access::Read, move |p, ctx| {
        if !p.is_null() && !p.as_object().is_some_and(|o| o.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "net.replication takes no parameters",
            ));
        }
        let traffic = traffic();
        ctx.with_guest(|m| {
            reply(&*m, &EVENTS, traffic, creatures(&*m)).map_err(|e| {
                RpcError::new(
                    code::UNAVAILABLE,
                    format!("reading the simulation at {:#x}", e.addr),
                )
            })
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{testing::FakeGuest, view::fake};

    const SIM: u32 = 0x9000;
    const POOL: u32 = 0x10000;
    const BODIES: u32 = 0x40000;

    /// A guest with the simulation globals in `mode` and an empty pool.
    fn guest(mode: u32) -> (FakeGuest, Datums<Header>) {
        let mut m = FakeGuest::new();
        m.map(SIM, 0x40);
        m.put_u32(guest_layout::points::SIM_GLOBALS.base, SIM)
            .put_u32(SIM + SIM_MODE, mode);
        let pool = fake::datums::<Header>(&mut m, POOL, 16);
        m.put_u32(guest_layout::points::OBJECTS.base, pool.array.addr);
        (m, pool)
    }

    /// One more live object of type `kind` with `entity` at body `+0xd4`.
    fn push(m: &mut FakeGuest, pool: &Datums<Header>, slot: u32, kind: u8, entity: u32) -> u32 {
        let (handle, header) = fake::push(m, pool);
        let body = BODIES + slot * 0x200;
        m.map(body, 0x200);
        m.put_u8(header.addr + 0x3, kind)
            .put_u32(header.addr + 0x8, body)
            .put_u32(body + BODY_ENTITY, entity);
        handle.raw()
    }

    #[test]
    fn counts_objects_by_kind_with_and_without_an_entity() {
        let (mut m, pool) = guest(4);
        push(&mut m, &pool, 0, 0, 7);
        push(&mut m, &pool, 1, 0, u32::MAX);
        push(&mut m, &pool, 2, 5, 9);
        push(&mut m, &pool, 3, 200, u32::MAX);
        let r = reply(&m, &EventCounts::new(), None, CreatureReplication::default()).unwrap();
        assert_eq!(r["world_mode"], 4);
        assert_eq!(r["mode"], "dist-server");
        assert_eq!(r["distributed"], true);
        assert_eq!(r["objects"]["bipd"]["objects"], 2);
        assert_eq!(r["objects"]["bipd"]["with_entity"], 1);
        assert_eq!(r["objects"]["proj"]["with_entity"], 1);
        assert_eq!(r["objects"]["other"]["objects"], 1);
        assert_eq!(r["entities"], 2);
        assert!(r["traffic"].is_null());
    }

    #[test]
    fn reports_no_mode_and_no_objects_before_the_simulation_exists() {
        let mut m = FakeGuest::new();
        m.put_u32(guest_layout::points::SIM_GLOBALS.base, 0)
            .put_u32(guest_layout::points::OBJECTS.base, 0);
        let r = reply(&m, &EventCounts::new(), None, CreatureReplication::default()).unwrap();
        assert!(r["world_mode"].is_null());
        assert!(r["mode"].is_null());
        assert_eq!(r["distributed"], false);
        assert_eq!(r["entities"], 0);
        assert!(r["objects"].as_object().unwrap().is_empty());
    }

    #[test]
    fn a_local_game_is_not_distributed() {
        let (m, _) = guest(1);
        let r = reply(&m, &EventCounts::new(), None, CreatureReplication::default()).unwrap();
        assert_eq!(r["mode"], "local");
        assert_eq!(r["distributed"], false);
    }

    #[test]
    fn event_counts_are_cumulative_and_named() {
        let (m, _) = guest(4);
        let events = EventCounts::new();
        events.count(12);
        events.count(12);
        events.count(22);
        let r = reply(&m, &events, None, CreatureReplication::default()).unwrap();
        assert_eq!(r["events"]["total"], 3);
        let rows = r["events"]["by_type"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["type"], 12);
        assert_eq!(rows[0]["name"], "weapon fire");
        assert_eq!(rows[0]["count"], 2);
        assert_eq!(rows[1]["name"], "grenade release");
        events.count(12);
        let r = reply(&m, &events, None, CreatureReplication::default()).unwrap();
        assert_eq!(r["events"]["by_type"][0]["count"], 3);
    }

    #[test]
    fn unknown_event_types_land_in_other() {
        let (m, _) = guest(5);
        let events = EventCounts::new();
        events.count(26);
        events.count(u32::MAX);
        let r = reply(&m, &events, None, CreatureReplication::default()).unwrap();
        let rows = r["events"]["by_type"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["name"], "other");
        assert!(rows[0]["type"].is_null());
        assert_eq!(rows[0]["count"], 2);
    }

    #[test]
    fn creature_entities_report_objects_and_updates() {
        let (mut m, pool) = guest(5);
        let object = push(&mut m, &pool, 0, ObjectType::Creature as u8, 0x401);
        let creatures = CreatureReplication {
            slot: Some(17),
            entities: vec![CreatureEntity {
                entity: 0x401,
                object,
                updates: 3,
            }],
        };
        let r = reply(&m, &EventCounts::new(), None, creatures).unwrap();
        assert_eq!(r["objects"]["crea"]["with_entity"], 1);
        assert_eq!(r["creature_type"], 17);
        assert_eq!(r["creatures"], json!([{
            "entity": 0x401,
            "type": 17,
            "object": format!("0x{object:08x}"),
            "updates": 3,
        }]));

        let control = reply(&m, &EventCounts::new(), None, CreatureReplication::default()).unwrap();
        assert!(control["creature_type"].is_null());
        assert_eq!(control["creatures"], json!([]));
    }

    #[test]
    fn traffic_is_passed_through() {
        let (m, _) = guest(4);
        let t = Traffic {
            sent: 3,
            sent_bytes: 400,
            received: 2,
            received_bytes: 90,
            largest: 300,
        };
        let r = reply(&m, &EventCounts::new(), Some(t), CreatureReplication::default()).unwrap();
        assert_eq!(r["traffic"]["sent_bytes"], 400);
        assert_eq!(r["traffic"]["largest"], 300);
    }

    #[test]
    fn the_method_is_registered() {
        use agent_proto::Object;
        let a = Agent::new();
        install(&a, || None, |_| CreatureReplication::default());
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let conn = a.connect(tx);
        struct NoGuest;
        impl crate::Reader for NoGuest {
            fn status(&mut self) -> crate::GameStatus {
                crate::GameStatus::default()
            }
            fn objects(&mut self) -> Result<Vec<Object>, String> {
                Ok(Vec::new())
            }
        }
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains(METHOD), "{line}");
    }
}
