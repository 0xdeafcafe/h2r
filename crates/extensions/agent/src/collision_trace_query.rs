//! A read-only entry J query on the active structure at a tick boundary.

use std::sync::Arc;

use agent_proto::collision::{TraceLocation, TraceParams, TraceResult, method};
use agent_proto::{Boundary, RpcError, code};
use engine::sim_collision::{Bail, CollisionWorld, EPOCH, query};
use runtime::Runtime;
use serde_json::Value;

use crate::{Access, Agent};

fn params(value: Value) -> Result<TraceParams, RpcError> {
    let p: TraceParams = serde_json::from_value(value)
        .map_err(|e| RpcError::new(code::INVALID_PARAMS, format!("collision.trace: {e}")))?;
    if !p.start.iter().chain(&p.delta).all(|x| x.is_finite()) {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            "coordinates must fit a finite f32",
        ));
    }
    Ok(p)
}

fn declined(why: Bail) -> RpcError {
    let mut err = RpcError::new(
        code::UNAVAILABLE,
        format!("native collision query declined: {}", why.name()),
    );
    err.data = Some(serde_json::json!({ "reason": why.name() }));
    err
}

pub fn install(agent: &Agent, runtime: &Arc<Runtime>) {
    let runtime = Arc::downgrade(runtime);
    // Keep probe decodes out of the simulation's map budget and counters.
    let world = CollisionWorld::new(&EPOCH);
    agent.add_game_method(method::TRACE, Access::Read, move |value, ctx| {
        let p = params(value)?;
        if ctx.at.boundary != Boundary::Tick {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "collision.trace needs a game tick",
            ));
        }
        let rt = runtime
            .upgrade()
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "game has shut down"))?;
        let mut mem = rt.memory();
        if mem.has_trap() {
            return Err(declined(Bail::Trap));
        }
        let result = query::trace(
            &mut *mem,
            &world,
            p.start.map(f32::to_bits),
            p.delta.map(f32::to_bits),
            p.flags,
            [u32::MAX; 2],
        )
        .map_err(declined)?;
        let wire = TraceResult {
            hit_type: result.hit_type,
            fraction: f32::from_bits(result.fraction),
            position: result.position.map(f32::from_bits),
            material: result.material,
            plane: result.plane.map(|v| v.map(f32::from_bits)),
            locations: result.locations.map(|loc| TraceLocation {
                leaf: loc.leaf as i32,
                cluster: loc.cluster,
                bsp: loc.bsp,
            }),
            cluster_marks: result.cluster_marks,
            instance_marks: result.instance_marks,
            object_marks: result.object_marks,
        };
        Ok(serde_json::to_value(wire).expect("collision trace serialises"))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GameStatus, Reader};
    use engine::sim_collision::layout;
    use runtime::{Dispatch, Guest, guest::Permissions};
    use serde_json::json;
    use tags::collision::{Bsp, Child, Leaf, Node3d, Plane};

    #[test]
    fn rejects_bad_vectors_and_unknown_fields() {
        for value in [
            json!({"start":[0,1],"delta":[0,0,3],"flags":1}),
            json!({"start":[1e40,1,0],"delta":[0,0,3],"flags":1}),
            json!({"start":[0,1,2],"delta":[0,0,3],"flags":-1}),
            json!({"start":[0,1,2],"delta":[0,0,3],"flags":1,"unknown":42}),
        ] {
            assert_eq!(params(value).unwrap_err().code, code::INVALID_PARAMS);
        }
    }

    #[test]
    fn query_at_tick_reads_one_leaf_and_leaves_guest_bytes_intact() {
        const BASE: u32 = 0x0040_0000;
        const LENGTH: u32 = 0x0010_0000;
        const STRUCTURE: u32 = BASE + 0x1000;
        const BSP: u32 = BASE + 0x2000;
        const LEAVES: u32 = BASE + 0x3000;
        let mut g = Guest::new();
        g.map_zeroed(BASE, LENGTH, Permissions::RW).unwrap();
        let bsp = Bsp {
            nodes: vec![Node3d {
                plane: 0,
                back: Child::Leaf(0),
                front: Child::None,
            }],
            planes: vec![Plane([1.0, 0.0, 0.0, 6.0])],
            leaves: vec![Leaf {
                flags: 0,
                ref_count: 0,
                first_ref: 0xffff,
            }],
            ..Bsp::default()
        };
        g.write_bytes(BSP, &bsp.encode_blocks(BSP), 1).unwrap();
        g.set_u32(layout::STRUCTURE_BSP, BSP).unwrap();
        g.set_u32(0x004e_0340, BSP).unwrap(); // J's active structure collision BSP
        g.set_u32(layout::ACTIVE_STRUCTURE, STRUCTURE).unwrap();
        g.set_u32(STRUCTURE + layout::STRUCTURE_LEAF_COUNT, 1)
            .unwrap();
        g.set_u32(STRUCTURE + layout::STRUCTURE_LEAVES, LEAVES)
            .unwrap();
        g.set_u16(LEAVES, 3).unwrap();
        g.set_u32(layout::ONE, 1.0f32.to_bits()).unwrap();
        g.set_u32(0x0044_aed4, 1.0f32.to_bits()).unwrap(); // J's limit for the structure trace
        g.set_u32(0x004e_d280, BASE + 0x4000).unwrap(); // live breakable mask base
        let bytes = |guest: &Guest| {
            let mut bytes = vec![0; LENGTH as usize];
            guest.read_bytes(BASE, &mut bytes, 1).unwrap();
            bytes
        };
        let before = bytes(&g);
        let rt = Runtime::new(g, Dispatch::default());
        let agent = Agent::new();
        install(&agent, &rt);
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let conn = agent.connect(tx);
        let request = json!({"jsonrpc":"2.0", "id":1, "method":method::TRACE,
                             "params":{"start":[0,0,0],"delta":[1,2,3],"flags":1}});
        agent.submit(conn, &request.to_string());
        struct Tick;
        impl Reader for Tick {
            fn status(&mut self) -> GameStatus {
                GameStatus {
                    tick: Some(7),
                    ..Default::default()
                }
            }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(Vec::new())
            }
        }
        agent.on_tick(&mut Tick);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["at"]["tick"], 7, "{reply}");
        let result: TraceResult = serde_json::from_value(reply["result"].clone()).unwrap();
        assert_eq!(result.hit_type, 0);
        assert_eq!(result.fraction, 1.0);
        assert_eq!(result.position, [1.0, 2.0, 3.0]);
        for loc in result.locations {
            assert_eq!((loc.leaf, loc.cluster, loc.bsp), (0, 3, 0));
        }
        assert!(result.cluster_marks.is_empty());
        assert!(result.instance_marks.is_empty());
        assert!(result.object_marks.is_empty());
        assert!(bytes(&rt.memory()) == before, "trace changed guest RAM");

        rt.memory().set_u32(0x004e_0340, 0x8000_0000).unwrap();
        agent.submit(conn, &request.to_string());
        agent.on_tick(&mut Tick);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["error"]["code"], code::UNAVAILABLE, "{reply}");
        assert_eq!(reply["error"]["data"]["reason"], "fault");
        assert!(reply.get("result").is_none());
    }
}
