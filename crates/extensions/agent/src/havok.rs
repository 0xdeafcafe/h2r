//! `havok.*` methods (docs/runtime/agent-api.md, "havok"):
//! bodies, contact constraints and collision-agent caches, with
//! bodies named by the agent ids of their objects, and `havok.pair.set`,
//! which overwrites one pair's word. Game methods, answered at a tick boundary.

use std::collections::HashMap;

use agent_proto::havok::{
    Bodies, BodiesParams, Body, Contact, Contacts, ContactsParams, Digest, DigestParams,
    IslandFallback, Islands, TakeoverArm, IslandsParams, Point, Shape, Side, method,
};
use agent_proto::{Kind, Object, ObjectId, RpcError, code};
use live_telemetry::havok::{self as hk, IslandState};
use live_telemetry::havok_agents::digest as hk_digest;
use serde_json::{Value, json};

use crate::exec::GameCtx;
use crate::{Access, Agent};

mod agents;

/// Registers the read-only `havok.*` methods on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::AGENTS, Access::Read, agents::agents_method);
    agent.add_game_method(method::BODIES, Access::Read, bodies_method);
    agent.add_game_method(method::CONTACTS, Access::Read, contacts_method);
    agent.add_game_method(method::DIGEST, Access::Read, digest_method);
    agent.add_game_method(method::ISLANDS, Access::Read, |v, _| islands_method(v));
}

/// Registers `havok.pair.set`, using sim's host-write checks.
pub fn install_writes(agent: &Agent, sim: &crate::sim::SimHandle) {
    let sim = sim.clone();
    agent.add_game_method(method::PAIR_SET, Access::Claimed, move |v, ctx| {
        agents::pair_set_method(v, ctx, &sim)
    });
}

fn params<T: serde::de::DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    let v = if v.is_null() { json!({}) } else { v };
    serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
}

fn hex(v: u32) -> String {
    format!("{v:08x}")
}

/// The object datum behind an API object: a player's unit, else the id's.
fn datum_of(o: &Object) -> Option<u32> {
    match o.kind {
        Kind::Player => o
            .unit
            .as_deref()
            .and_then(|u| u32::from_str_radix(u, 16).ok()),
        _ => ObjectId::parse(&o.id).map(|id| id.key),
    }
}

/// Agent ids by object datum.
fn ids(objects: &[Object]) -> HashMap<u32, String> {
    objects
        .iter()
        .filter_map(|o| datum_of(o).map(|d| (d, o.id.clone())))
        .collect()
}

/// The datum an id names, among the listed objects.
pub(crate) fn resolve(objects: &[Object], id: &str) -> Result<(u32, Option<[f64; 3]>), RpcError> {
    if ObjectId::parse(id).is_none() {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("{id} is not an id (player.N or kind.xxxxxxxx)"),
        ));
    }
    let o = objects
        .iter()
        .find(|o| o.id == id)
        .ok_or_else(|| RpcError::new(code::GONE, format!("{id} is not in the world")))?;
    let d = datum_of(o)
        .ok_or_else(|| RpcError::new(code::GONE, format!("{id} has no object (between lives)")))?;
    Ok((d, o.pos))
}

fn read<T>(
    ctx: &mut GameCtx<'_>,
    f: impl Fn(&dyn guest_layout::GuestMemory) -> Result<T, guest_layout::MemFault>,
) -> Result<T, RpcError> {
    let mut out = None;
    let ran = ctx.reader.with_guest(&mut |m| out = Some(f(&*m)));
    match out {
        Some(r) if ran => r.map_err(|e| {
            RpcError::new(
                code::UNAVAILABLE,
                format!("reading the Havok world at {:08x}: {e:?}", e.addr),
            )
        }),
        _ => Err(RpcError::new(
            code::UNAVAILABLE,
            "no guest memory here (the game isn't running)",
        )),
    }
}

fn objects(ctx: &mut GameCtx<'_>) -> Result<Vec<Object>, RpcError> {
    ctx.reader
        .objects()
        .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("reading the world: {e}")))
}

