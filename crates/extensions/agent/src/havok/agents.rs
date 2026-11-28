use std::collections::HashMap;

use agent_proto::havok::{
    AgentCache, Agents, AgentsParams, ChildCache, CollisionAgent, ConvexCache, PairSet,
    PairSetParams, Side, TreeCache,
};
use agent_proto::{RpcError, code};
use agent_proto::raw::to_hex;
use live_telemetry::havok_agents as hk;
use serde_json::Value;

use super::{hex, ids, objects, params, read, resolve};
use crate::exec::GameCtx;
use crate::sim::SimHandle;

pub(super) fn agents_method(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: AgentsParams = params(v)?;
    let objects = objects(ctx)?;
    let (datum, _) = resolve(&objects, &p.object)?;
    let raw = read(ctx, |m| hk::agents(m, datum))?;
    let ids = ids(&objects);
    let answer = Agents {
        object: p.object,
        agents: raw.into_iter().map(|a| pair(a, &ids)).collect(),
    };
    Ok(serde_json::to_value(answer).expect("collision agents serialise"))
}

fn hex_word(name: &str, v: &str) -> Result<u32, RpcError> {
    if v.len() != 8 {
        return Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("{name} must be eight hex digits"),
        ));
    }
    u32::from_str_radix(v, 16).map_err(|_| {
        RpcError::new(code::INVALID_PARAMS, format!("{name} must be eight hex digits"))
    })
}

pub(super) fn pair_set_method(
    v: Value,
    ctx: &mut GameCtx<'_>,
    sim: &SimHandle,
) -> Result<Value, RpcError> {
    let p: PairSetParams = params(v)?;
    let entry = p.entry.as_deref().map(|e| hex_word("entry", e)).transpose()?;
    let word = hex_word("word", &p.word)?;
    sim.writable_session(ctx)?;
    let objects = objects(ctx)?;
    let (datum, _) = resolve(&objects, &p.object)?;
    let fault = |e: guest_layout::MemFault| {
        RpcError::new(
            code::UNAVAILABLE,
            format!("the Havok world at {:08x} faulted", e.addr),
        )
    };
    let written = ctx.with_guest(|m| {
        let Some(entry) = (match entry {
            Some(e) => Some(e),
            None => hk::fixed_pair(&*m, datum).map_err(fault)?,
        }) else {
            return Ok(None);
        };
        Ok(hk::set_pair_word(m, datum, entry, word)
            .map_err(fault)?
            .map(|before| (entry, before)))
    })?;
    let (entry, before) = written.ok_or_else(|| {
        let which = p
            .entry
            .as_deref()
            .map_or("against the level".to_owned(), |e| format!("at {e}"));
        RpcError::new(
            code::GONE,
            format!("{} has no collision pair {which}", p.object),
        )
    })?;
    let answer = PairSet {
        object: p.object,
        entry: hex(entry),
        before: hex(before),
        after: hex(word),
    };
    Ok(serde_json::to_value(answer).expect("pair write serialises"))
}

fn cache(c: hk::Cache) -> AgentCache {
    AgentCache {
        base: hex(c.base),
        vtable: hex(c.vtable),
        manager: hex(c.manager),
        kind: c.kind.to_owned(),
        hex: to_hex(&c.bytes),
        convex: c.convex.map(|v| ConvexCache {
            vertices_a: v.vertices_a,
            vertices_b: v.vertices_b,
            contact_ids: v.contact_ids,
            separating_axis: v.separating_axis,
            separating_distance: v.separating_distance,
            swept_sides: v.swept_sides,
        }),
    }
}

