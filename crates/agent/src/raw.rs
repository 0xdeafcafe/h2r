//! `world.raw` (docs/runtime/agent-api.md, "world.raw"): an object's body,
//! physics component, Havok entity, motion and shape as raw guest bytes,
//! as `live_telemetry::raw` reads them. Read-only; a game method, answered
//! at a tick boundary. `h2 agent raw-diff` compares two answers.

use agent_proto::raw::{PARTS, Raw, RawParams, RawPart, method, to_hex};
use agent_proto::{RpcError, code};
use live_telemetry::raw::{self as rd, Part};
use serde_json::Value;

use crate::exec::GameCtx;
use crate::{Access, Agent};

/// Registers `world.raw` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::RAW, Access::Read, raw_method);
}

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

/// The parts asked for; all of them without `parts`.
fn parts(p: &RawParams) -> Result<Vec<Part>, RpcError> {
    let Some(names) = &p.parts else {
        return Ok(Part::ALL.to_vec());
    };
    if names.is_empty() {
        return Err(invalid("parts is empty; leave it out for all of them"));
    }
    names
        .iter()
        .map(|n| {
            Part::parse(n).ok_or_else(|| {
                invalid(format!("unknown part {n} (one of {})", PARTS.join(", ")))
            })
        })
        .collect()
}

fn raw_method(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: RawParams = serde_json::from_value(v).map_err(|e| invalid(e.to_string()))?;
    let parts = parts(&p)?;
    if p.len == Some(0) {
        return Err(invalid("len must be at least 1"));
    }
    let objects = ctx
        .reader
        .objects()
        .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("reading the world: {e}")))?;
    let (datum, _) = crate::havok::resolve(&objects, &p.id)?;
    let mut out = None;
    let ran = ctx
        .reader
        .with_guest(&mut |m| out = Some(rd::object_parts(&*m, datum, &parts, p.len)));
    let regions = match out {
        Some(Ok(Some(r))) if ran => r,
        Some(Ok(None)) if ran => {
            return Err(RpcError::new(
                code::GONE,
                format!("{} ({datum:08x}) has no object body", p.id),
            ));
        }
        Some(Err(e)) if ran => {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                format!("reading the objects at {:08x}: {e:?}", e.addr),
            ));
        }
        _ => {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "no guest memory here (the game isn't running)",
            ));
        }
    };
    Ok(serde_json::to_value(answer(&p.id, datum, &regions)).expect("raw serialises"))
}