fn bodies_method(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: BodiesParams = params(v)?;
    let raw = read(ctx, hk::bodies)?;
    let objects = objects(ctx)?;
    Ok(serde_json::to_value(answer_bodies(&raw, &objects, &p)?).expect("bodies serialise"))
}

fn contacts_method(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: ContactsParams = params(v)?;
    let raw = read(ctx, hk::contacts)?;
    let bodies = read(ctx, hk::bodies)?;
    let objects = objects(ctx)?;
    Ok(
        serde_json::to_value(answer_contacts(&raw, &bodies, &objects, &p)?)
            .expect("contacts serialise"),
    )
}

fn digest_method(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let DigestParams {} = params(v)?;
    let raw = read(ctx, hk_digest::read)?;
    Ok(serde_json::to_value(answer_digest(&raw)).expect("digest serialises"))
}

fn islands_method(v: Value) -> Result<Value, RpcError> {
    let IslandsParams {} = params(v)?;
    let (by_reason, solved) = sim_havok::island_solve::fallbacks();
    let islands = Islands {
        solved,
        declined: by_reason.iter().map(|(_, n)| n).sum(),
        fallbacks: fallbacks(&by_reason),
        integrate: takeover_arm(&engine::sim_physics::parity::ISLAND_INTEGRATE, &sim_havok::island_integrate::fallbacks()),
        deactivation: takeover_arm(&engine::sim_physics::parity::DEACTIVATION, &sim_havok::deactivation::fallbacks()),
        phase: takeover_arm(&engine::sim_physics::parity::INTEGRATE_PHASE, &sim_havok::integrate_phase::fallbacks()),
        polygon: takeover_arm(&engine::sim_physics::parity::STRUCTURE_POLYGON, &sim_havok::structure::fallbacks()),
        convex: takeover_arm(&engine::sim_physics::parity::CONVEX_AGENT, &sim_havok::convex::fallbacks()),
        query: takeover_arm(&engine::sim_physics::parity::CONVEX_QUERY, &sim_havok::closest::fallbacks()),
        point: takeover_arm(&engine::sim_physics::parity::CONTACT_POINT, &sim_havok::contact_point::fallbacks()),
        commit: takeover_arm(&engine::sim_physics::parity::CONTACT_COMMIT, &sim_havok::commit::fallbacks()),
    };
    Ok(serde_json::to_value(islands).expect("islands serialise"))
}

/// One answering arm's counts, with its model's declines by reason.
fn takeover_arm(arm: &engine::sim_physics::parity::Arm, by_reason: &[(&str, u64)]) -> TakeoverArm {
    use engine::sim_physics::parity;
    let (.., declined) = parity::status().into_iter().find(|s| s.0 == arm.name).expect("the arm is in the table");
    let takeover = parity::takeover().into_iter().find(|t| t.name == arm.name).expect("the arm answers");
    TakeoverArm { answered: takeover.answered, declined, fallbacks: fallbacks(by_reason) }
}

/// Counters by reason, on the wire.
fn fallbacks(by_reason: &[(&str, u64)]) -> Vec<IslandFallback> {
    by_reason.iter().map(|&(reason, count)| IslandFallback { reason: reason.to_owned(), count }).collect()
}

pub(crate) fn answer_digest(raw: &hk_digest::Digest) -> Digest {
    Digest {
        digest: format!("{:016x}", raw.hash),
        bodies: raw.bodies,
        constraints: raw.constraints,
        pairs: raw.pairs,
        heap_used: raw.heap_used,
    }
}

fn wire_body(b: &hk::Body, ids: &HashMap<u32, String>) -> Body {
    Body {
        entity: hex(b.entity),
        state: b.state.as_str().to_owned(),
        object: b.object.and_then(|d| ids.get(&d).cloned()),
        datum: b.object.map(hex),
        pos: b.pos,
        velocity: b.velocity,
        angular_velocity: b.angular_velocity,
        inv_mass: b.inv_mass,
        mass: (b.inv_mass > 0.0).then(|| 1.0 / b.inv_mass),
        friction: b.friction,
        restitution: b.restitution,
        distance: None,
        shape: b.shape.as_ref().map(wire_shape),
    }
}