fn pair(p: hk::Pair, ids: &HashMap<u32, String>) -> CollisionAgent {
    CollisionAgent {
        island: hex(p.island),
        entry: hex(p.entry),
        collidables: p.collidables.map(hex),
        object_side: ["a", "b"][p.object_side].to_owned(),
        entity: hex(p.entity),
        other: p.other_entity.map(|e| Side {
            entity: hex(e),
            datum: p.other_datum.map(hex),
            object: p.other_datum.and_then(|d| ids.get(&d).cloned()),
            fixed: p.other_fixed,
        }),
        pair_word: hex(p.pair_word),
        cache: cache(p.cache),
        tree: p.tree.map(|t| TreeCache {
            min: t.min,
            max: t.max,
            children: t
                .children
                .into_iter()
                .map(|c| ChildCache {
                    entry: hex(c.entry),
                    key: hex(c.key),
                    word_04: hex(c.word_04),
                    surface: c.surface,
                    cache: cache(c.cache),
                })
                .collect(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn havok_agents_preserves_dispatch_order_object_id_and_byte_order() {
        let raw = hk::Pair {
            island: 0x8100,
            entry: 0x8200,
            collidables: [0xab0c, 0xcd0c],
            object_side: 1,
            entity: 0xcd00,
            other_entity: Some(0xab00),
            other_datum: Some(7),
            other_fixed: false,
            pair_word: u32::MAX,
            cache: hk::Cache {
                base: 0x8300,
                vtable: 0x1234,
                manager: 0x5678,
                kind: "other",
                bytes: vec![0x34, 0x12, 0xaa],
                convex: None,
            },
            tree: None,
        };
        let ids = HashMap::from([(7, "vehicle.00000007".to_owned())]);
        let wire = pair(raw, &ids);
        let value = serde_json::to_value(&wire).unwrap();
        assert_eq!(value["object_side"], "b");
        assert_eq!(value["collidables"], json!(["0000ab0c", "0000cd0c"]));
        assert_eq!(value["other"]["object"], "vehicle.00000007");
        assert_eq!(value["cache"]["base"], "00008300");
        assert_eq!(value["cache"]["hex"], "3412aa");
        assert_eq!(
            serde_json::from_value::<CollisionAgent>(value).unwrap(),
            wire
        );
    }

    #[test]
    fn registered_read_resolves_a_player_and_stamps_the_agent_list() {
        use crate::{Agent, GameStatus, Reader};
        use agent_proto::{Kind, Object};
        use guest_layout::testing::FakeGuest;

        struct Game(FakeGuest);
        impl Reader for Game {
            fn status(&mut self) -> GameStatus {
                GameStatus {
                    tick: Some(7),
                    ..Default::default()
                }
            }

            fn objects(&mut self) -> Result<Vec<Object>, String> {
                Ok(vec![Object {
                    id: "player.0".into(),
                    kind: Kind::Player,
                    unit: Some("e0010001".into()),
                    ..Default::default()
                }])
            }

            fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn guest_layout::GuestMemory)) -> bool {
                f(&mut self.0);
                true
            }
        }

        let mut game = Game(FakeGuest::new());
        game.0.put_u32(live_telemetry::layout::HAVOK_WORLD_PTR, 0);
        let agent = Agent::new();
        crate::havok::install(&agent);
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let conn = agent.connect(tx);
        let req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "havok.agents",
            "params": {"object": "player.0"},
        });
        agent.submit(conn, &req.to_string());
        agent.between_slices(&mut game, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["result"]["at"]["tick"], 7, "{reply}");
        let answer: Agents = serde_json::from_value(reply["result"].clone()).unwrap();
        assert_eq!(answer.object, "player.0");
        assert!(answer.agents.is_empty());
    }

    #[test]
    fn pair_set_takes_eight_hex_digit_entry_and_word() {
        assert_eq!(hex_word("word", "fffffffd").unwrap(), 0xffff_fffd);
        for bad in ["", "fffd", "-0000003", "fffffffdd", "zzzzzzzz"] {
            assert_eq!(
                hex_word("word", bad).unwrap_err().code,
                agent_proto::code::INVALID_PARAMS,
                "{bad}"
            );
        }
        assert!(
            params::<PairSetParams>(json!({"object": "player.0", "entry": "7fbfa8d0"})).is_err()
        );
        let level = params::<PairSetParams>(json!({"object": "player.0", "word": "fffffffd"}));
        assert_eq!(level.unwrap().entry, None);
    }

    #[test]
    fn havok_agents_requires_an_object_selector() {
        assert_eq!(
            params::<AgentsParams>(json!({})).unwrap_err().code,
            agent_proto::code::INVALID_PARAMS
        );
        assert!(params::<AgentsParams>(json!({"object": 12})).is_err());
        assert!(params::<AgentsParams>(json!({"object": "player.0", "limit": 2})).is_err());
        let p = params::<AgentsParams>(json!({"object": "player.0"})).unwrap();
        assert_eq!(p.object, "player.0");
    }
}