/// `world.raw` from what was read.
pub(crate) fn answer(id: &str, datum: u32, regions: &[rd::Region]) -> Raw {
    Raw {
        id: id.to_owned(),
        datum: format!("{datum:08x}"),
        parts: regions
            .iter()
            .map(|r| match &r.read {
                Ok((base, bytes)) => RawPart {
                    part: r.part.as_str().to_owned(),
                    base: Some(format!("{base:08x}")),
                    hex: Some(to_hex(bytes)),
                    missing: None,
                },
                Err(why) => RawPart {
                    part: r.part.as_str().to_owned(),
                    missing: Some(why.clone()),
                    ..Default::default()
                },
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_proto::raw::from_hex;
    use agent_proto::{Kind, Object};
    use engine::GuestMemory;
    use engine::testing::FakeGuest;
    use engine::view::{fake, objects};
    use live_telemetry::layout as l;
    use serde_json::json;

    use super::*;
    use crate::{GameStatus, Reader};

    const OBJECTS_HDR: u32 = 0x8010_0000;
    const BODY: u32 = 0x8020_0000;
    const COMPONENTS_HDR: u32 = 0x8030_0000;
    const COMPONENT_DATA: u32 = 0x8031_0000;
    const RIGIDS: u32 = 0x8040_0000;
    const ENTITY: u32 = 0x8041_0000;
    const MOTION: u32 = 0x8042_0000;

    struct Game {
        g: Option<FakeGuest>,
        objects: Vec<Object>,
    }

    impl Reader for Game {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(100),
                ..Default::default()
            }
        }
        fn objects(&mut self) -> Result<Vec<Object>, String> {
            Ok(self.objects.clone())
        }
        fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
            match &mut self.g {
                Some(g) => {
                    f(g);
                    true
                }
                None => false,
            }
        }
    }

    /// Player 0's unit: a `0x464`-byte body, physics component 1, one
    /// rigid body with a motion and no shape.
    fn game() -> Game {
        let mut g = FakeGuest::new();
        g.put_u32(engine::points::OBJECTS.base, OBJECTS_HDR);
        let table = fake::datums::<objects::Header>(&mut g, OBJECTS_HDR, 16);
        let (h, hdr) = fake::push(&mut g, &table);
        g.map(BODY, 0x464);
        hdr.set_body(&mut g, BODY).unwrap();
        g.put_u16(hdr.addr + l::OBJECT_BODY_SIZE, 0x464);
        g.put_u32(BODY + 4, 0xdead_beef);
        g.put_u32(BODY + l::BODY_PHYSICS, 0xab00_0001);
        g.put_u32(l::PHYSICS_COMPONENTS_PTR, COMPONENTS_HDR);
        g.put_u32(COMPONENTS_HDR + l::DATUM_DATA, COMPONENT_DATA);
        let comp = COMPONENT_DATA + l::PHYSICS_COMPONENT_SIZE;
        g.map(COMPONENT_DATA, 2 * l::PHYSICS_COMPONENT_SIZE);
        g.put_u32(comp + l::COMPONENT_BODIES, RIGIDS);
        g.put_u32(comp + l::COMPONENT_BODY_COUNT, 1);
        g.put_u32(RIGIDS + l::COMPONENT_BODY_RIGID, ENTITY);
        g.map(ENTITY, 0x200);
        g.put_u32(ENTITY + l::RIGID_MOTION, MOTION);
        g.map(MOTION, 0x140);
        g.put_f32(MOTION + l::MOTION_POSITION + 8, -3.2);
        Game {
            g: Some(g),
            objects: vec![Object {
                id: "player.0".into(),
                kind: Kind::Player,
                unit: Some(format!("{:08x}", h.raw())),
                ..Default::default()
            }],
        }
    }

    struct Harness {
        agent: Agent,
        conn: crate::ConnId,
        rx: std::sync::mpsc::Receiver<String>,
    }

    impl Harness {
        fn new() -> Harness {
            let agent = Agent::new();
            install(&agent);
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            let conn = agent.connect(tx);
            Harness { agent, conn, rx }
        }

        /// The reply to one call, answered at a boundary of `game`.
        fn call(&self, game: &mut Game, p: Value) -> Value {
            let req = json!({"jsonrpc": "2.0", "id": 1, "method": method::RAW, "params": p});
            self.agent.submit(self.conn, &req.to_string());
            self.agent.between_slices(game, 0);
            let line = self.rx.recv_timeout(Duration::from_secs(5)).unwrap();
            serde_json::from_str(&line).unwrap()
        }
    }

    fn error_code(v: &Value) -> i64 {
        v["error"]["code"].as_i64().unwrap_or_else(|| panic!("{v}"))
    }

    #[test]
    fn answers_every_part_with_hex_and_a_reason_for_the_missing_one() {
        let h = Harness::new();
        let mut game = game();
        let v = h.call(&mut game, json!({"id": "player.0"}));
        let r: Raw = serde_json::from_value(v["result"].clone()).unwrap();
        assert_eq!(r.id, "player.0");
        let names: Vec<&str> = r.parts.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(names, PARTS);

        let object = &r.parts[0];
        assert_eq!(object.base.as_deref(), Some("80200000"));
        let bytes = from_hex(object.hex.as_deref().unwrap()).unwrap();
        assert_eq!(bytes.len(), 0x464);
        assert_eq!(bytes[4..8], 0xdead_beefu32.to_le_bytes());
        assert_eq!(r.parts[2].base.as_deref(), Some("80410000"));
        let motion = from_hex(r.parts[3].hex.as_deref().unwrap()).unwrap();
        let z = (l::MOTION_POSITION + 8) as usize;
        assert_eq!(motion[z..z + 4], (-3.2f32).to_le_bytes());
        // The entity has no shape.
        let shape = &r.parts[4];
        assert!(shape.hex.is_none() && shape.base.is_none());
        assert!(shape.missing.as_deref().unwrap().contains("no shape"));
        // The stamp.
        assert!(v["result"]["at"].is_object(), "{v}");
    }

    #[test]
    fn parts_pick_and_order_the_answer() {
        let h = Harness::new();
        let mut game = game();
        let p = json!({"id": "player.0", "parts": ["motion", "object"], "len": 16});
        let v = h.call(&mut game, p);
        let r: Raw = serde_json::from_value(v["result"].clone()).unwrap();
        let names: Vec<&str> = r.parts.iter().map(|p| p.part.as_str()).collect();
        assert_eq!(names, ["object", "motion"]);
        // The header's size wins over len.
        assert_eq!(r.parts[0].hex.as_ref().unwrap().len(), 2 * 0x464);
    }

    #[test]
    fn bad_params_and_missing_objects_are_refused() {
        let h = Harness::new();
        let mut game = game();
        let bad = [
            json!({}),
            json!({"id": "player.0", "parts": ["havok"]}),
            json!({"id": "player.0", "parts": []}),
            json!({"id": "player.0", "len": 0}),
            json!({"id": "nonsense"}),
            json!({"id": "player.0", "extra": 1}),
        ];
        for p in bad {
            let v = h.call(&mut game, p.clone());
            assert_eq!(error_code(&v), code::INVALID_PARAMS, "{p} {v}");
        }
        let v = h.call(&mut game, json!({"id": "player.3"}));
        assert_eq!(error_code(&v), code::GONE, "{v}");

        // A listed id whose datum is stale.
        game.objects[0].unit = Some("12340007".into());
        let v = h.call(&mut game, json!({"id": "player.0"}));
        assert_eq!(error_code(&v), code::GONE, "{v}");

        // No guest memory: unavailable, not a panic.
        let mut none = game;
        none.g = None;
        let v = h.call(&mut none, json!({"id": "player.0"}));
        assert_eq!(error_code(&v), code::UNAVAILABLE, "{v}");
    }

    #[test]
    fn the_method_is_listed() {
        let h = Harness::new();
        h.agent.submit(
            h.conn,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#,
        );
        h.agent.between_slices(&mut game(), 0);
        let line = h.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(line.contains(method::RAW), "{line}");
    }
}