fn wire_shape(s: &hk::Shape) -> Shape {
    let mut out = Shape {
        addr: hex(s.addr),
        vtable: hex(s.vtable),
        ..Default::default()
    };
    let kind = match &s.kind {
        hk::ShapeKind::Capsule { a, b, radius } => {
            out.a = Some(*a);
            out.b = Some(*b);
            out.radius = Some(*radius);
            out.height = Some(dist(*a, *b) + 2.0 * radius);
            "capsule"
        }
        hk::ShapeKind::Sphere { radius } => {
            out.radius = Some(*radius);
            "sphere"
        }
        hk::ShapeKind::Transform {
            translation,
            child,
            child_shape,
        } => {
            out.translation = Some(*translation);
            out.child = Some(Box::new(match child_shape {
                Some(c) => wire_shape(c),
                // A child that did not read: its address only.
                None => Shape {
                    addr: hex(*child),
                    kind: "unread".into(),
                    ..Default::default()
                },
            }));
            "transform"
        }
        hk::ShapeKind::Other => "other",
    };
    out.kind = kind.to_owned();
    out
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f64>()
        .sqrt()
}

/// `havok.bodies` from what was read.
pub(crate) fn answer_bodies(
    raw: &[hk::Body],
    objects: &[Object],
    p: &BodiesParams,
) -> Result<Bodies, RpcError> {
    let limit = p.limit.unwrap_or(BodiesParams::LIMIT);
    if !(1..=BodiesParams::MAX_LIMIT).contains(&limit) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("limit must be 1 to {}", BodiesParams::MAX_LIMIT),
        ));
    }
    if p.radius.is_some() && p.near.is_none() {
        return Err(RpcError::new(code::INVALID_PARAMS, "radius needs near"));
    }
    let ids = ids(objects);
    let mut out: Vec<Body> = raw.iter().map(|b| wire_body(b, &ids)).collect();
    if let Some(near) = &p.near {
        let (datum, pos) = resolve(objects, near)?;
        // Its own body when it has one: where the physics has it.
        let centre = raw
            .iter()
            .find(|b| b.object == Some(datum))
            .map(|b| b.pos)
            .or(pos)
            .ok_or_else(|| RpcError::new(code::GONE, format!("{near} has no position")))?;
        let radius = p.radius.unwrap_or(BodiesParams::RADIUS);
        for b in &mut out {
            b.distance = Some(dist(b.pos, centre));
        }
        // The level's fixed body sits at its origin, not near anything.
        out.retain(|b| b.distance.is_some_and(|d| d <= radius) && b.datum.is_some());
        out.sort_by(|a, b| a.distance.partial_cmp(&b.distance).expect("finite"));
    }
    let more = out.len() > limit;
    out.truncate(limit);
    Ok(Bodies {
        total: raw.len(),
        bodies: out,
        more,
    })
}

fn side(entity: u32, by_entity: &HashMap<u32, &hk::Body>, ids: &HashMap<u32, String>) -> Side {
    let b = by_entity.get(&entity);
    let datum = b.and_then(|b| b.object);
    Side {
        entity: hex(entity),
        object: datum.and_then(|d| ids.get(&d).cloned()),
        datum: datum.map(hex),
        fixed: b.is_some_and(|b| b.state == IslandState::Fixed),
    }
}

/// `havok.contacts` from what was read: `bodies` names the entities.
pub(crate) fn answer_contacts(
    raw: &[hk::Contact],
    bodies: &[hk::Body],
    objects: &[Object],
    p: &ContactsParams,
) -> Result<Contacts, RpcError> {
    let ids = ids(objects);
    let by_entity: HashMap<u32, &hk::Body> = bodies.iter().map(|b| (b.entity, b)).collect();
    let datum = match &p.object {
        Some(id) => Some(resolve(objects, id)?.0),
        None => None,
    };
    let owner = |e: u32| by_entity.get(&e).and_then(|b| b.object);
    let mut out = Vec::new();
    for c in raw {
        let (a, b, flip) = match datum {
            None => (c.a, c.b, false),
            Some(d) if owner(c.a) == Some(d) => (c.a, c.b, false),
            Some(d) if owner(c.b) == Some(d) => (c.b, c.a, true),
            Some(_) => continue,
        };
        let sign = if flip { -1.0 } else { 1.0 };
        out.push(Contact {
            a: side(a, &by_entity, &ids),
            b: side(b, &by_entity, &ids),
            points: c
                .points
                .iter()
                .map(|q| Point {
                    pos: q.pos,
                    normal: q.normal.map(|n| n * sign),
                    distance: q.distance,
                    impulse: q.impulse,
                    friction: q.friction,
                    restitution: q.restitution,
                })
                .collect(),
        });
    }
    Ok(Contacts { contacts: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn islands_report_the_solve_counters_by_reason() {
        use sim_havok::island_solve::{self as island, Fallback};
        let read = || serde_json::from_value::<Islands>(islands_method(Value::Null).unwrap()).unwrap();
        let before = read();
        island::count(Fallback::Overflow);
        island::count_solved();
        let after = read();
        let reasons: Vec<_> = after.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "overflow", "motion", "constraint", "tables", "solver", "float"]);
        assert!(after.fallbacks[1].count > before.fallbacks[1].count);
        assert!(after.solved > before.solved);
        assert_eq!(after.declined, after.fallbacks.iter().map(|f| f.count).sum::<u64>());
        let reasons: Vec<_> = after.integrate.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "action", "motion", "float", "solve", "monitor", "unobserved"]);
        sim_havok::island_integrate::count(sim_havok::island_integrate::Fallback::Action);
        assert!(read().integrate.fallbacks[1].count > after.integrate.fallbacks[1].count);
        let reasons: Vec<_> = after.deactivation.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "deactivator", "object", "float", "monitor", "unobserved"]);
        let reasons: Vec<_> = after.phase.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "pending", "deactivate", "listener", "split", "island", "float", "monitor", "unobserved"]);
        let reasons: Vec<_> = after.polygon.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "ring", "float", "unobserved"]);
        let reasons: Vec<_> = after.convex.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "closest", "points", "float", "monitor", "unobserved"]);
        let reasons: Vec<_> = after.query.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "shape", "float", "monitor", "unobserved"]);
        let reasons: Vec<_> = after.point.fallbacks.iter().map(|f| f.reason.as_str()).collect();
        assert_eq!(reasons, ["memory", "commit", "float", "unobserved"]);
        sim_havok::integrate_phase::count(sim_havok::integrate_phase::Fallback::Listener);
        assert!(read().phase.fallbacks[3].count > after.phase.fallbacks[3].count);
        assert_eq!(islands_method(json!({"reset": true})).unwrap_err().code, code::INVALID_PARAMS);
    }

    fn body(entity: u32, state: IslandState, object: Option<u32>, pos: [f64; 3]) -> hk::Body {
        hk::Body {
            entity,
            state,
            object,
            pos,
            velocity: [0.0, 0.0, 11.0],
            angular_velocity: [0.0; 3],
            inv_mass: 0.0125,
            friction: 0.6,
            restitution: 0.1,
            shape: None,
        }
    }

    fn world() -> (Vec<hk::Body>, Vec<Object>) {
        let bodies = vec![
            body(
                0x0080_1000,
                IslandState::Active,
                Some(0xe001_0001),
                [0.0, 0.0, 1.0],
            ),
            body(
                0x0080_2000,
                IslandState::Inactive,
                Some(0xe002_0002),
                [3.0, 0.0, 1.0],
            ),
            body(
                0x0080_3000,
                IslandState::Active,
                Some(0xe003_0003),
                [9.0, 0.0, 1.0],
            ),
            body(0x0080_4000, IslandState::Fixed, None, [0.0; 3]),
        ];
        let objects = vec![
            Object {
                id: "player.0".into(),
                kind: Kind::Player,
                unit: Some("e0010001".into()),
                pos: Some([0.0, 0.0, 1.2]),
                ..Default::default()
            },
            Object {
                id: "vehicle.e0020002".into(),
                kind: Kind::Vehicle,
                ..Default::default()
            },
        ];
        (bodies, objects)
    }

    #[test]
    fn bodies_carry_object_ids_and_mass() {
        let (raw, objects) = world();
        let r = answer_bodies(&raw, &objects, &BodiesParams::default()).unwrap();
        assert_eq!(r.total, 4);
        assert_eq!(r.bodies.len(), 4);
        let b = &r.bodies[0];
        assert_eq!(b.entity, "00801000");
        assert_eq!(b.state, "active");
        assert_eq!(b.object.as_deref(), Some("player.0"));
        assert_eq!(b.datum.as_deref(), Some("e0010001"));
        assert_eq!(b.mass, Some(80.0));
        assert_eq!(r.bodies[1].object.as_deref(), Some("vehicle.e0020002"));
        // A datum the API doesn't list keeps its datum, no id.
        assert_eq!(r.bodies[2].object, None);
        assert_eq!(r.bodies[2].datum.as_deref(), Some("e0030003"));
        assert_eq!(r.bodies[3].state, "fixed");
        assert_eq!(r.bodies[3].datum, None);
    }

    #[test]
    fn bodies_carry_their_shape() {
        let (mut raw, objects) = world();
        raw[0].shape = Some(hk::Shape {
            addr: 0x0070_0000,
            vtable: 0x0045_5500,
            kind: hk::ShapeKind::Capsule {
                a: [0.0, 0.0, 0.175],
                b: [0.0, 0.0, 0.375],
                radius: 0.175,
            },
        });
        raw[1].shape = Some(hk::Shape {
            addr: 0x0070_1000,
            vtable: 0x0045_5498,
            kind: hk::ShapeKind::Transform {
                translation: [0.0, 0.0, 0.3],
                child: 0x0070_2000,
                child_shape: None,
            },
        });
        let r = answer_bodies(&raw, &objects, &BodiesParams::default()).unwrap();
        let s = r.bodies[0].shape.as_ref().unwrap();
        assert_eq!(
            (s.addr.as_str(), s.vtable.as_str()),
            ("00700000", "00455500")
        );
        assert_eq!(s.kind, "capsule");
        assert_eq!(s.radius, Some(0.175));
        // 0.2 between the ends, 0.35 of caps: the crouching pill's 0.55.
        assert!((s.height.unwrap() - 0.55).abs() < 1e-9);
        let t = r.bodies[1].shape.as_ref().unwrap();
        assert_eq!(t.kind, "transform");
        assert_eq!(t.translation, Some([0.0, 0.0, 0.3]));
        let c = t.child.as_ref().unwrap();
        assert_eq!((c.addr.as_str(), c.kind.as_str()), ("00702000", "unread"));
        assert!(r.bodies[2].shape.is_none());
        let v = serde_json::to_value(&r.bodies[2]).unwrap();
        assert!(v.get("shape").is_none(), "{v}");
    }

    #[test]
    fn near_keeps_bodies_in_radius_nearest_first_and_limit_cuts() {
        let (raw, objects) = world();
        let p = BodiesParams {
            near: Some("player.0".into()),
            radius: Some(5.0),
            limit: None,
        };
        let r = answer_bodies(&raw, &objects, &p).unwrap();
        let got: Vec<_> = r.bodies.iter().map(|b| b.entity.as_str()).collect();
        // The level's fixed body is left out; the far crate too.
        assert_eq!(got, ["00801000", "00802000"]);
        assert_eq!(r.bodies[1].distance, Some(3.0));
        let p = BodiesParams {
            limit: Some(1),
            ..p
        };
        let r = answer_bodies(&raw, &objects, &p).unwrap();
        assert_eq!(r.bodies.len(), 1);
        assert!(r.more);
    }

    #[test]
    fn bad_params_are_refused() {
        let (raw, objects) = world();
        let e = |p: BodiesParams| answer_bodies(&raw, &objects, &p).unwrap_err().code;
        assert_eq!(
            e(BodiesParams {
                radius: Some(1.0),
                ..Default::default()
            }),
            code::INVALID_PARAMS
        );
        assert_eq!(
            e(BodiesParams {
                limit: Some(0),
                ..Default::default()
            }),
            code::INVALID_PARAMS
        );
        assert_eq!(
            e(BodiesParams {
                near: Some("player.4".into()),
                ..Default::default()
            }),
            code::GONE
        );
        assert_eq!(
            e(BodiesParams {
                near: Some("nonsense".into()),
                ..Default::default()
            }),
            code::INVALID_PARAMS
        );
    }

    #[test]
    fn contacts_for_an_object_put_it_first_and_flip_the_normal() {
        let (bodies, objects) = world();
        let point = hk::Point {
            pos: [0.0, 0.0, 0.8],
            normal: [0.0, 0.0, -1.0],
            distance: -0.1,
            impulse: 4.0,
            friction: 0.5,
            restitution: 0.0,
        };
        let raw = vec![
            // The level (a) against the player (b).
            hk::Contact {
                constraint: 0x0090_0000,
                a: 0x0080_4000,
                b: 0x0080_1000,
                points: vec![point.clone()],
            },
            // The crate against the far body: not the player's.
            hk::Contact {
                constraint: 0x0090_1000,
                a: 0x0080_2000,
                b: 0x0080_3000,
                points: vec![point],
            },
        ];
        let all = answer_contacts(&raw, &bodies, &objects, &ContactsParams::default()).unwrap();
        assert_eq!(all.contacts.len(), 2);
        assert!(all.contacts[0].a.fixed);

        let p = ContactsParams {
            object: Some("player.0".into()),
        };
        let mine = answer_contacts(&raw, &bodies, &objects, &p).unwrap();
        assert_eq!(mine.contacts.len(), 1);
        let c = &mine.contacts[0];
        assert_eq!(c.a.object.as_deref(), Some("player.0"));
        assert_eq!(c.b.entity, "00804000");
        assert!(c.b.fixed && c.b.datum.is_none());
        assert_eq!(c.points[0].normal, [0.0, 0.0, 1.0]);
        assert_eq!(c.points[0].distance, -0.1);
        assert_eq!(c.points[0].impulse, 4.0);
    }

    #[test]
    fn the_digest_answer_is_sixteen_hex_digits_and_the_walk_counts() {
        let raw = hk_digest::Digest {
            hash: 0xbeef,
            bodies: 3,
            constraints: 2,
            pairs: 1,
            heap_used: 0x40,
        };
        let d = answer_digest(&raw);
        assert_eq!(d.digest, "000000000000beef");
        assert_eq!((d.bodies, d.constraints, d.pairs, d.heap_used), (3, 2, 1, 0x40));
        assert_eq!(params::<DigestParams>(Value::Null).unwrap(), DigestParams {});
        let e = params::<DigestParams>(json!({"near": "player.0"})).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }

    #[test]
    fn the_methods_are_registered() {
        let a = Agent::new();
        install(&a);
        let sim = crate::sim::install(&a, Box::new(crate::sim::bots::NotLanded), None);
        install_writes(&a, &sim);
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let conn = a.connect(tx);
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#,
        );
        struct NoGuest;
        impl crate::Reader for NoGuest {
            fn status(&mut self) -> crate::GameStatus {
                crate::GameStatus::default()
            }
            fn objects(&mut self) -> Result<Vec<Object>, String> {
                Ok(Vec::new())
            }
        }
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(
            method::ALL.iter().all(|name| line.contains(*name)),
            "{line}"
        );
        // Without guest memory the reads are unavailable, not a panic.
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":2,"method":"havok.bodies","params":{}}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains(&code::UNAVAILABLE.to_string()), "{line}");
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":3,"method":"havok.digest","params":{}}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains(&code::UNAVAILABLE.to_string()), "{line}");
    }
}
